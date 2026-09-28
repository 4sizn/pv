use pv_media_runtime::{ports::*, *};
use std::{
    collections::BTreeMap,
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
}
struct FakeSignaling(Arc<Harness>);
struct Connection(Arc<Harness>);
struct FakeEngine(Arc<Harness>);
struct FakePeer {
    harness: Arc<Harness>,
    closed: bool,
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
    fn send(&mut self, _: &str, _: SignalPayload) -> Result<(), NativeError> {
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
            })
        })
    }
    fn answer(&mut self, _: Description) -> PortFuture<'_, Description> {
        Box::pin(async {
            Ok(Description {
                r#type: DescriptionType::Answer,
                sdp: "test-answer".into(),
            })
        })
    }
    fn accept_answer(&mut self, _: Description) -> PortFuture<'_, ()> {
        Box::pin(async { Ok(()) })
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
    fn close(&mut self) {
        if !self.closed {
            self.closed = true;
            self.harness.peer_closes.fetch_add(1, Ordering::SeqCst);
        }
    }
}
impl Harness {
    fn client(self: &Arc<Self>) -> NativeDataClient {
        NativeDataClient::new(
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
