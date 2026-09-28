//! Native data runtime composition. No product roles or authorization policy.
mod signaling;
pub use pv_media_runtime::{
    Event, EventBatch, IceServer, JoinOptions, NativeDataClient, NativeError, SendResult, Snapshot,
    State,
};
use std::sync::Arc;

/// Construct within a running Tokio runtime. The Rust actor owns all state.
pub fn create_client() -> Result<NativeDataClient, NativeError> {
    tokio::runtime::Handle::try_current()
        .map_err(|_| NativeError::new("runtime", "A Tokio runtime is required"))?;
    NativeDataClient::new(
        Arc::new(pv_media_libwebrtc::LibWebRtcFactory::default()),
        Arc::new(signaling::WebSocketFactory),
    )
}
