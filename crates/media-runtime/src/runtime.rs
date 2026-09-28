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
    Publish(
        String,
        Box<dyn SourcePort>,
        oneshot::Sender<Result<Snapshot, NativeError>>,
    ),
    Unpublish(String, oneshot::Sender<Result<Snapshot, NativeError>>),
    MediaStats(oneshot::Sender<Result<Vec<PeerMediaStats>, NativeError>>),
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
pub struct NativeMediaClient {
    handle: Arc<Handle>,
}
impl NativeMediaClient {
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
            pending_cleanup_error: None,
            inputs,
            input_rx,
            overflow,
            overflow_rx,
            generation: 0,
            next_peer_instance: 0,
            signaling: None,
            peers: BTreeMap::new(),
            publications: BTreeMap::new(),
            source_revision: 0,
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
    /// Transfers source ownership even when admission fails. Successful publication means
    /// local ownership; negotiated/current and later peers receive that source independently.
    /// Dropping the response future does not cancel an admitted command. Admission-rejected
    /// cleanup belongs to this future, outside actor stop acknowledgement; Drop still cancels.
    pub async fn publish(
        &self,
        id: String,
        source: Box<dyn SourcePort>,
    ) -> Result<Snapshot, NativeError> {
        let (tx, rx) = oneshot::channel();
        if let Err(rejected) = self.enqueue_owned(Command::Publish(id, source, tx)) {
            let (command, error) = *rejected;
            reject(command, error).await;
        }
        rx.await.unwrap_or_else(|_| Err(NativeError::destroyed()))
    }
    pub async fn unpublish(&self, id: String) -> Result<Snapshot, NativeError> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(Command::Unpublish(id, tx))?;
        rx.await.unwrap_or_else(|_| Err(NativeError::destroyed()))
    }
    /// Scalar receive diagnostics; raw frames remain in the native engine.
    pub async fn media_stats(&self) -> Result<Vec<PeerMediaStats>, NativeError> {
        let (tx, rx) = oneshot::channel();
        self.enqueue(Command::MediaStats(tx))?;
        rx.await.unwrap_or_else(|_| Err(NativeError::destroyed()))
    }
    fn enqueue(&self, command: Command) -> Result<(), NativeError> {
        self.enqueue_owned(command).map_err(|rejected| rejected.1)
    }
    fn enqueue_owned(&self, command: Command) -> Result<(), Box<(Command, NativeError)>> {
        let control = *self.handle.control.borrow();
        if control.destroyed || self.snapshot().state == State::Destroyed {
            return Err(Box::new((command, NativeError::destroyed())));
        }
        self.handle
            .commands
            .try_send(QueuedCommand {
                sequence: control.sequence,
                command,
            })
            .map_err(|error| {
                Box::new(match error {
                    mpsc::error::TrySendError::Full(queued) => (
                        queued.command,
                        NativeError::new("busy", "The command queue is full"),
                    ),
                    mpsc::error::TrySendError::Closed(queued) => {
                        (queued.command, NativeError::destroyed())
                    }
                })
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
    source_manifest: Option<(u64, Vec<SourceBinding>)>,
}
struct Actor {
    engine: Option<Arc<dyn EngineFactory>>,
    signaling_factory: Arc<dyn SignalingFactory>,
    shared: Arc<Shared>,
    commands: mpsc::Receiver<QueuedCommand>,
    control: watch::Receiver<Control>,
    acknowledged: watch::Sender<ControlAck>,
    processed_control: u64,
    pending_cleanup_error: Option<NativeError>,
    inputs: mpsc::Sender<StampedInput>,
    input_rx: mpsc::Receiver<StampedInput>,
    overflow: watch::Sender<u64>,
    overflow_rx: watch::Receiver<u64>,
    generation: u64,
    next_peer_instance: u64,
    signaling: Option<Box<dyn SignalingPort>>,
    peers: BTreeMap<String, Peer>,
    publications: BTreeMap<String, Box<dyn SourcePort>>,
    source_revision: u64,
    ice: Vec<IceServer>,
}
impl Actor {
    async fn run(mut self) {
        loop {
            let requested = *self.control.borrow_and_update();
            if requested.sequence != self.processed_control {
                let result = self.teardown(requested.destroyed).await;
                if self.control.borrow().sequence != requested.sequence {
                    self.pending_cleanup_error = result.as_ref().err().cloned();
                }
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
                self.end_session().await;
            }
            tokio::select! {
                biased;
                result = self.control.changed() => { if result.is_err() { let _ = self.teardown(true).await; break; } }
                _ = self.overflow_rx.changed() => {}
                command = self.commands.recv() => {
                    let Some(command) = command else { let _ = self.teardown(true).await; break; };
                    if command.sequence != self.processed_control {
                        if let Some(error) = reject(command.command, NativeError::cancelled()).await {
                            self.remember_pending_cleanup(&error);
                            self.emit_error(error);
                        }
                        continue;
                    }
                    match command.command {
                        Command::Join(options, reply) => { let result = self.join(options).await; let _ = reply.send(result); }
                        Command::Send(data, reply) => { let _ = reply.send(self.send(data)); }
                        Command::Publish(id, source, reply) => { let result = self.publish(id, source).await; let _ = reply.send(result); }
                        Command::Unpublish(id, reply) => { let result = self.unpublish(&id).await; let _ = reply.send(result); }
                        Command::MediaStats(reply) => { let result = self.media_stats().await; let _ = reply.send(result); }
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
            reject(command.command, NativeError::destroyed()).await;
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
        let local_sources = self
            .publications
            .iter()
            .map(|(id, source)| SourceDescriptor {
                id: id.clone(),
                kind: source.kind(),
            })
            .collect();
        let remote_sources = self
            .peers
            .iter()
            .filter(|(_, peer)| peer.remote_description)
            .flat_map(|(peer_id, peer)| {
                peer.source_manifest.iter().flat_map(move |(_, sources)| {
                    sources.iter().map(move |source| RemoteSourceDescriptor {
                        peer_id: peer_id.clone(),
                        id: source.id.clone(),
                        kind: source.kind,
                        mid: source.mid.clone(),
                    })
                })
            })
            .collect();
        if previous.state == state
            && previous.peer_id == peer_id
            && previous.peers == peers
            && previous.ready_peers == ready_peers
            && previous.local_sources == local_sources
            && previous.remote_sources == remote_sources
        {
            return;
        }
        self.shared.snapshot.send_replace(Snapshot {
            revision: previous.revision + 1,
            state,
            peer_id,
            peers,
            ready_peers,
            local_sources,
            remote_sources,
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
                self.end_session().await;
                return Err(error);
            }
        };
        self.signaling = Some(connected.signaling);
        if !valid_membership(&connected.peer_id, &connected.peers) {
            self.end_session().await;
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
                source_manifest: None,
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
            Input::Signaling(SignalingEvent::InvalidSignal { from }) => {
                if self.peers.contains_key(&from) {
                    self.remove_peer(&from);
                    self.emit_error(NativeError::new(
                        "invalid-signal",
                        "Peer sent an invalid media signal",
                    ));
                }
            }
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
                self.end_session().await;
            }
            Input::Signaling(SignalingEvent::Error(error)) => {
                let peer_departed = error.code == "peer-unavailable";
                self.emit_error(error);
                if !peer_departed {
                    self.end_session().await;
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
                            self.end_session().await;
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
                            self.end_session().await;
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
                self.attach_publications(peer_id)?;
                self.validate_peer_manifest(peer_id)?;
                self.send_manifest(peer_id)?;
                self.update_peers();
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
            SignalPayload::Sources {
                revision,
                mut sources,
            } => {
                if peer
                    .source_manifest
                    .as_ref()
                    .is_some_and(|(current, _)| revision < *current)
                {
                    return Ok(());
                }
                validate_manifest(&sources)?;
                sources.sort_by_key(|source| source.kind);
                if let Some((current, previous)) = &peer.source_manifest {
                    if revision == *current {
                        if previous != &sources {
                            return Err(NativeError::new(
                                "invalid-sources",
                                "Source manifest changed without a revision",
                            ));
                        }
                        return Ok(());
                    }
                }
                peer.source_manifest = Some((revision, sources));
                self.validate_peer_manifest(peer_id)?;
                self.update_peers();
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
    async fn publish(
        &mut self,
        id: String,
        source: Box<dyn SourcePort>,
    ) -> Result<Snapshot, NativeError> {
        let error = if self.shared.snapshot.borrow().state != State::Joined {
            Some(NativeError::new(
                "invalid-state",
                "Join a session before publishing",
            ))
        } else if !valid_peer_id(&id) {
            Some(NativeError::new(
                "invalid-source",
                "Source identifier is empty or exceeds limits",
            ))
        } else if self.publications.contains_key(&id)
            || self
                .publications
                .values()
                .any(|active| active.kind() == source.kind())
        {
            Some(NativeError::new(
                "duplicate-source",
                "Source identifier and kind must each be unique",
            ))
        } else {
            None
        };
        if let Some(error) = error {
            return Err(match close_source(source).await {
                Ok(()) => error,
                Err(cleanup_error) => {
                    self.remember_pending_cleanup(&cleanup_error);
                    cleanup_error
                }
            });
        }
        self.publications.insert(id.clone(), source);
        self.source_revision += 1;
        let peers: Vec<_> = self.peers.keys().cloned().collect();
        for peer_id in peers {
            let peer = self.peers.get_mut(&peer_id).expect("known peer");
            if !peer.remote_description {
                continue;
            }
            let source = self.publications.get(&id).expect("owned source");
            if let Err(error) = peer.port.set_source(source.kind(), Some(source.as_ref())) {
                // Even an adapter that fails after attaching must not keep hidden transmission.
                self.remove_peer(&peer_id);
                self.emit_error(error);
                continue;
            }
            if let Err(error) = self.send_manifest(&peer_id) {
                self.remove_peer(&peer_id);
                self.emit_error(error);
            }
        }
        self.update_peers();
        Ok(self.shared.snapshot.borrow().clone())
    }
    async fn unpublish(&mut self, id: &str) -> Result<Snapshot, NativeError> {
        let Some(source) = self.publications.remove(id) else {
            return Ok(self.shared.snapshot.borrow().clone());
        };
        self.source_revision += 1;
        let peers: Vec<_> = self.peers.keys().cloned().collect();
        for peer_id in peers {
            let peer = self.peers.get_mut(&peer_id).expect("known peer");
            if !peer.remote_description {
                continue;
            }
            if let Err(error) = peer.port.set_source(source.kind(), None) {
                self.remove_peer(&peer_id);
                self.emit_error(error);
                continue;
            }
            if let Err(error) = self.send_manifest(&peer_id) {
                self.remove_peer(&peer_id);
                self.emit_error(error);
            }
        }
        let result = close_source(source).await;
        self.update_peers();
        if let Err(error) = result {
            self.remember_pending_cleanup(&error);
            self.emit_error(error.clone());
            return Err(error);
        }
        Ok(self.shared.snapshot.borrow().clone())
    }
    fn attach_publications(&mut self, peer_id: &str) -> Result<(), NativeError> {
        let peer = self.peers.get_mut(peer_id).expect("known peer");
        for source in self.publications.values() {
            peer.port.set_source(source.kind(), Some(source.as_ref()))?;
        }
        Ok(())
    }
    fn send_manifest(&mut self, peer_id: &str) -> Result<(), NativeError> {
        let peer = self.peers.get(peer_id).expect("known peer");
        if !peer.remote_description {
            return Ok(());
        }
        let slots = peer.port.slots();
        let sources = self
            .publications
            .iter()
            .map(|(id, source)| {
                let slot = slots
                    .iter()
                    .find(|slot| slot.kind == source.kind())
                    .ok_or_else(|| {
                        NativeError::new("missing-slot", "Publication has no negotiated media slot")
                    })?;
                Ok(SourceBinding {
                    id: id.clone(),
                    kind: source.kind(),
                    mid: slot.mid.clone(),
                })
            })
            .collect::<Result<Vec<_>, NativeError>>()?;
        self.signal(
            peer_id,
            SignalPayload::Sources {
                revision: self.source_revision,
                sources,
            },
        )
    }
    fn validate_peer_manifest(&self, peer_id: &str) -> Result<(), NativeError> {
        let peer = self.peers.get(peer_id).expect("known peer");
        if !peer.remote_description {
            return Ok(());
        }
        let slots = peer.port.slots();
        if peer.source_manifest.as_ref().is_some_and(|(_, sources)| {
            sources.iter().any(|source| {
                !slots
                    .iter()
                    .any(|slot| slot.kind == source.kind && slot.mid == source.mid)
            })
        }) {
            return Err(NativeError::new(
                "invalid-sources",
                "Source manifest does not match negotiated slots",
            ));
        }
        Ok(())
    }
    async fn media_stats(&mut self) -> Result<Vec<PeerMediaStats>, NativeError> {
        if self.shared.snapshot.borrow().state != State::Joined {
            return Err(NativeError::new(
                "invalid-state",
                "Join a session before reading media observations",
            ));
        }
        let mut stats = Vec::new();
        for (peer_id, peer) in &mut self.peers {
            if !peer.remote_description {
                continue;
            }
            let media = interruptible(
                peer.port.media_stats(),
                &mut self.control,
                &mut self.overflow_rx,
                self.generation,
                Duration::from_secs(5),
            )
            .await?;
            stats.push(PeerMediaStats {
                peer_id: peer_id.clone(),
                media,
            });
        }
        Ok(stats)
    }
    fn remove_peer(&mut self, peer_id: &str) {
        if let Some(mut peer) = self.peers.remove(peer_id) {
            peer.port.close();
            self.update_peers();
        }
    }
    async fn end_session(&mut self) {
        let result = self.teardown(false).await;
        // A stop can interrupt an operation or arrive while its failure is being cleaned up.
        // That stop must acknowledge this cleanup result even if its own teardown is empty.
        if let Err(error) = result {
            self.remember_pending_cleanup(&error);
        }
    }
    fn remember_pending_cleanup(&mut self, error: &NativeError) {
        if self.control.borrow().sequence != self.processed_control {
            self.pending_cleanup_error
                .get_or_insert_with(|| error.clone());
        }
    }
    async fn teardown(&mut self, destroy: bool) -> Result<(), NativeError> {
        if destroy {
            self.commands.close();
        }
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
        let mut result = self.pending_cleanup_error.take().map_or(Ok(()), Err);
        let queued_result = self.reject_pending().await;
        if result.is_ok() {
            result = queued_result;
        }
        for (_, source) in std::mem::take(&mut self.publications) {
            if let Err(error) = close_source(source).await {
                if result.is_ok() {
                    result = Err(error);
                }
            }
        }
        self.source_revision += 1;
        let signaling_result = if let Some(mut signaling) = self.signaling.take() {
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
        if result.is_ok() {
            result = signaling_result;
        }
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
    async fn reject_pending(&mut self) -> Result<(), NativeError> {
        // Queue admission is bounded. Close abandoned owned sources concurrently so teardown
        // has one source-close deadline, not one deadline for every queued publication.
        let mut cleanup = tokio::task::JoinSet::new();
        for _ in 0..self.commands.len() {
            let Ok(queued) = self.commands.try_recv() else {
                break;
            };
            cleanup.spawn(reject(queued.command, NativeError::cancelled()));
        }
        let mut first_error = None;
        while let Some(result) = cleanup.join_next().await {
            match result {
                Ok(Some(error)) => {
                    first_error.get_or_insert(error);
                }
                Err(_) => {
                    first_error.get_or_insert(NativeError::new(
                        "cleanup-failed",
                        "Queued source cleanup failed",
                    ));
                }
                Ok(None) => {}
            }
        }
        first_error.map_or(Ok(()), Err)
    }
}

async fn reject(command: Command, error: NativeError) -> Option<NativeError> {
    match command {
        Command::Join(_, reply) | Command::Unpublish(_, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Send(_, reply) => {
            let _ = reply.send(Err(error));
        }
        Command::MediaStats(reply) => {
            let _ = reply.send(Err(error));
        }
        Command::Publish(_, source, reply) => {
            let cleanup_error = close_source(source).await.err();
            let _ = reply.send(Err(cleanup_error.clone().unwrap_or(error)));
            return cleanup_error;
        }
    }
    None
}
async fn close_source(mut source: Box<dyn SourcePort>) -> Result<(), NativeError> {
    tokio::time::timeout(Duration::from_secs(2), source.close())
        .await
        .unwrap_or_else(|_| {
            Err(NativeError::new(
                "cleanup-timeout",
                "Source cleanup exceeded its deadline",
            ))
        })
}
fn validate_manifest(sources: &[SourceBinding]) -> Result<(), NativeError> {
    use std::collections::BTreeSet;
    let mut ids = BTreeSet::new();
    let mut kinds = BTreeSet::new();
    let mut mids = BTreeSet::new();
    if sources.len() > 3
        || sources.iter().any(|source| {
            !valid_peer_id(&source.id)
                || !valid_peer_id(&source.mid)
                || !ids.insert(&source.id)
                || !kinds.insert(source.kind)
                || !mids.insert(&source.mid)
        })
    {
        return Err(NativeError::new(
            "invalid-sources",
            "Source manifest exceeds limits or repeats identifiers/slots",
        ));
    }
    Ok(())
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
