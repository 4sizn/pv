use crate::{ports::*, *};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex, Notify};

#[derive(Clone, Copy, Default)]
struct Control {
    sequence: u64,
    destroyed: bool,
}
#[derive(Clone, Default)]
struct ControlAck {
    sequence: u64,
    error: Option<NativeError>,
}
enum Command {
    Join(JoinOptions, oneshot::Sender<Result<Snapshot, NativeError>>),
    Send(String, oneshot::Sender<Result<SendResult, NativeError>>),
}
struct QueuedCommand {
    sequence: u64,
    command: Command,
}
struct Shared {
    snapshot: watch::Sender<Snapshot>,
    events: Mutex<VecDeque<Event>>,
    notify: Notify,
    reader: AsyncMutex<u64>,
    cleanup_error: Mutex<Option<NativeError>>,
}
struct Handle {
    commands: mpsc::Sender<QueuedCommand>,
    control: watch::Sender<Control>,
    acknowledged: watch::Receiver<ControlAck>,
    shared: Arc<Shared>,
}
impl Drop for Handle {
    fn drop(&mut self) {
        self.control.send_modify(|c| {
            c.sequence += 1;
            c.destroyed = true;
        });
    }
}
/// A cloneable command handle. Only the actor owns native session state.
#[derive(Clone)]
pub struct NativeDataClient {
    handle: Arc<Handle>,
}
impl NativeDataClient {
    pub fn new(
        engine: Arc<dyn EngineFactory>,
        signaling: Arc<dyn SignalingFactory>,
    ) -> Result<Self, NativeError> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| NativeError::new("runtime", "A Tokio runtime is required"))?;
        let (commands, command_rx) = mpsc::channel(32);
        let (control, control_rx) = watch::channel(Control::default());
        let (acknowledged, acknowledged_rx) = watch::channel(ControlAck::default());
        let (snapshot, _) = watch::channel(Snapshot::default());
        let (inputs, input_rx) = mpsc::channel(INPUT_CAPACITY);
        let (overflow, overflow_rx) = watch::channel(0);
        let shared = Arc::new(Shared {
            snapshot,
            events: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
            reader: AsyncMutex::new(u64::MAX),
            cleanup_error: Mutex::new(None),
        });
        let actor = Actor {
            engine: Some(engine),
            signaling_factory: signaling,
            shared: shared.clone(),
            commands: command_rx,
            control: control_rx,
            acknowledged,
            processed_control: 0,
            inputs,
            input_rx,
            overflow,
            overflow_rx,
            generation: 0,
            next_peer_instance: 0,
            signaling: None,
            peers: BTreeMap::new(),
            ice: vec![],
        };
        runtime.spawn(actor.run());
        Ok(Self {
            handle: Arc::new(Handle {
                commands,
                control,
                acknowledged: acknowledged_rx,
                shared,
            }),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        self.handle.shared.snapshot.borrow().clone()
    }
    pub async fn join(&self, options: JoinOptions) -> Result<Snapshot, NativeError> {
        validate_options(&options)?;
        let (tx, rx) = oneshot::channel();
        self.enqueue(Command::Join(options, tx))?;
        rx.await.unwrap_or_else(|_| Err(NativeError::destroyed()))
    }
    pub async fn send(&self, data: String) -> Result<SendResult, NativeError> {
        if data.len() > MAX_DATA_BYTES {
            return Err(NativeError::new(
                "message-too-large",
                "Data exceeds the 16384 byte limit",
            ));
        }
        let (tx, rx) = oneshot::channel();
        self.enqueue(Command::Send(data, tx))?;
        rx.await.unwrap_or_else(|_| Err(NativeError::destroyed()))
    }
    fn enqueue(&self, command: Command) -> Result<(), NativeError> {
        let control = *self.handle.control.borrow();
        if control.destroyed || self.snapshot().state == State::Destroyed {
            return Err(NativeError::destroyed());
        }
        self.handle
            .commands
            .try_send(QueuedCommand {
                sequence: control.sequence,
                command,
            })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => {
                    NativeError::new("busy", "The command queue is full")
                }
                mpsc::error::TrySendError::Closed(_) => NativeError::destroyed(),
            })
    }
    pub async fn leave(&self) -> Result<Snapshot, NativeError> {
        self.stop(false).await
    }
    pub async fn destroy(&self) -> Result<Snapshot, NativeError> {
        self.stop(true).await
    }
    async fn stop(&self, destroy: bool) -> Result<Snapshot, NativeError> {
        if self.snapshot().state == State::Destroyed {
            return self.cleanup_snapshot();
        }
        let mut requested = 0;
        self.handle.control.send_modify(|c| {
            c.sequence += 1;
            c.destroyed |= destroy;
            requested = c.sequence;
        });
        let mut ack = self.handle.acknowledged.clone();
        loop {
            let acknowledged = ack.borrow_and_update().clone();
            if acknowledged.sequence >= requested {
                return acknowledged.error.map_or_else(|| Ok(self.snapshot()), Err);
            }
            if self.snapshot().state == State::Destroyed {
                return self.cleanup_snapshot();
            }
            if ack.changed().await.is_err() {
                return Err(NativeError::destroyed());
            }
        }
    }
    fn cleanup_snapshot(&self) -> Result<Snapshot, NativeError> {
        self.handle
            .shared
            .cleanup_error
            .lock()
            .expect("cleanup result lock")
            .clone()
            .map_or_else(|| Ok(self.snapshot()), Err)
    }
    /// One pending read per client, including clones. Cancellation releases the
    /// read guard without consuming events. Snapshots and batches are bounded.
    pub async fn read_batch(&self) -> Result<EventBatch, NativeError> {
        let shared = &self.handle.shared;
        let mut last_revision = shared.reader.try_lock().map_err(|_| {
            NativeError::new("read-in-progress", "An event read is already pending")
        })?;
        loop {
            let notified = shared.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let (snapshot, events) = {
                let mut queue = shared.events.lock().expect("event queue lock");
                let snapshot = self.snapshot();
                let count = EVENT_BATCH_LIMIT.min(queue.len());
                (snapshot, queue.drain(..count).collect::<Vec<_>>())
            };
            if snapshot.revision != *last_revision
                || !events.is_empty()
                || snapshot.state == State::Destroyed
            {
                *last_revision = snapshot.revision;
                return Ok(EventBatch { snapshot, events });
            }
            notified.await;
        }
    }
}

struct Peer {
    instance: u64,
    port: Box<dyn PeerPort>,
    initiator: bool,
    remote_description: bool,
    early_ice: Vec<Candidate>,
    ready: bool,
}
struct Actor {
    engine: Option<Arc<dyn EngineFactory>>,
    signaling_factory: Arc<dyn SignalingFactory>,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<QueuedCommand>,
    control: watch::Receiver<Control>,
    acknowledged: watch::Sender<ControlAck>,
    processed_control: u64,
    inputs: mpsc::Sender<StampedInput>,
    input_rx: mpsc::Receiver<StampedInput>,
    overflow: watch::Sender<u64>,
    overflow_rx: watch::Receiver<u64>,
    generation: u64,
    next_peer_instance: u64,
    signaling: Option<Box<dyn SignalingPort>>,
    peers: BTreeMap<String, Peer>,
    ice: Vec<IceServer>,
}
impl Actor {
    async fn run(mut self) {
        loop {
            let requested = *self.control.borrow_and_update();
            if requested.sequence != self.processed_control {
                let result = self.teardown(requested.destroyed).await;
                self.processed_control = requested.sequence;
                self.acknowledged.send_replace(ControlAck {
                    sequence: requested.sequence,
                    error: result.err(),
                });
                if requested.destroyed {
                    break;
                }
            }
            if *self.overflow_rx.borrow_and_update() == self.generation && self.generation != 0 {
                self.emit_error(NativeError::new(
                    "event-overflow",
                    "Native event queue overflowed; session closed",
                ));
                let _ = self.teardown(false).await;
            }
            tokio::select! {
                biased;
                result = self.control.changed() => { if result.is_err() { let _ = self.teardown(true).await; break; } }
                _ = self.overflow_rx.changed() => {}
                command = self.commands.recv() => {
                    let Some(command) = command else { let _ = self.teardown(true).await; break; };
                    if command.sequence != self.processed_control { reject(command.command, NativeError::cancelled()); continue; }
                    match command.command {
                        Command::Join(options, reply) => { let result = self.join(options).await; let _ = reply.send(result); }
                        Command::Send(data, reply) => { let _ = reply.send(self.send(data)); }
                    }
                }
                input = self.input_rx.recv() => {
                    if let Some(input) = input {
                        let current_peer = match &input.input {
                            Input::Engine { peer_id, .. } => self.peers.get(peer_id).is_some_and(|peer| peer.instance == input.peer_instance),
                            Input::Signaling(_) => true,
                        };
                        if current_peer && input.generation == self.generation && self.signaling.is_some() { self.handle_input(input.input).await; }
                    }
                }
            }
        }
        self.commands.close();
        while let Some(command) = self.commands.recv().await {
            reject(command.command, NativeError::destroyed());
        }
    }
    fn sink(&self) -> EventSink {
        EventSink::new(self.generation, self.inputs.clone(), self.overflow.clone())
    }
    fn update(&self, state: State, peer_id: Option<String>) {
        let _events = self.shared.events.lock().expect("event queue lock");
        let peers = self.peers.keys().cloned().collect();
        let ready_peers = self
            .peers
            .iter()
            .filter(|(_, peer)| peer.ready)
            .map(|(id, _)| id.clone())
            .collect();
        let previous = self.shared.snapshot.borrow().clone();
        if previous.state == state
            && previous.peer_id == peer_id
            && previous.peers == peers
            && previous.ready_peers == ready_peers
        {
            return;
        }
        self.shared.snapshot.send_replace(Snapshot {
            revision: previous.revision + 1,
            state,
            peer_id,
            peers,
            ready_peers,
        });
        self.shared.notify.notify_waiters();
    }
    fn update_peers(&self) {
        let snapshot = self.shared.snapshot.borrow().clone();
        self.update(snapshot.state, snapshot.peer_id);
    }
    fn emit(&self, event: Event) -> bool {
        let mut events = self.shared.events.lock().expect("event queue lock");
        let available = events.len() < PUBLIC_EVENT_CAPACITY;
        if available {
            events.push_back(event);
        } else {
            self.overflow
                .send_modify(|value| *value = (*value).max(self.generation));
            events.clear();
            events.push_back(Event::Error {
                error: NativeError::new(
                    "event-overflow",
                    "Consumer event queue overflowed; session closed",
                ),
            });
        }
        drop(events);
        self.shared.notify.notify_waiters();
        available
    }
    fn emit_error(&self, error: NativeError) {
        self.emit(Event::Error { error });
    }
    async fn join(&mut self, options: JoinOptions) -> Result<Snapshot, NativeError> {
        if self.shared.snapshot.borrow().state != State::Idle {
            return Err(NativeError::new(
                "invalid-state",
                "Leave the current session before joining",
            ));
        }
        self.generation += 1;
        self.update(State::Joining, None);
        self.ice = options.ice_servers.clone();
        let factory = self.signaling_factory.clone();
        let result = interruptible(
            factory.connect(options, self.sink()),
            &mut self.control,
            &mut self.overflow_rx,
            self.generation,
            Duration::from_secs(10),
        )
        .await;
        let connected = match result {
            Ok(value) => value,
            Err(error) => {
                let _ = self.teardown(false).await;
                return Err(error);
            }
        };
        self.signaling = Some(connected.signaling);
        if !valid_membership(&connected.peer_id, &connected.peers) {
            let _ = self.teardown(false).await;
            return Err(NativeError::new(
                "invalid-membership",
                "Signaling returned invalid membership",
            ));
        }
        self.update(State::Joined, Some(connected.peer_id));
        for peer_id in connected.peers {
            if let Err(error) = self.add_peer(peer_id.clone()).await {
                if error.code == "cancelled" {
                    return Err(error);
                }
                self.remove_peer(&peer_id);
                self.emit_error(error);
            }
        }
        Ok(self.shared.snapshot.borrow().clone())
    }
    async fn add_peer(&mut self, peer_id: String) -> Result<(), NativeError> {
        let local_id = self
            .shared
            .snapshot
            .borrow()
            .peer_id
            .clone()
            .ok_or_else(|| NativeError::new("invalid-state", "No active membership"))?;
        if self.peers.contains_key(&peer_id) {
            return Ok(());
        }
        if !valid_peer_id(&peer_id) || peer_id == local_id || self.peers.len() >= 3 {
            return Err(NativeError::new(
                "invalid-membership",
                "Peer membership exceeds session limits",
            ));
        }
        let initiator = local_id < peer_id;
        self.next_peer_instance += 1;
        let port = self
            .engine
            .as_ref()
            .ok_or_else(NativeError::destroyed)?
            .create(
                &peer_id,
                initiator,
                &self.ice,
                self.sink().for_peer(self.next_peer_instance),
            )?;
        self.peers.insert(
            peer_id.clone(),
            Peer {
                instance: self.next_peer_instance,
                port,
                initiator,
                remote_description: false,
                early_ice: vec![],
                ready: false,
            },
        );
        self.update_peers();
        if initiator {
            let peer = self.peers.get_mut(&peer_id).expect("inserted peer");
            let description = interruptible(
                peer.port.offer(),
                &mut self.control,
                &mut self.overflow_rx,
                self.generation,
                Duration::from_secs(5),
            )
            .await?;
            self.signal(&peer_id, SignalPayload::Description { description })?;
        }
        Ok(())
    }
    fn signal(&mut self, peer_id: &str, payload: SignalPayload) -> Result<(), NativeError> {
        self.signaling
            .as_mut()
            .ok_or_else(|| NativeError::new("disconnected", "Signaling is closed"))?
            .send(peer_id, payload)
    }
    async fn handle_input(&mut self, input: Input) {
        match input {
            Input::Signaling(SignalingEvent::PeerJoined(peer_id)) => {
                if let Err(error) = self.add_peer(peer_id.clone()).await {
                    self.remove_peer(&peer_id);
                    if error.code != "cancelled" {
                        self.emit_error(error);
                    }
                }
            }
            Input::Signaling(SignalingEvent::PeerLeft(peer_id)) => self.remove_peer(&peer_id),
            Input::Signaling(SignalingEvent::Signal { from, payload }) => {
                if !self.peers.contains_key(&from) {
                    return;
                }
                if let Err(error) = self.receive_signal(&from, payload).await {
                    self.remove_peer(&from);
                    if error.code != "cancelled" {
                        self.emit_error(error);
                    }
                }
            }
            Input::Signaling(SignalingEvent::Closed) => {
                self.emit_error(NativeError::new("disconnected", "Signaling disconnected"));
                let _ = self.teardown(false).await;
            }
            Input::Signaling(SignalingEvent::Error(error)) => {
                let peer_departed = error.code == "peer-unavailable";
                self.emit_error(error);
                if !peer_departed {
                    let _ = self.teardown(false).await;
                }
            }
            Input::Engine { peer_id, event } => {
                if !self.peers.contains_key(&peer_id) {
                    return;
                }
                match event {
                    EngineEvent::Ready(ready) => {
                        self.peers.get_mut(&peer_id).expect("known peer").ready = ready;
                        self.update_peers();
                    }
                    EngineEvent::Candidate(candidate) => {
                        if let Err(error) = self.signal(&peer_id, SignalPayload::Ice { candidate })
                        {
                            self.emit_error(error);
                            let _ = self.teardown(false).await;
                        }
                    }
                    EngineEvent::Message(data) => {
                        if data.len() > MAX_DATA_BYTES {
                            self.remove_peer(&peer_id);
                            self.emit_error(NativeError::new(
                                "message-too-large",
                                "Received data exceeds the byte limit",
                            ));
                        } else if !self.emit(Event::Message { peer_id, data }) {
                            let _ = self.teardown(false).await;
                        }
                    }
                    EngineEvent::Failed => {
                        self.remove_peer(&peer_id);
                        self.emit_error(NativeError::new(
                            "peer-failed",
                            "A native peer connection failed",
                        ));
                    }
                    EngineEvent::Error(error) => {
                        self.remove_peer(&peer_id);
                        self.emit_error(error);
                    }
                }
            }
        }
    }
    async fn receive_signal(
        &mut self,
        peer_id: &str,
        payload: SignalPayload,
    ) -> Result<(), NativeError> {
        let peer = self.peers.get_mut(peer_id).expect("known peer");
        match payload {
            SignalPayload::Description { description } => {
                if description.sdp.len() > 60_000 || peer.remote_description {
                    return Err(NativeError::new(
                        "invalid-description",
                        "Unexpected session description",
                    ));
                }
                let answer = match (&description.r#type, peer.initiator) {
                    (DescriptionType::Offer, false) => Some(
                        interruptible(
                            peer.port.answer(description),
                            &mut self.control,
                            &mut self.overflow_rx,
                            self.generation,
                            Duration::from_secs(5),
                        )
                        .await?,
                    ),
                    (DescriptionType::Answer, true) => {
                        interruptible(
                            peer.port.accept_answer(description),
                            &mut self.control,
                            &mut self.overflow_rx,
                            self.generation,
                            Duration::from_secs(5),
                        )
                        .await?;
                        None
                    }
                    _ => {
                        return Err(NativeError::new(
                            "invalid-description",
                            "Unexpected session description direction",
                        ))
                    }
                };
                peer.remote_description = true;
                for candidate in std::mem::take(&mut peer.early_ice) {
                    interruptible(
                        peer.port.add_candidate(candidate),
                        &mut self.control,
                        &mut self.overflow_rx,
                        self.generation,
                        Duration::from_secs(5),
                    )
                    .await?;
                }
                if let Some(description) = answer {
                    self.signal(peer_id, SignalPayload::Description { description })?;
                }
            }
            SignalPayload::Ice { candidate } => {
                if candidate.candidate.is_empty() {
                    return Ok(());
                }
                if candidate.candidate.len() > 4_096
                    || candidate
                        .sdp_mid
                        .as_ref()
                        .is_some_and(|mid| mid.len() > 128)
                {
                    return Err(NativeError::new(
                        "invalid-candidate",
                        "ICE candidate exceeds limits",
                    ));
                }
                if peer.remote_description {
                    interruptible(
                        peer.port.add_candidate(candidate),
                        &mut self.control,
                        &mut self.overflow_rx,
                        self.generation,
                        Duration::from_secs(5),
                    )
                    .await?;
                } else if peer.early_ice.len() < 64 {
                    peer.early_ice.push(candidate);
                } else {
                    return Err(NativeError::new(
                        "ice-overflow",
                        "Early ICE candidate queue is full",
                    ));
                }
            }
            SignalPayload::Track { .. } | SignalPayload::TrackRemoved { .. } => {}
        }
        Ok(())
    }
    fn send(&mut self, data: String) -> Result<SendResult, NativeError> {
        if self.shared.snapshot.borrow().state != State::Joined {
            return Err(NativeError::new(
                "invalid-state",
                "Join a session before sending",
            ));
        }
        let mut result = SendResult::default();
        for (peer_id, peer) in &mut self.peers {
            match peer.port.send(&data) {
                Ok(()) => result.accepted_peer_ids.push(peer_id.clone()),
                Err(error) => result.failures.push(SendFailure {
                    peer_id: peer_id.clone(),
                    message: error.message,
                }),
            }
        }
        Ok(result)
    }
    fn remove_peer(&mut self, peer_id: &str) {
        if let Some(mut peer) = self.peers.remove(peer_id) {
            peer.port.close();
            self.update_peers();
        }
    }
    async fn teardown(&mut self, destroy: bool) -> Result<(), NativeError> {
        self.generation += 1;
        self.shared
            .events
            .lock()
            .expect("event queue lock")
            .retain(|event| matches!(event, Event::Error { .. }));
        if self.shared.snapshot.borrow().state != State::Idle
            && self.shared.snapshot.borrow().state != State::Destroyed
        {
            let id = self.shared.snapshot.borrow().peer_id.clone();
            self.update(State::Leaving, id);
        }
        for (_, mut peer) in std::mem::take(&mut self.peers) {
            peer.port.close();
        }
        let result = if let Some(mut signaling) = self.signaling.take() {
            tokio::time::timeout(Duration::from_secs(2), signaling.close())
                .await
                .unwrap_or_else(|_| {
                    Err(NativeError::new(
                        "cleanup-timeout",
                        "Signaling cleanup exceeded its deadline",
                    ))
                })
        } else {
            Ok(())
        };
        self.ice.clear();
        while self.input_rx.try_recv().is_ok() {}
        if destroy {
            self.engine.take();
        }
        *self
            .shared
            .cleanup_error
            .lock()
            .expect("cleanup result lock") = result.as_ref().err().cloned();
        if let Err(error) = &result {
            self.emit_error(error.clone());
        }
        self.update(
            if destroy {
                State::Destroyed
            } else {
                State::Idle
            },
            None,
        );
        result
    }
}

fn reject(command: Command, error: NativeError) {
    match command {
        Command::Join(_, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Send(_, reply) => {
            let _ = reply.send(Err(error));
        }
    }
}
async fn interruptible<T>(
    future: PortFuture<'_, T>,
    control: &mut watch::Receiver<Control>,
    overflow: &mut watch::Receiver<u64>,
    generation: u64,
    deadline: Duration,
) -> Result<T, NativeError> {
    let future = tokio::time::timeout(deadline, future);
    tokio::pin!(future);
    loop {
        tokio::select! {
            biased;
            _ = control.changed() => return Err(NativeError::cancelled()),
            _ = overflow.changed() => { if *overflow.borrow_and_update() == generation { return Err(NativeError::new("event-overflow", "Native event queue overflowed")); } }
            result = &mut future => return result.unwrap_or_else(|_| Err(NativeError::new("timeout", "Native operation timed out"))),
        }
    }
}
fn valid_peer_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && !id.chars().any(char::is_control)
}
fn valid_membership(local: &str, peers: &[String]) -> bool {
    valid_peer_id(local)
        && peers.len() <= 3
        && peers.iter().all(|id| valid_peer_id(id) && id != local)
        && peers
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == peers.len()
}
fn validate_options(options: &JoinOptions) -> Result<(), NativeError> {
    if [&options.room_id, &options.room_token, &options.device_token]
        .iter()
        .any(|value| value.is_empty() || value.len() > 4096)
        || options.signaling_url.len() > 4096
        || options.ice_servers.len() > 16
        || options.ice_servers.iter().any(|server| {
            server.urls.is_empty()
                || server.urls.len() > 8
                || server
                    .urls
                    .iter()
                    .any(|url| url.is_empty() || url.len() > 2048)
                || server.username.as_ref().is_some_and(|s| s.len() > 4096)
                || server.credential.as_ref().is_some_and(|s| s.len() > 4096)
        })
    {
        return Err(NativeError::new(
            "invalid-options",
            "Session options exceed limits or contain empty credentials",
        ));
    }
    Ok(())
}
