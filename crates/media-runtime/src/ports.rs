//! IO contracts. Callbacks enqueue owned values; they do not own session policy.
use crate::{
    Candidate, Description, IceServer, JoinOptions, MediaObservation, MediaSlot, NativeError,
    SignalPayload, SourceKind,
};
use std::{any::Any, future::Future, pin::Pin};
use tokio::sync::{mpsc, watch};

pub type PortFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, NativeError>> + Send + 'a>>;

pub enum SignalingEvent {
    PeerJoined(String),
    PeerLeft(String),
    Signal {
        from: String,
        payload: SignalPayload,
    },
    InvalidSignal {
        from: String,
    },
    Closed,
    Error(NativeError),
}
pub enum EngineEvent {
    Ready(bool),
    Candidate(Candidate),
    Message(String),
    Failed,
    Error(NativeError),
}
pub enum Input {
    Signaling(SignalingEvent),
    Engine { peer_id: String, event: EngineEvent },
}
pub struct StampedInput {
    pub generation: u64,
    pub peer_instance: u64,
    pub input: Input,
}

/// Shared bounded callback inlet. Overflow is reported through a separate watch
/// slot so a full queue cannot hide the need to close the session.
#[derive(Clone)]
pub struct EventSink {
    generation: u64,
    peer_instance: u64,
    sender: mpsc::Sender<StampedInput>,
    overflow: watch::Sender<u64>,
}
impl EventSink {
    pub(crate) fn new(
        generation: u64,
        sender: mpsc::Sender<StampedInput>,
        overflow: watch::Sender<u64>,
    ) -> Self {
        Self {
            generation,
            peer_instance: 0,
            sender,
            overflow,
        }
    }
    pub(crate) fn for_peer(mut self, instance: u64) -> Self {
        self.peer_instance = instance;
        self
    }
    pub fn send(&self, input: Input) -> bool {
        match self.sender.try_send(StampedInput {
            generation: self.generation,
            peer_instance: self.peer_instance,
            input,
        }) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.overflow
                    .send_modify(|value| *value = (*value).max(self.generation));
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

pub struct Connected {
    pub peer_id: String,
    pub peers: Vec<String>,
    pub signaling: Box<dyn SignalingPort>,
}
pub trait SignalingFactory: Send + Sync {
    /// Dropping this future must release any incomplete socket connection.
    fn connect(&self, options: JoinOptions, events: EventSink) -> PortFuture<'_, Connected>;
}
pub trait SignalingPort: Send {
    /// Enqueue one bounded write. No independent reconnect policy.
    fn send(&mut self, to: &str, payload: SignalPayload) -> Result<(), NativeError>;
    /// Complete owned tasks and close the socket. Drop must also cancel tasks.
    fn close(&mut self) -> PortFuture<'_, ()>;
}
pub trait EngineFactory: Send + Sync {
    fn create(
        &self,
        peer_id: &str,
        initiator: bool,
        ice: &[IceServer],
        events: EventSink,
    ) -> Result<Box<dyn PeerPort>, NativeError>;
}
pub trait PeerPort: Send {
    fn offer(&mut self) -> PortFuture<'_, Description>;
    fn answer(&mut self, offer: Description) -> PortFuture<'_, Description>;
    fn accept_answer(&mut self, answer: Description) -> PortFuture<'_, ()>;
    fn add_candidate(&mut self, candidate: Candidate) -> PortFuture<'_, ()>;
    fn send(&mut self, data: &str) -> Result<(), NativeError>;
    /// Actual negotiated slots, available after remote description application.
    fn slots(&self) -> Vec<MediaSlot>;
    /// Bind/detach one source without renegotiation. The runtime retains source ownership.
    fn set_source(
        &mut self,
        kind: SourceKind,
        source: Option<&dyn SourcePort>,
    ) -> Result<(), NativeError>;
    /// Scalar native observations only; raw media stays in the engine's bounded sinks.
    fn media_stats(&mut self) -> PortFuture<'_, Vec<MediaObservation>>;
    /// Called by the owner outside native callbacks. Must be idempotent.
    fn close(&mut self);
}

/// One producer/source handle, transferred to the runtime by publish even on failure.
/// close joins owned producer tasks; Drop must cancel them if an operation is abandoned.
pub trait SourcePort: Send + Sync {
    fn kind(&self) -> SourceKind;
    fn as_any(&self) -> &dyn Any;
    fn close(&mut self) -> PortFuture<'_, ()>;
}
