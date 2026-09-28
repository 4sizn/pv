//! Native frame ingress. Owns tracks and ingress lifetime, not capture devices or
//! producer scheduling. Closing waits for in-flight synchronous pushes, releases
//! the source, and permanently rejects future pushes through cloned inputs.
use crate::{engine_error, LibWebRtcFactory};
use libwebrtc::{
    audio_frame::AudioFrame,
    audio_source::{native::NativeAudioSource, AudioSourceOptions},
    media_stream_track::MediaStreamTrack,
    peer_connection_factory::native::PeerConnectionFactoryExt,
    video_source::{native::NativeVideoSource, VideoResolution},
};
use pv_media_runtime::{
    ports::{PortFuture, SourcePort},
    NativeError, SourceKind,
};
use std::{
    any::Any,
    borrow::Cow,
    future::Future,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};

pub use libwebrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};

enum Ingress {
    Video {
        source: NativeVideoSource,
        width: u32,
        height: u32,
    },
    Audio(NativeAudioSource),
}

pub struct LibWebRtcSource {
    kind: SourceKind,
    track: Option<MediaStreamTrack>,
    ingress: Arc<Mutex<Option<Ingress>>>,
}
impl LibWebRtcSource {
    pub(crate) fn track(&self) -> Result<MediaStreamTrack, NativeError> {
        self.track.clone().ok_or_else(closed)
    }

    fn release(&mut self) {
        self.ingress.lock().expect("native ingress lock").take();
        self.track.take();
    }
}
impl SourcePort for LibWebRtcSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn close(&mut self) -> PortFuture<'_, ()> {
        self.release();
        Box::pin(async { Ok(()) })
    }
}
impl Drop for LibWebRtcSource {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Clone)]
pub struct VideoInput {
    ingress: Arc<Mutex<Option<Ingress>>>,
}
impl VideoInput {
    /// The frame must own initialized I420 planes. The engine may retain the
    /// buffer; consuming the frame prevents later mutation. False means engine adaptation
    /// dropped this frame, rather than an ingress error.
    pub fn capture(&self, frame: VideoFrame<I420Buffer>) -> Result<bool, NativeError> {
        use libwebrtc::video_frame::VideoBuffer;
        let guard = self.ingress.lock().expect("native ingress lock");
        let Some(Ingress::Video {
            source,
            width,
            height,
        }) = guard.as_ref()
        else {
            return Err(closed());
        };
        if frame.buffer.width() != *width || frame.buffer.height() != *height {
            return Err(NativeError::new(
                "invalid-frame",
                "Video dimensions differ from the source",
            ));
        }
        Ok(source.capture_frame(&frame))
    }
}

#[derive(Clone)]
pub struct AudioInput {
    ingress: Arc<Mutex<Option<Ingress>>>,
}
impl AudioInput {
    /// Push one caller-paced 10 ms frame of signed 16-bit mono PCM at 48 kHz.
    /// This source always has queue_size_ms=0: the pinned binding completes this
    /// fast path synchronously. A single poll holds the ingress lock until FFI
    /// returns, so close cannot race an in-flight push or leave a pending task.
    pub fn capture_10ms(&self, samples: &[i16]) -> Result<(), NativeError> {
        if samples.len() != 480 {
            return Err(NativeError::new(
                "invalid-frame",
                "Expected 480 mono PCM samples",
            ));
        }
        let guard = self.ingress.lock().expect("native ingress lock");
        let Some(Ingress::Audio(source)) = guard.as_ref() else {
            return Err(closed());
        };
        let frame = AudioFrame {
            data: Cow::Borrowed(samples),
            sample_rate: 48_000,
            num_channels: 1,
            samples_per_channel: 480,
        };
        let future = source.capture_frame(&frame);
        let mut future = std::pin::pin!(future);
        match future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(result) => result.map_err(|_| engine_error("Native audio ingress failed")),
            Poll::Pending => Err(engine_error(
                "Native audio fast path unexpectedly suspended",
            )),
        }
    }
}

impl LibWebRtcFactory {
    pub fn video_source(
        &self,
        kind: SourceKind,
        width: u32,
        height: u32,
    ) -> Result<(LibWebRtcSource, VideoInput), NativeError> {
        if kind == SourceKind::Microphone
            || !(2..=1920).contains(&width)
            || !(2..=1080).contains(&height)
            || width % 2 != 0
            || height % 2 != 0
        {
            return Err(NativeError::new(
                "invalid-source",
                "Expected a camera or screen source with bounded even dimensions",
            ));
        }
        let source = NativeVideoSource::new_without_keepalive(
            VideoResolution { width, height },
            kind == SourceKind::Screen,
        );
        let track = self
            .factory
            .create_video_track(&libwebrtc::native::create_random_uuid(), source.clone())
            .into();
        let ingress = Arc::new(Mutex::new(Some(Ingress::Video {
            source,
            width,
            height,
        })));
        Ok((
            LibWebRtcSource {
                kind,
                track: Some(track),
                ingress: ingress.clone(),
            },
            VideoInput { ingress },
        ))
    }

    pub fn audio_source(&self) -> Result<(LibWebRtcSource, AudioInput), NativeError> {
        let source = NativeAudioSource::new(AudioSourceOptions::default(), 48_000, 1, 0);
        let track = self
            .factory
            .create_audio_track(&libwebrtc::native::create_random_uuid(), source.clone())
            .into();
        let ingress = Arc::new(Mutex::new(Some(Ingress::Audio(source))));
        Ok((
            LibWebRtcSource {
                kind: SourceKind::Microphone,
                track: Some(track),
                ingress: ingress.clone(),
            },
            AudioInput { ingress },
        ))
    }
}

fn closed() -> NativeError {
    NativeError::new("source-closed", "Native source is closed")
}
