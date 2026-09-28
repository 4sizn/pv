//! Authenticated, role-neutral signaling. HTTP/WS adapters delegate membership to state.
pub mod config;
pub mod http;
pub mod ice;
pub mod models;
pub mod state;
pub mod ws;

pub use http::router;
pub use state::AppState;
