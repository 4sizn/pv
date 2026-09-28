//! Native media runtime composition. No product roles or authorization policy.
mod signaling;
pub use pv_media_runtime::ports::{EngineFactory, SourcePort};
pub use pv_media_runtime::{
    Event, EventBatch, IceServer, JoinOptions, MediaObservation, MediaSlot, NativeError,
    NativeMediaClient, PeerMediaStats, RemoteSourceDescriptor, SendResult, Snapshot,
    SourceDescriptor, SourceKind, State,
};
use std::sync::Arc;

/// Construct within a running Tokio runtime. The Rust actor owns all state.
pub fn create_client() -> Result<NativeMediaClient, NativeError> {
    tokio::runtime::Handle::try_current()
        .map_err(|_| NativeError::new("runtime", "A Tokio runtime is required"))?;
    create_client_with_engine(Arc::new(pv_media_libwebrtc::LibWebRtcFactory::default()))
}

/// Share an engine factory with native source adapters. Signaling stays in this composition root.
pub fn create_client_with_engine(
    engine: Arc<dyn EngineFactory>,
) -> Result<NativeMediaClient, NativeError> {
    NativeMediaClient::new(engine, Arc::new(signaling::WebSocketFactory))
}
