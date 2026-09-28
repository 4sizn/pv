use pv_media_runtime::{ports::*, *};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::time::timeout;

#[derive(Default)]
struct Harness {
    close_failure: AtomicBool,
    close_pending: AtomicBool,
    cancelled_closes: AtomicUsize,
    connect_pending: AtomicBool,
    offer_pending: AtomicBool,
    connects: AtomicUsize,
    cancelled_connects: AtomicUsize,
    signal_closes: AtomicUsize,
    peer_closes: AtomicUsize,
    candidate_count: AtomicUsize,
    sinks: Mutex<Vec<EventSink>>,
    peer_sinks: Mutex<BTreeMap<String, EventSink>>,
    initial_peers: Mutex<Vec<String>>,
    sent_signals: Mutex<Vec<(String, SignalPayload)>>,
    bindings: Mutex<BTreeMap<(String, SourceKind), usize>>,
    binding_failures: Mutex<BTreeSet<(String, bool)>>,
    bind_calls: AtomicUsize,
    stats_pending: AtomicBool,
    stats_calls: AtomicUsize,
}
struct FakeSignaling(Arc<Harness>);
struct Connection(Arc<Harness>);
struct FakeEngine(Arc<Harness>);
struct FakePeer {
    harness: Arc<Harness>,
    closed: bool,
    id: String,
    negotiated: bool,
}
impl SignalingFactory for FakeSignaling {
    fn connect(&self, _: JoinOptions, events: EventSink) -> PortFuture<'_, Connected> {
        Box::pin(async move {
            self.0.connects.fetch_add(1, Ordering::SeqCst);
            self.0.sinks.lock().unwrap().push(events);
            if self.0.connect_pending.load(Ordering::SeqCst) {
                struct PendingGuard(Arc<Harness>);
                impl Drop for PendingGuard {
                    fn drop(&mut self) {
                        self.0.cancelled_connects.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let _guard = PendingGuard(self.0.clone());
                std::future::pending::<()>().await;
            }
            Ok(Connected {
                peer_id: "a".into(),
                peers: self.0.initial_peers.lock().unwrap().clone(),
                signaling: Box::new(Connection(self.0.clone())),
            })
        })
    }
}
impl SignalingPort for Connection {
    fn send(&mut self, to: &str, payload: SignalPayload) -> Result<(), NativeError> {
        self.0
            .sent_signals
            .lock()
            .unwrap()
            .push((to.into(), payload));
        Ok(())
    }
    fn close(&mut self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.0.signal_closes.fetch_add(1, Ordering::SeqCst);
            if self.0.close_failure.load(Ordering::SeqCst) {
                return Err(NativeError::new(
                    "cleanup-failed",
                    "Injected socket cleanup failure",
                ));
            }
            if self.0.close_pending.load(Ordering::SeqCst) {
                struct PendingClose(Arc<Harness>);
                impl Drop for PendingClose {
                    fn drop(&mut self) {
                        self.0.cancelled_closes.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let _guard = PendingClose(self.0.clone());
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }
}
impl EngineFactory for FakeEngine {
    fn create(
        &self,
        id: &str,
        _: bool,
        _: &[IceServer],
        events: EventSink,
    ) -> Result<Box<dyn PeerPort>, NativeError> {
        self.0.peer_sinks.lock().unwrap().insert(id.into(), events);
        Ok(Box::new(FakePeer {
            harness: self.0.clone(),
            closed: false,
            id: id.into(),
            negotiated: false,
        }))
    }
}
impl PeerPort for FakePeer {
    fn offer(&mut self) -> PortFuture<'_, Description> {
        Box::pin(async move {
            if self.harness.offer_pending.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            Ok(Description {
                r#type: DescriptionType::Offer,
                sdp: "test-offer".into(),
                slots: media_slots(),
            })
        })
    }
    fn answer(&mut self, _: Description) -> PortFuture<'_, Description> {
        Box::pin(async move {
            self.negotiated = true;
            Ok(Description {
                r#type: DescriptionType::Answer,
                sdp: "test-answer".into(),
                slots: media_slots(),
            })
        })
    }
    fn accept_answer(&mut self, _: Description) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.negotiated = true;
            Ok(())
        })
    }
    fn add_candidate(&mut self, _: Candidate) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.harness.candidate_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }
    fn send(&mut self, _: &str) -> Result<(), NativeError> {
        Ok(())
    }
    fn slots(&self) -> Vec<MediaSlot> {
        if self.negotiated {
            media_slots()
        } else {
            vec![]
        }
    }
    fn set_source(
        &mut self,
        kind: SourceKind,
        source: Option<&dyn SourcePort>,
    ) -> Result<(), NativeError> {
        assert!(self.negotiated, "Sources must wait for negotiated slots");
        self.harness.bind_calls.fetch_add(1, Ordering::SeqCst);
        let key = (self.id.clone(), kind);
        if let Some(source) = source {
            let source = source.as_any().downcast_ref::<FakeSource>().unwrap();
            self.harness
                .bindings
                .lock()
                .unwrap()
                .insert(key, source.token);
        } else if !self
            .harness
            .binding_failures
            .lock()
            .unwrap()
            .contains(&(self.id.clone(), false))
        {
            self.harness.bindings.lock().unwrap().remove(&key);
        }
        if self
            .harness
            .binding_failures
            .lock()
            .unwrap()
            .contains(&(self.id.clone(), source.is_some()))
        {
            return Err(NativeError::new(
                "binding-failed",
                "Injected source binding failure",
            ));
        }
        Ok(())
    }
    fn media_stats(&mut self) -> PortFuture<'_, Vec<MediaObservation>> {
        Box::pin(async move {
            self.harness.stats_calls.fetch_add(1, Ordering::SeqCst);
            if self.harness.stats_pending.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            Ok(vec![MediaObservation {
                kind: SourceKind::Camera,
                mid: "camera-mid".into(),
                frames_decoded: 7,
                total_samples_received: 0,
                bytes_received: 100,
                observed_frames: 1,
                content_signature: 42,
                audio_energy: 0,
            }])
        })
    }
    fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.harness.peer_closes.fetch_add(1, Ordering::SeqCst);
            self.harness
                .bindings
                .lock()
                .unwrap()
                .retain(|(id, _), _| id != &self.id);
        }
    }
}
fn media_slots() -> Vec<MediaSlot> {
    [
        (SourceKind::Camera, "camera-mid"),
        (SourceKind::Screen, "screen-mid"),
        (SourceKind::Microphone, "audio-mid"),
    ]
    .into_iter()
    .map(|(kind, mid)| MediaSlot {
        kind,
        mid: mid.into(),
    })
    .collect()
}

#[derive(Default)]
struct SourceState {
    closes: AtomicUsize,
    drops: AtomicUsize,
    close_failure: AtomicBool,
    close_pending: AtomicBool,
    stopped: AtomicBool,
    close_release: tokio::sync::Notify,
}
struct FakeSource {
    kind: SourceKind,
    token: usize,
    state: Arc<SourceState>,
}
impl SourcePort for FakeSource {
    fn kind(&self) -> SourceKind {
        self.kind
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn close(&mut self) -> PortFuture<'_, ()> {
        Box::pin(async move {
            self.state.closes.fetch_add(1, Ordering::SeqCst);
            if self.state.close_pending.load(Ordering::SeqCst) {
                self.state.close_release.notified().await;
            }
            if self.state.close_failure.load(Ordering::SeqCst) {
                return Err(NativeError::new(
                    "source-cleanup",
                    "Injected source cleanup failure",
                ));
            }
            self.state.stopped.store(true, Ordering::SeqCst);
            Ok(())
        })
    }
}
impl Drop for FakeSource {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::SeqCst);
        self.state.drops.fetch_add(1, Ordering::SeqCst);
    }
}
fn source(kind: SourceKind, token: usize) -> (Box<dyn SourcePort>, Arc<SourceState>) {
    let state = Arc::new(SourceState::default());
    (
        Box::new(FakeSource {
            kind,
            token,
            state: state.clone(),
        }),
        state,
    )
}
impl Harness {
    fn client(self: &Arc<Self>) -> NativeMediaClient {
        NativeMediaClient::new(
            Arc::new(FakeEngine(self.clone())),
            Arc::new(FakeSignaling(self.clone())),
        )
        .unwrap()
    }
    fn signaling(&self, event: SignalingEvent) {
        self.sinks
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .send(Input::Signaling(event));
    }
    fn engine(&self, id: &str, event: EngineEvent) {
        self.peer_sinks.lock().unwrap()[id].send(Input::Engine {
            peer_id: id.into(),
            event,
        });
    }
}
fn options() -> JoinOptions {
    JoinOptions {
        signaling_url: "ws://127.0.0.1/ws".into(),
        room_id: "room".into(),
        room_token: "invitation".into(),
        device_token: "device".into(),
        ice_servers: vec![],
    }
}
async fn until(mut condition: impl FnMut() -> bool) {
    timeout(Duration::from_secs(2), async {
        while !condition() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("condition deadline");
}

#[tokio::test]
async fn leave_cancels_pending_join_even_when_the_command_queue_is_full() {
    let h = Arc::new(Harness::default());
    h.connect_pending.store(true, Ordering::SeqCst);
    let client = h.client();
    let c = client.clone();
    let joining = tokio::spawn(async move { c.join(options()).await });
    until(|| h.connects.load(Ordering::SeqCst) == 1).await;
    let mut sends = vec![];
    for _ in 0..64 {
        let c = client.clone();
        sends.push(tokio::spawn(async move { c.send("queued".into()).await }));
    }
    tokio::task::yield_now().await;
    assert_eq!(
        timeout(Duration::from_secs(1), client.leave())
            .await
            .unwrap()
            .unwrap()
            .state,
        State::Idle
    );
    assert_eq!(joining.await.unwrap().unwrap_err().code, "cancelled");
    assert_eq!(h.cancelled_connects.load(Ordering::SeqCst), 1);
    for send in sends {
        assert!(send.await.unwrap().is_err());
    }
    h.connect_pending.store(false, Ordering::SeqCst);
    assert_eq!(client.join(options()).await.unwrap().state, State::Joined);
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn destroy_cancels_pending_sdp_and_closes_resources_once() {
    let h = Arc::new(Harness::default());
    h.offer_pending.store(true, Ordering::SeqCst);
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    let c = client.clone();
    let joining = tokio::spawn(async move { c.join(options()).await });
    until(|| !h.peer_sinks.lock().unwrap().is_empty()).await;
    assert_eq!(
        timeout(Duration::from_secs(1), client.destroy())
            .await
            .unwrap()
            .unwrap()
            .state,
        State::Destroyed
    );
    assert_eq!(joining.await.unwrap().unwrap_err().code, "cancelled");
    client.destroy().await.unwrap();
    client.leave().await.unwrap();
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 1);
    assert_eq!(h.signal_closes.load(Ordering::SeqCst), 1);
    assert_eq!(client.join(options()).await.unwrap_err().code, "destroyed");
}

#[tokio::test]
async fn failed_peer_is_isolated_and_readiness_is_engine_owned() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into(), "c".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    assert!(client.snapshot().ready_peers.is_empty());
    h.engine("b", EngineEvent::Ready(true));
    h.engine("c", EngineEvent::Ready(true));
    until(|| client.snapshot().ready_peers.len() == 2).await;
    h.engine("b", EngineEvent::Failed);
    until(|| client.snapshot().peers == ["c"]).await;
    assert_eq!(client.snapshot().ready_peers, ["c"]);
    assert_eq!(client.snapshot().state, State::Joined);
    assert_eq!(
        client
            .send("still running".into())
            .await
            .unwrap()
            .accepted_peer_ids,
        ["c"]
    );
    client.destroy().await.unwrap();
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn callback_overflow_closes_session_instead_of_silently_dropping_signals() {
    let h = Arc::new(Harness::default());
    h.offer_pending.store(true, Ordering::SeqCst);
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    let c = client.clone();
    let joining = tokio::spawn(async move { c.join(options()).await });
    until(|| !h.peer_sinks.lock().unwrap().is_empty()).await;
    for _ in 0..(INPUT_CAPACITY + 1) {
        h.engine("b", EngineEvent::Message("flood".into()));
    }
    assert_eq!(joining.await.unwrap().unwrap().state, State::Joined);
    until(|| client.snapshot().state == State::Idle).await;
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 1);
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn unread_messages_are_bounded_and_previous_generation_cannot_leak_into_rejoin() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    let old = h.peer_sinks.lock().unwrap()["b"].clone();
    for _ in 0..(PUBLIC_EVENT_CAPACITY + 1) {
        h.engine("b", EngineEvent::Message("unread".into()));
        tokio::task::yield_now().await;
    }
    until(|| client.snapshot().state == State::Idle).await;
    let batch = client.read_batch().await.unwrap();
    assert!(batch.events.len() <= EVENT_BATCH_LIMIT);
    assert!(batch
        .events
        .iter()
        .any(|event| matches!(event, Event::Error { error } if error.code == "event-overflow")));
    client.join(options()).await.unwrap();
    old.send(Input::Engine {
        peer_id: "b".into(),
        event: EngineEvent::Message("obsolete".into()),
    });
    h.engine("b", EngineEvent::Message("new".into()));
    let messages = timeout(Duration::from_secs(1), async {
        loop {
            let events = client.read_batch().await.unwrap().events;
            if !events.is_empty() {
                break events;
            }
        }
    })
    .await
    .unwrap();
    assert!(matches!(&messages[..], [Event::Message { data, .. }] if data == "new"));
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn early_ice_is_bounded_and_only_applied_after_description() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    for _ in 0..2 {
        h.signaling(SignalingEvent::Signal {
            from: "b".into(),
            payload: SignalPayload::Ice {
                candidate: Candidate {
                    candidate: "candidate:test".into(),
                    sdp_mid: Some("0".into()),
                    sdp_m_line_index: Some(0),
                },
            },
        });
    }
    tokio::task::yield_now().await;
    assert_eq!(h.candidate_count.load(Ordering::SeqCst), 0);
    h.signaling(SignalingEvent::Signal {
        from: "b".into(),
        payload: SignalPayload::Description {
            description: Description {
                r#type: DescriptionType::Answer,
                sdp: "answer".into(),
                slots: media_slots(),
            },
        },
    });
    until(|| h.candidate_count.load(Ordering::SeqCst) == 2).await;
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn single_pending_read_is_cancellable_and_destroy_wakes_it() {
    let h = Arc::new(Harness::default());
    let client = h.client();
    client.read_batch().await.unwrap();
    let c = client.clone();
    let pending = tokio::spawn(async move { c.read_batch().await });
    tokio::task::yield_now().await;
    assert_eq!(
        client.read_batch().await.unwrap_err().code,
        "read-in-progress"
    );
    pending.abort();
    let _ = pending.await;
    let c = client.clone();
    let pending = tokio::spawn(async move { c.read_batch().await });
    client.destroy().await.unwrap();
    assert_eq!(
        pending.await.unwrap().unwrap().snapshot.state,
        State::Destroyed
    );
}

#[tokio::test]
async fn invalid_commands_do_not_allocate_resources_and_utf8_limit_is_bytes() {
    let h = Arc::new(Harness::default());
    let client = h.client();
    let mut invalid = options();
    invalid.device_token.clear();
    assert_eq!(
        client.join(invalid).await.unwrap_err().code,
        "invalid-options"
    );
    assert_eq!(h.connects.load(Ordering::SeqCst), 0);
    assert_eq!(
        client.send("한".repeat(6000)).await.unwrap_err().code,
        "message-too-large"
    );
    assert_eq!(
        client.send("before join".into()).await.unwrap_err().code,
        "invalid-state"
    );
    client.join(options()).await.unwrap();
    assert_eq!(
        client.join(options()).await.unwrap_err().code,
        "invalid-state"
    );
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn dropping_last_handle_requests_resource_teardown() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    drop(client);
    until(|| h.signal_closes.load(Ordering::SeqCst) == 1).await;
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn callbacks_from_departed_peer_cannot_close_its_replacement_in_same_session() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    let old = h.peer_sinks.lock().unwrap()["b"].clone();
    h.signaling(SignalingEvent::PeerLeft("b".into()));
    until(|| client.snapshot().peers.is_empty()).await;
    h.signaling(SignalingEvent::PeerJoined("b".into()));
    until(|| client.snapshot().peers == ["b"]).await;
    old.send(Input::Engine {
        peer_id: "b".into(),
        event: EngineEvent::Failed,
    });
    h.engine("b", EngineEvent::Ready(true));
    until(|| client.snapshot().ready_peers == ["b"] || client.snapshot().peers.is_empty()).await;
    assert_eq!(client.snapshot().ready_peers, ["b"]);
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn cleanup_failure_is_returned_even_when_destruction_is_terminal() {
    let h = Arc::new(Harness::default());
    h.close_failure.store(true, Ordering::SeqCst);
    let client = h.client();
    client.join(options()).await.unwrap();
    assert_eq!(client.destroy().await.unwrap_err().code, "cleanup-failed");
    assert_eq!(client.snapshot().state, State::Destroyed);
    assert_eq!(client.destroy().await.unwrap_err().code, "cleanup-failed");
    assert_eq!(h.signal_closes.load(Ordering::SeqCst), 1);
    assert!(client
        .read_batch()
        .await
        .unwrap()
        .events
        .iter()
        .any(|event| matches!(event, Event::Error { error } if error.code == "cleanup-failed")));
}

#[tokio::test]
async fn cleanup_timeout_is_reported_and_pending_cleanup_is_cancelled() {
    let h = Arc::new(Harness::default());
    h.close_pending.store(true, Ordering::SeqCst);
    let client = h.client();
    client.join(options()).await.unwrap();
    let result = timeout(Duration::from_secs(3), client.leave())
        .await
        .unwrap();
    assert_eq!(result.unwrap_err().code, "cleanup-timeout");
    assert_eq!(h.cancelled_closes.load(Ordering::SeqCst), 1);
    assert_eq!(client.snapshot().state, State::Idle);
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn late_signal_to_departed_peer_does_not_disconnect_healthy_members() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into(), "c".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    h.signaling(SignalingEvent::PeerLeft("b".into()));
    h.signaling(SignalingEvent::Error(NativeError::new(
        "peer-unavailable",
        "The recipient has departed",
    )));
    until(|| client.snapshot().peers == ["c"]).await;
    let result = client.send("healthy peer".into()).await.unwrap();
    assert_eq!(result.accepted_peer_ids, ["c"]);
    assert_eq!(client.snapshot().state, State::Joined);
    client.destroy().await.unwrap();
}

async fn negotiate(h: &Harness, id: &str) {
    h.signaling(SignalingEvent::Signal {
        from: id.into(),
        payload: SignalPayload::Description {
            description: Description {
                r#type: if id < "a" {
                    DescriptionType::Offer
                } else {
                    DescriptionType::Answer
                },
                sdp: "fixture-sdp".into(),
                slots: media_slots(),
            },
        },
    });
    until(|| {
        h.sent_signals
            .lock()
            .unwrap()
            .iter()
            .any(|(to, payload)| to == id && matches!(payload, SignalPayload::Sources { .. }))
    })
    .await;
}

fn manifest(h: &Harness, id: &str, revision: u64, sources: Vec<SourceBinding>) {
    h.signaling(SignalingEvent::Signal {
        from: id.into(),
        payload: SignalPayload::Sources { revision, sources },
    });
}

fn camera_binding(id: &str) -> SourceBinding {
    SourceBinding {
        id: id.into(),
        kind: SourceKind::Camera,
        mid: "camera-mid".into(),
    }
}

#[tokio::test]
async fn publication_ownership_covers_invalid_duplicate_and_terminal_commands() {
    let h = Arc::new(Harness::default());
    let client = h.client();
    let (idle, idle_state) = source(SourceKind::Camera, 0);
    assert_eq!(
        client.publish("idle".into(), idle).await.unwrap_err().code,
        "invalid-state"
    );
    assert_eq!(idle_state.closes.load(Ordering::SeqCst), 1);
    assert_eq!(idle_state.drops.load(Ordering::SeqCst), 1);
    client.join(options()).await.unwrap();
    let (first, first_state) = source(SourceKind::Camera, 1);
    client.publish("camera-first".into(), first).await.unwrap();
    for (id, kind, code) in [
        ("camera-second", SourceKind::Camera, "duplicate-source"),
        ("camera-first", SourceKind::Screen, "duplicate-source"),
        ("", SourceKind::Microphone, "invalid-source"),
    ] {
        let (rejected, state) = source(kind, 2);
        assert_eq!(
            client.publish(id.into(), rejected).await.unwrap_err().code,
            code
        );
        assert_eq!(state.closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.drops.load(Ordering::SeqCst), 1);
    }
    assert_eq!(
        client.snapshot().local_sources,
        vec![SourceDescriptor {
            id: "camera-first".into(),
            kind: SourceKind::Camera
        }]
    );
    assert_eq!(first_state.closes.load(Ordering::SeqCst), 0);
    client.destroy().await.unwrap();
    assert_eq!(first_state.closes.load(Ordering::SeqCst), 1);
    let (late, state) = source(SourceKind::Camera, 3);
    assert_eq!(
        client.publish("late".into(), late).await.unwrap_err().code,
        "destroyed"
    );
    assert_eq!(state.closes.load(Ordering::SeqCst), 1);
    assert_eq!(state.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn publications_wait_for_negotiated_slots_and_attach_to_later_peers_without_new_sdp() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    let (camera, state) = source(SourceKind::Camera, 10);
    client.publish("camera".into(), camera).await.unwrap();
    assert!(h.bindings.lock().unwrap().is_empty());
    negotiate(&h, "b").await;
    assert_eq!(
        h.bindings
            .lock()
            .unwrap()
            .get(&("b".into(), SourceKind::Camera)),
        Some(&10)
    );
    h.signaling(SignalingEvent::PeerJoined("c".into()));
    until(|| client.snapshot().peers.len() == 2).await;
    negotiate(&h, "c").await;
    assert_eq!(
        h.bindings
            .lock()
            .unwrap()
            .get(&("c".into(), SourceKind::Camera)),
        Some(&10)
    );
    let descriptions = || {
        h.sent_signals
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, payload)| matches!(payload, SignalPayload::Description { .. }))
            .count()
    };
    assert_eq!(descriptions(), 2);
    client.unpublish("camera".into()).await.unwrap();
    assert!(h.bindings.lock().unwrap().is_empty());
    assert_eq!(state.closes.load(Ordering::SeqCst), 1);
    let (replacement, replacement_state) = source(SourceKind::Camera, 20);
    client
        .publish("camera-next".into(), replacement)
        .await
        .unwrap();
    assert_eq!(descriptions(), 2);
    assert!(h
        .bindings
        .lock()
        .unwrap()
        .values()
        .all(|token| *token == 20));
    assert!(h.sent_signals.lock().unwrap().iter().any(|(_, payload)| matches!(payload, SignalPayload::Sources { sources, .. } if sources == &vec![camera_binding("camera-next")])));
    client.leave().await.unwrap();
    assert_eq!(replacement_state.closes.load(Ordering::SeqCst), 1);
    assert!(client.snapshot().local_sources.is_empty());
    assert!(client.snapshot().remote_sources.is_empty());
    client.join(options()).await.unwrap();
    assert!(client.snapshot().local_sources.is_empty());
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn source_binding_failures_close_only_affected_peers_without_hidden_transmission() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into(), "c".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    negotiate(&h, "b").await;
    negotiate(&h, "c").await;
    h.binding_failures
        .lock()
        .unwrap()
        .insert(("b".into(), true));
    let (camera, state) = source(SourceKind::Camera, 10);
    client.publish("camera".into(), camera).await.unwrap();
    assert_eq!(client.snapshot().peers, ["c"]);
    assert_eq!(h.bindings.lock().unwrap().len(), 1);
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 1);
    h.binding_failures
        .lock()
        .unwrap()
        .insert(("c".into(), false));
    client.unpublish("camera".into()).await.unwrap();
    assert!(client.snapshot().peers.is_empty());
    assert!(h.bindings.lock().unwrap().is_empty());
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 2);
    assert_eq!(state.closes.load(Ordering::SeqCst), 1);
    assert!(client
        .read_batch()
        .await
        .unwrap()
        .events
        .iter()
        .any(|event| matches!(event, Event::Error { error } if error.code == "binding-failed")));
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn unpublish_detaches_before_awaiting_source_close_and_does_not_stop_receivers() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    negotiate(&h, "b").await;
    let (camera, state) = source(SourceKind::Camera, 10);
    client.publish("camera".into(), camera).await.unwrap();
    state.close_pending.store(true, Ordering::SeqCst);
    let c = client.clone();
    let closing = tokio::spawn(async move { c.unpublish("camera".into()).await });
    until(|| state.closes.load(Ordering::SeqCst) == 1).await;
    assert!(!closing.is_finished());
    assert!(h.bindings.lock().unwrap().is_empty());
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 0);
    state.close_release.notify_one();
    closing.await.unwrap().unwrap();
    client.unpublish("camera".into()).await.unwrap();
    assert_eq!(state.closes.load(Ordering::SeqCst), 1);
    assert_eq!(state.drops.load(Ordering::SeqCst), 1);
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn source_manifests_are_revisioned_bounded_and_bind_to_actual_negotiated_slots() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    manifest(&h, "b", 2, vec![camera_binding("first")]);
    h.engine("b", EngineEvent::Ready(true));
    until(|| client.snapshot().ready_peers == ["b"]).await;
    assert!(
        client.snapshot().remote_sources.is_empty(),
        "Metadata cannot expose an unnegotiated receiver"
    );
    negotiate(&h, "b").await;
    until(|| client.snapshot().remote_sources.len() == 1).await;
    assert_eq!(client.snapshot().remote_sources[0].id, "first");
    manifest(&h, "b", 4, vec![camera_binding("replacement")]);
    until(|| client.snapshot().remote_sources[0].id == "replacement").await;
    manifest(&h, "b", 3, vec![camera_binding("obsolete")]);
    h.engine("b", EngineEvent::Ready(false));
    until(|| client.snapshot().ready_peers.is_empty()).await;
    assert_eq!(client.snapshot().remote_sources[0].id, "replacement");
    assert_eq!(client.snapshot().remote_sources[0].mid, "camera-mid");
    manifest(&h, "b", 5, vec![]);
    until(|| client.snapshot().remote_sources.is_empty()).await;
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 0);
    manifest(
        &h,
        "b",
        6,
        vec![SourceBinding {
            id: "wrong-slot".into(),
            kind: SourceKind::Screen,
            mid: "camera-mid".into(),
        }],
    );
    until(|| client.snapshot().peers.is_empty()).await;
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn duplicate_manifest_identifiers_and_equal_revision_changes_are_rejected() {
    for sources in [
        vec![
            camera_binding("duplicate"),
            SourceBinding {
                id: "duplicate".into(),
                kind: SourceKind::Screen,
                mid: "screen-mid".into(),
            },
        ],
        vec![camera_binding("first"), camera_binding("second")],
        vec![SourceBinding {
            id: "".into(),
            kind: SourceKind::Camera,
            mid: "camera-mid".into(),
        }],
    ] {
        let h = Arc::new(Harness::default());
        *h.initial_peers.lock().unwrap() = vec!["b".into()];
        let client = h.client();
        client.join(options()).await.unwrap();
        negotiate(&h, "b").await;
        manifest(&h, "b", 1, sources);
        until(|| client.snapshot().peers.is_empty()).await;
        assert!(client.snapshot().remote_sources.is_empty());
        client.destroy().await.unwrap();
    }
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    negotiate(&h, "b").await;
    manifest(&h, "b", 1, vec![camera_binding("first")]);
    until(|| client.snapshot().remote_sources.len() == 1).await;
    manifest(&h, "b", 1, vec![camera_binding("second")]);
    until(|| client.snapshot().peers.is_empty()).await;
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn queued_and_rejected_publications_are_closed_before_destroy_acknowledgement() {
    let h = Arc::new(Harness::default());
    h.offer_pending.store(true, Ordering::SeqCst);
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    let c = client.clone();
    let joining = tokio::spawn(async move { c.join(options()).await });
    until(|| !h.peer_sinks.lock().unwrap().is_empty()).await;
    let mut publications = Vec::new();
    let mut states = Vec::new();
    for index in 0..64 {
        let (source, state) = source(SourceKind::Camera, index);
        states.push(state);
        let c = client.clone();
        publications.push(tokio::spawn(async move {
            c.publish(format!("queued-{index}"), source).await
        }));
    }
    until(|| {
        states
            .iter()
            .map(|state| state.closes.load(Ordering::SeqCst))
            .sum::<usize>()
            >= 32
    })
    .await;
    timeout(Duration::from_secs(1), client.destroy())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(joining.await.unwrap().unwrap_err().code, "cancelled");
    for state in &states {
        assert_eq!(state.closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.drops.load(Ordering::SeqCst), 1);
    }
    for publication in publications {
        assert!(publication.await.unwrap().is_err());
    }
    assert!(client.snapshot().local_sources.is_empty());
}

#[tokio::test]
async fn leave_waits_for_abandoned_publication_cleanup_even_after_its_caller_is_cancelled() {
    let h = Arc::new(Harness::default());
    h.offer_pending.store(true, Ordering::SeqCst);
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    let c = client.clone();
    let joining = tokio::spawn(async move { c.join(options()).await });
    until(|| !h.peer_sinks.lock().unwrap().is_empty()).await;
    let (source, state) = source(SourceKind::Camera, 1);
    state.close_pending.store(true, Ordering::SeqCst);
    let mut publishing = Box::pin(client.publish("abandoned".into(), source));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
            publishing.as_mut(),
            cx
        )))
        .await
        .is_pending()
    );
    drop(publishing);
    let c = client.clone();
    let leaving = tokio::spawn(async move { c.leave().await });
    until(|| state.closes.load(Ordering::SeqCst) == 1).await;
    assert!(!leaving.is_finished());
    state.close_release.notify_one();
    leaving.await.unwrap().unwrap();
    assert_eq!(joining.await.unwrap().unwrap_err().code, "cancelled");
    assert_eq!(state.drops.load(Ordering::SeqCst), 1);
    assert_eq!(client.snapshot().state, State::Idle);
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn destruction_closes_every_source_even_after_one_source_reports_failure() {
    let h = Arc::new(Harness::default());
    let client = h.client();
    client.join(options()).await.unwrap();
    let mut states = Vec::new();
    for (index, kind) in [
        SourceKind::Camera,
        SourceKind::Screen,
        SourceKind::Microphone,
    ]
    .into_iter()
    .enumerate()
    {
        let (source, state) = source(kind, index);
        state.close_failure.store(index == 0, Ordering::SeqCst);
        client
            .publish(format!("source-{index}"), source)
            .await
            .unwrap();
        states.push(state);
    }
    assert_eq!(client.destroy().await.unwrap_err().code, "source-cleanup");
    assert_eq!(client.snapshot().state, State::Destroyed);
    assert!(client.snapshot().local_sources.is_empty());
    for state in states {
        assert_eq!(state.closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.drops.load(Ordering::SeqCst), 1);
        assert!(state.stopped.load(Ordering::SeqCst));
    }
    assert_eq!(client.destroy().await.unwrap_err().code, "source-cleanup");
}

#[tokio::test]
async fn native_receive_observations_are_scoped_to_peers_and_pending_reads_are_interruptible() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into()];
    let client = h.client();
    assert_eq!(
        client.media_stats().await.unwrap_err().code,
        "invalid-state"
    );
    client.join(options()).await.unwrap();
    assert!(client.media_stats().await.unwrap().is_empty());
    negotiate(&h, "b").await;
    let observed = client.media_stats().await.unwrap();
    assert_eq!(observed.len(), 1);
    assert_eq!(observed[0].peer_id, "b");
    assert_eq!(observed[0].media[0].frames_decoded, 7);
    h.stats_pending.store(true, Ordering::SeqCst);
    let c = client.clone();
    let pending = tokio::spawn(async move { c.media_stats().await });
    until(|| h.stats_calls.load(Ordering::SeqCst) == 2).await;
    timeout(Duration::from_secs(1), client.leave())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.await.unwrap().unwrap_err().code, "cancelled");
    client.destroy().await.unwrap();
}

#[tokio::test]
async fn cancelled_connect_preserves_queued_source_cleanup_failure_for_stop_acknowledgement() {
    for destroy in [false, true] {
        let h = Arc::new(Harness::default());
        h.connect_pending.store(true, Ordering::SeqCst);
        let client = h.client();
        let c = client.clone();
        let joining = tokio::spawn(async move { c.join(options()).await });
        until(|| h.connects.load(Ordering::SeqCst) == 1).await;
        let (source, state) = source(SourceKind::Camera, 1);
        state.close_failure.store(true, Ordering::SeqCst);
        let mut publishing = Box::pin(client.publish("queued".into(), source));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
                publishing.as_mut(),
                cx
            )))
            .await
            .is_pending()
        );
        let stopped = if destroy {
            client.destroy().await
        } else {
            client.leave().await
        };
        assert_eq!(stopped.unwrap_err().code, "source-cleanup");
        assert_eq!(publishing.await.unwrap_err().code, "source-cleanup");
        assert_eq!(joining.await.unwrap().unwrap_err().code, "cancelled");
        assert_eq!(state.closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.drops.load(Ordering::SeqCst), 1);
        if destroy {
            assert_eq!(client.snapshot().state, State::Destroyed);
            assert_eq!(client.destroy().await.unwrap_err().code, "source-cleanup");
        } else {
            h.connect_pending.store(false, Ordering::SeqCst);
            client.join(options()).await.unwrap();
            client.leave().await.unwrap();
            client.destroy().await.unwrap();
        }
    }
}

#[tokio::test]
async fn overlapping_destroy_preserves_failure_from_already_running_leave_cleanup() {
    let h = Arc::new(Harness::default());
    let client = h.client();
    client.join(options()).await.unwrap();
    let (source, state) = source(SourceKind::Camera, 1);
    state.close_pending.store(true, Ordering::SeqCst);
    state.close_failure.store(true, Ordering::SeqCst);
    client.publish("camera".into(), source).await.unwrap();
    let c = client.clone();
    let leaving = tokio::spawn(async move { c.leave().await });
    until(|| state.closes.load(Ordering::SeqCst) == 1).await;
    let mut destroying = Box::pin(client.destroy());
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
            destroying.as_mut(),
            cx
        )))
        .await
        .is_pending()
    );
    state.close_release.notify_one();
    assert_eq!(leaving.await.unwrap().unwrap_err().code, "source-cleanup");
    assert_eq!(destroying.await.unwrap_err().code, "source-cleanup");
    assert_eq!(client.snapshot().state, State::Destroyed);
    assert_eq!(state.closes.load(Ordering::SeqCst), 1);
    assert_eq!(state.drops.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn destroy_preserves_failure_from_source_cleanup_already_owned_by_another_command() {
    for unpublish in [false, true] {
        let h = Arc::new(Harness::default());
        let client = h.client();
        client.join(options()).await.unwrap();
        let (source, state) = source(SourceKind::Camera, 1);
        state.close_pending.store(true, Ordering::SeqCst);
        state.close_failure.store(true, Ordering::SeqCst);
        let c = client.clone();
        let closing = if unpublish {
            client.publish("camera".into(), source).await.unwrap();
            tokio::spawn(async move { c.unpublish("camera".into()).await })
        } else {
            tokio::spawn(async move { c.publish("".into(), source).await })
        };
        until(|| state.closes.load(Ordering::SeqCst) == 1).await;
        let mut destroying = Box::pin(client.destroy());
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(
                destroying.as_mut(),
                cx
            )))
            .await
            .is_pending()
        );
        state.close_release.notify_one();
        assert_eq!(closing.await.unwrap().unwrap_err().code, "source-cleanup");
        assert_eq!(destroying.await.unwrap_err().code, "source-cleanup");
        assert_eq!(client.snapshot().state, State::Destroyed);
        assert_eq!(state.closes.load(Ordering::SeqCst), 1);
        assert_eq!(state.drops.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn invalid_media_signal_closes_only_known_sender_and_preserves_healthy_publications() {
    let h = Arc::new(Harness::default());
    *h.initial_peers.lock().unwrap() = vec!["b".into(), "c".into()];
    let client = h.client();
    client.join(options()).await.unwrap();
    negotiate(&h, "b").await;
    negotiate(&h, "c").await;
    let (source, state) = source(SourceKind::Camera, 1);
    client.publish("local".into(), source).await.unwrap();
    manifest(&h, "b", 1, vec![camera_binding("bad-peer-camera")]);
    manifest(&h, "c", 1, vec![camera_binding("healthy-camera")]);
    until(|| client.snapshot().remote_sources.len() == 2).await;
    h.signaling(SignalingEvent::InvalidSignal { from: "b".into() });
    h.signaling(SignalingEvent::InvalidSignal {
        from: "unknown".into(),
    });
    h.engine("c", EngineEvent::Ready(true));
    until(|| client.snapshot().ready_peers == ["c"]).await;
    let snapshot = client.snapshot();
    assert_eq!(snapshot.state, State::Joined);
    assert_eq!(snapshot.peers, ["c"]);
    assert_eq!(snapshot.remote_sources.len(), 1);
    assert_eq!(snapshot.remote_sources[0].id, "healthy-camera");
    assert_eq!(snapshot.local_sources.len(), 1);
    assert_eq!(h.bindings.lock().unwrap().len(), 1);
    assert_eq!(h.peer_closes.load(Ordering::SeqCst), 1);
    assert_eq!(state.closes.load(Ordering::SeqCst), 0);
    assert_eq!(
        client
            .send("still connected".into())
            .await
            .unwrap()
            .accepted_peer_ids,
        ["c"]
    );
    let errors = client.read_batch().await.unwrap().events;
    assert_eq!(errors.len(), 1);
    assert!(matches!(&errors[0], Event::Error { error } if error.code == "invalid-signal"));
    client.destroy().await.unwrap();
}
