//! Shared native integration fixtures; never linked into production.
use parentview_signaling::{config::Config, AppState};
use pv_media_libwebrtc::{
    I420Buffer, LibWebRtcFactory, LibWebRtcSource, VideoFrame, VideoRotation,
};
use pv_media_native::{
    Event, JoinOptions, MediaObservation, NativeError, NativeMediaClient, SourceKind,
};
use pv_media_runtime::{ports::*, Candidate, Description, IceServer, MediaSlot};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    net::TcpListener,
    sync::watch,
    task::JoinHandle,
    time::{sleep, timeout},
};

pub(crate) struct Server {
    state: AppState,
    url: String,
    task: JoinHandle<()>,
}
impl Server {
    pub(crate) async fn start() -> Self {
        let state = AppState::new(Config::default()).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/ws", listener.local_addr().unwrap());
        let app = parentview_signaling::router(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { state, url, task }
    }
    pub(crate) fn options(&self) -> Vec<JoinOptions> {
        let devices: Vec<_> = (0..2)
            .map(|_| self.state.create_device(Instant::now()).unwrap())
            .collect();
        let (room_id, room_token, _) = self
            .state
            .create_room(&devices[0].device_token, Instant::now())
            .unwrap();
        devices
            .into_iter()
            .map(|device| JoinOptions {
                signaling_url: self.url.clone(),
                room_id: room_id.clone(),
                room_token: room_token.clone(),
                device_token: device.device_token,
                ice_servers: vec![],
            })
            .collect()
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.state.disconnect_all();
        self.task.abort();
    }
}

// Count real offer/answer invocations while preserving the production adapter.
pub(crate) struct CountedFactory {
    pub(crate) engine: Arc<LibWebRtcFactory>,
    pub(crate) exchanges: Arc<AtomicUsize>,
}
impl EngineFactory for CountedFactory {
    fn create(
        &self,
        peer_id: &str,
        initiator: bool,
        ice: &[IceServer],
        events: EventSink,
    ) -> Result<Box<dyn PeerPort>, NativeError> {
        Ok(Box::new(CountedPeer {
            inner: self.engine.create(peer_id, initiator, ice, events)?,
            exchanges: self.exchanges.clone(),
        }))
    }
}
struct CountedPeer {
    inner: Box<dyn PeerPort>,
    pub(crate) exchanges: Arc<AtomicUsize>,
}
impl PeerPort for CountedPeer {
    fn offer(&mut self) -> PortFuture<'_, Description> {
        self.exchanges.fetch_add(1, Ordering::SeqCst);
        self.inner.offer()
    }
    fn answer(&mut self, offer: Description) -> PortFuture<'_, Description> {
        self.exchanges.fetch_add(1, Ordering::SeqCst);
        self.inner.answer(offer)
    }
    fn accept_answer(&mut self, answer: Description) -> PortFuture<'_, ()> {
        self.inner.accept_answer(answer)
    }
    fn add_candidate(&mut self, candidate: Candidate) -> PortFuture<'_, ()> {
        self.inner.add_candidate(candidate)
    }
    fn send(&mut self, data: &str) -> Result<(), NativeError> {
        self.inner.send(data)
    }
    fn slots(&self) -> Vec<MediaSlot> {
        self.inner.slots()
    }
    fn set_source(
        &mut self,
        kind: SourceKind,
        source: Option<&dyn SourcePort>,
    ) -> Result<(), NativeError> {
        self.inner.set_source(kind, source)
    }
    fn media_stats(&mut self) -> PortFuture<'_, Vec<MediaObservation>> {
        self.inner.media_stats()
    }
    fn close(&mut self) {
        self.inner.close();
    }
}

struct SyntheticSource {
    source: LibWebRtcSource,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    closed: Arc<AtomicUsize>,
}
impl SourcePort for SyntheticSource {
    fn kind(&self) -> SourceKind {
        self.source.kind()
    }
    fn as_any(&self) -> &dyn Any {
        self.source.as_any()
    }
    fn close(&mut self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.stop.send_replace(true);
            if let Some(task) = self.task.as_mut() {
                let result = task.await;
                self.task.take();
                result
                    .map_err(|_| NativeError::new("producer", "Synthetic producer did not stop"))?;
                self.closed.fetch_add(1, Ordering::SeqCst);
            }
            self.source.close().await
        })
    }
}
impl Drop for SyntheticSource {
    fn drop(&mut self) {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub(crate) fn synthetic(
    factory: &LibWebRtcFactory,
    kind: SourceKind,
    marker: u8,
    closed: Arc<AtomicUsize>,
) -> Box<dyn SourcePort> {
    let (stop, mut stopping) = watch::channel(false);
    let (source, task) = if kind == SourceKind::Microphone {
        let (source, input) = factory.audio_source().unwrap();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(10));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut position = 0usize;
            loop {
                tokio::select! { biased;
                    _ = stopping.changed() => break,
                    _ = interval.tick() => {
                        let mut pcm = [0i16; 480];
                        for sample in &mut pcm {
                            *sample = ((position as f64 * std::f64::consts::TAU * (400.0 + f64::from(marker)) / 48_000.0).sin() * 8000.0) as i16;
                            position += 1;
                        }
                        if input.capture_10ms(&pcm).is_err() { break; }
                    }
                }
            }
        });
        (source, task)
    } else {
        let (source, input) = factory.video_source(kind, 320, 180).unwrap();
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(100));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut index = 0u8;
            loop {
                tokio::select! { biased;
                    _ = stopping.changed() => break,
                    _ = interval.tick() => {
                        let mut buffer = I420Buffer::new_black(320, 180);
                        let (y, u, v) = buffer.data_mut();
                        y.fill(marker + (index % 3) * 12);
                        u.fill(if kind == SourceKind::Camera { 100 } else { 150 });
                        v.fill(128);
                        index = index.wrapping_add(1);
                        if input.capture(VideoFrame::new(VideoRotation::VideoRotation0, buffer)).is_err() { break; }
                    }
                }
            }
        });
        (source, task)
    };
    Box::new(SyntheticSource {
        source,
        stop,
        task: Some(task),
        closed,
    })
}

pub(crate) async fn ready(client: &NativeMediaClient, peers: usize) {
    timeout(Duration::from_secs(15), async {
        loop {
            if client.snapshot().ready_peers.len() == peers
                && client.snapshot().peers.len() == peers
            {
                break;
            }
            for event in client.read_batch().await.unwrap().events {
                if let Event::Error { error } = event {
                    panic!("native media readiness failed: {error}");
                }
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("media readiness timeout: {:?}", client.snapshot()));
}
pub(crate) async fn observations(
    client: &NativeMediaClient,
) -> BTreeMap<SourceKind, MediaObservation> {
    let mut stats = client.media_stats().await.unwrap();
    assert_eq!(stats.len(), 1);
    stats
        .pop()
        .unwrap()
        .media
        .into_iter()
        .map(|item| (item.kind, item))
        .collect()
}
pub(crate) async fn decoded(
    client: &NativeMediaClient,
    baseline: Option<&BTreeMap<SourceKind, MediaObservation>>,
) -> BTreeMap<SourceKind, MediaObservation> {
    timeout(Duration::from_secs(15), async {
        loop {
            let media = observations(client).await;
            if media.len() == 3
                && media.values().all(|item| {
                    let previous = baseline.and_then(|before| before.get(&item.kind));
                    item.bytes_received > previous.map_or(0, |before| before.bytes_received)
                        && item.observed_frames
                            > previous.map_or(0, |before| before.observed_frames)
                        && if item.kind == SourceKind::Microphone {
                            item.total_samples_received
                                > previous.map_or(0, |before| before.total_samples_received)
                                && item.audio_energy
                                    > previous.map_or(0, |before| before.audio_energy)
                        } else {
                            item.frames_decoded > previous.map_or(2, |before| before.frames_decoded)
                                && item.content_signature != 0
                        }
                })
            {
                return media;
            }
            sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("decoded media deadline: {:?}", client.snapshot()))
}
pub(crate) async fn changed_video(
    client: &NativeMediaClient,
    baseline: &BTreeMap<SourceKind, MediaObservation>,
) {
    timeout(Duration::from_secs(5), async {
        let mut changed = BTreeSet::new();
        while changed.len() < 2 {
            let current = observations(client).await;
            for kind in [SourceKind::Camera, SourceKind::Screen] {
                if current[&kind].frames_decoded > baseline[&kind].frames_decoded
                    && current[&kind].content_signature != baseline[&kind].content_signature
                {
                    changed.insert(kind);
                }
            }
            if changed.len() < 2 {
                sleep(Duration::from_millis(50)).await;
            }
        }
    })
    .await
    .expect("changing decoded camera and screen");
}
pub(crate) async fn received_data(client: &NativeMediaClient, expected: &str) {
    timeout(Duration::from_secs(5), async {
        loop {
            for event in client.read_batch().await.unwrap().events {
                match event {
                    Event::Message { data, .. } if data == expected => return,
                    Event::Error { error } => panic!("data with media failed: {error}"),
                    _ => {}
                }
            }
        }
    })
    .await
    .expect("data delivery while native AV is active");
}
pub(crate) async fn remote_sources(client: &NativeMediaClient, prefix: &str, count: usize) {
    timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = client.snapshot();
            if snapshot.remote_sources.len() == count
                && snapshot
                    .remote_sources
                    .iter()
                    .all(|source| source.id.starts_with(prefix))
            {
                break;
            }
            for event in client.read_batch().await.unwrap().events {
                if let Event::Error { error } = event {
                    panic!("remote sources failed: {error}");
                }
            }
        }
    })
    .await
    .expect("remote source manifest deadline");
}
pub(crate) async fn publish_all(
    client: &NativeMediaClient,
    factory: &LibWebRtcFactory,
    prefix: &str,
    closed: &Arc<AtomicUsize>,
    marker: u8,
) {
    for (index, kind) in [
        SourceKind::Camera,
        SourceKind::Screen,
        SourceKind::Microphone,
    ]
    .into_iter()
    .enumerate()
    {
        client
            .publish(
                format!("{prefix}-{index}"),
                synthetic(factory, kind, marker + index as u8 * 50, closed.clone()),
            )
            .await
            .unwrap();
    }
}
