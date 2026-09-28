//! Fixed native transceiver slots and bounded decoded-media observations. No
//! receiving task is spawned: diagnostics drain bounded queues on demand, and
//! peer teardown synchronously detaches every sink.
use crate::{engine_error, LibWebRtcSource};
use libwebrtc::{
    audio_stream::native::NativeAudioStream,
    media_stream_track::MediaStreamTrack,
    peer_connection::PeerConnection,
    peer_connection_factory::PeerConnectionFactory,
    rtp_sender::VideoEncoderBackend,
    rtp_transceiver::{RtpTransceiver, RtpTransceiverDirection, RtpTransceiverInit},
    stats::RtcStats,
    video_stream::native::NativeVideoStream,
    MediaType,
};
use pv_media_runtime::{ports::SourcePort, MediaObservation, MediaSlot, NativeError, SourceKind};
use std::{
    collections::BTreeSet,
    pin::Pin,
    task::{Context, Poll, Waker},
};
use tokio_stream::Stream;

pub(crate) const KINDS: [SourceKind; 3] = [
    SourceKind::Camera,
    SourceKind::Screen,
    SourceKind::Microphone,
];

pub(crate) struct Media {
    slots: Vec<Slot>,
}
struct Slot {
    kind: SourceKind,
    transceiver: RtpTransceiver,
    receiver: Option<Decoded>,
    observed_frames: u64,
    content_signature: u64,
    audio_energy: u64,
}
enum Decoded {
    Video(NativeVideoStream),
    Audio(NativeAudioStream),
}

impl Media {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    pub fn reserve(
        &mut self,
        connection: &PeerConnection,
        factory: &PeerConnectionFactory,
    ) -> Result<(), NativeError> {
        for kind in KINDS {
            let transceiver = connection
                .add_transceiver_for_media(
                    media_type(kind),
                    RtpTransceiverInit {
                        direction: RtpTransceiverDirection::SendRecv,
                        stream_ids: Vec::new(),
                        send_encodings: Vec::new(),
                    },
                )
                .map_err(|_| engine_error("Could not reserve native media slots"))?;
            configure(&transceiver, factory, kind)?;
            self.slots.push(Slot {
                kind,
                transceiver,
                receiver: None,
                observed_frames: 0,
                content_signature: 0,
                audio_energy: 0,
            });
        }
        Ok(())
    }

    pub fn adopt(
        &mut self,
        connection: &PeerConnection,
        factory: &PeerConnectionFactory,
        offered: &[MediaSlot],
    ) -> Result<(), NativeError> {
        validate_slots(offered)?;
        let transceivers = connection.transceivers();
        if transceivers.len() != KINDS.len() {
            return Err(engine_error("Unexpected native media slot count"));
        }
        for kind in KINDS {
            let mid = &offered
                .iter()
                .find(|slot| slot.kind == kind)
                .expect("validated slot")
                .mid;
            let transceiver = transceivers
                .iter()
                .find(|t| t.mid().as_ref() == Some(mid))
                .ok_or_else(|| engine_error("Offered media slot MID is absent"))?
                .clone();
            let track = transceiver
                .receiver()
                .track()
                .ok_or_else(|| engine_error("Native receiver has no track"))?;
            if !matches!(
                (&track, kind),
                (MediaStreamTrack::Audio(_), SourceKind::Microphone)
                    | (
                        MediaStreamTrack::Video(_),
                        SourceKind::Camera | SourceKind::Screen
                    )
            ) {
                return Err(engine_error("Offered media slot kind differs from SDP"));
            }
            transceiver
                .set_direction(RtpTransceiverDirection::SendRecv)
                .map_err(|_| engine_error("Could not adopt bidirectional media slot"))?;
            configure(&transceiver, factory, kind)?;
            self.slots.push(Slot {
                kind,
                transceiver,
                receiver: None,
                observed_frames: 0,
                content_signature: 0,
                audio_energy: 0,
            });
        }
        Ok(())
    }

    pub fn descriptors(&self) -> Vec<MediaSlot> {
        self.slots
            .iter()
            .filter_map(|slot| {
                slot.transceiver.mid().map(|mid| MediaSlot {
                    kind: slot.kind,
                    mid,
                })
            })
            .collect()
    }

    pub fn accept(&mut self, answer: &[MediaSlot]) -> Result<(), NativeError> {
        validate_slots(answer)?;
        if self.descriptors() != answer {
            return Err(engine_error("Remote media slots differ from the offer"));
        }
        self.observe()
    }

    pub fn observe(&mut self) -> Result<(), NativeError> {
        for slot in &mut self.slots {
            if slot.transceiver.current_direction() != Some(RtpTransceiverDirection::SendRecv) {
                return Err(engine_error("Native media slot is not bidirectional"));
            }
            let track = slot
                .transceiver
                .receiver()
                .track()
                .ok_or_else(|| engine_error("Native receiver has no track"))?;
            slot.receiver = Some(match track {
                MediaStreamTrack::Video(track) => Decoded::Video(NativeVideoStream::new(track)),
                MediaStreamTrack::Audio(track) => {
                    Decoded::Audio(NativeAudioStream::new(track, 48_000, 1))
                }
            });
        }
        Ok(())
    }

    pub fn set_source(
        &mut self,
        kind: SourceKind,
        source: Option<&dyn SourcePort>,
    ) -> Result<(), NativeError> {
        let slot = self
            .slots
            .iter()
            .find(|slot| slot.kind == kind)
            .ok_or_else(|| engine_error("Native media slot is not ready"))?;
        let track = source
            .map(|source| {
                if source.kind() != kind {
                    return Err(engine_error("Native source kind differs from its slot"));
                }
                source
                    .as_any()
                    .downcast_ref::<LibWebRtcSource>()
                    .ok_or_else(|| engine_error("Source belongs to another native engine"))?
                    .track()
            })
            .transpose()?;
        slot.transceiver
            .sender()
            .set_track(track)
            .map_err(|_| engine_error("Native source binding failed"))
    }

    pub async fn stats(
        &mut self,
        connection: &PeerConnection,
    ) -> Result<Vec<MediaObservation>, NativeError> {
        let stats = connection
            .get_stats()
            .await
            .map_err(|_| engine_error("Native media statistics failed"))?;
        let mut result = Vec::with_capacity(self.slots.len());
        for slot in &mut self.slots {
            let Some(mid) = slot.transceiver.mid() else {
                continue;
            };
            slot.sample();
            let mut observation = MediaObservation {
                kind: slot.kind,
                mid,
                frames_decoded: 0,
                total_samples_received: 0,
                bytes_received: 0,
                observed_frames: slot.observed_frames,
                content_signature: slot.content_signature,
                audio_energy: slot.audio_energy,
            };
            for item in &stats {
                if let RtcStats::InboundRtp(item) = item {
                    if item.inbound.mid == observation.mid {
                        observation.frames_decoded += u64::from(item.inbound.frames_decoded);
                        observation.total_samples_received += item.inbound.total_samples_received;
                        observation.bytes_received += item.inbound.bytes_received;
                    }
                }
            }
            result.push(observation);
        }
        Ok(result)
    }

    pub fn close(&mut self) {
        for slot in &mut self.slots {
            if let Some(receiver) = &mut slot.receiver {
                match receiver {
                    Decoded::Video(stream) => stream.close(),
                    Decoded::Audio(stream) => stream.close(),
                }
            }
            slot.receiver.take();
            // A failing detach must not retain a source past the subsequent PC close.
            let _ = slot.transceiver.sender().set_track(None);
        }
        self.slots.clear();
    }
}

impl Slot {
    fn sample(&mut self) {
        let mut context = Context::from_waker(Waker::noop());
        match &mut self.receiver {
            Some(Decoded::Video(stream)) => {
                // Never allow a concurrent producer to turn diagnostic draining
                // into an unbounded loop. Video retains at most one latest frame.
                if let Poll::Ready(Some(frame)) = Pin::new(stream).poll_next(&mut context) {
                    self.observed_frames = self.observed_frames.saturating_add(1);
                    let buffer = frame.buffer.as_ref().to_i420();
                    let (y, _, _) = buffer.data();
                    self.content_signature = signature(
                        y.iter()
                            .step_by((y.len() / 64).max(1))
                            .copied()
                            .map(u64::from),
                    );
                }
            }
            Some(Decoded::Audio(stream)) => {
                for _ in 0..10 {
                    let Poll::Ready(Some(frame)) = Pin::new(&mut *stream).poll_next(&mut context)
                    else {
                        break;
                    };
                    self.observed_frames = self.observed_frames.saturating_add(1);
                    for sample in frame.data.iter() {
                        self.audio_energy = self
                            .audio_energy
                            .saturating_add(i64::from(*sample).unsigned_abs().pow(2));
                    }
                    self.content_signature = signature(
                        frame
                            .data
                            .iter()
                            .take(64)
                            .map(|sample| u64::from(*sample as u16)),
                    );
                }
            }
            None => {}
        }
    }
}

fn signature(values: impl Iterator<Item = u64>) -> u64 {
    values.fold(0xcbf29ce484222325, |hash, value| {
        (hash ^ value).wrapping_mul(0x100000001b3)
    })
}
fn media_type(kind: SourceKind) -> MediaType {
    if kind == SourceKind::Microphone {
        MediaType::Audio
    } else {
        MediaType::Video
    }
}
fn configure(
    transceiver: &RtpTransceiver,
    factory: &PeerConnectionFactory,
    kind: SourceKind,
) -> Result<(), NativeError> {
    if kind != SourceKind::Microphone {
        let codecs: Vec<_> = factory
            .get_rtp_sender_capabilities(MediaType::Video)
            .codecs
            .into_iter()
            .filter(|codec| codec.mime_type.eq_ignore_ascii_case("video/vp8"))
            .collect();
        if codecs.is_empty() {
            return Err(engine_error("Native VP8 codec is unavailable"));
        }
        transceiver
            .set_codec_preferences(codecs)
            .map_err(|_| engine_error("Could not select native video codec"))?;
        transceiver
            .sender()
            .set_video_encoder_backend(VideoEncoderBackend::Software);
    }
    Ok(())
}
pub(crate) fn validate_slots(slots: &[MediaSlot]) -> Result<(), NativeError> {
    if slots.len() != KINDS.len()
        || slots.iter().map(|slot| slot.kind).collect::<BTreeSet<_>>()
            != KINDS.into_iter().collect()
        || slots.iter().any(|slot| {
            slot.mid.is_empty() || slot.mid.len() > 128 || slot.mid.chars().any(char::is_control)
        })
        || slots
            .iter()
            .map(|slot| &slot.mid)
            .collect::<BTreeSet<_>>()
            .len()
            != KINDS.len()
    {
        return Err(engine_error("Expected three distinct native media slots"));
    }
    Ok(())
}
