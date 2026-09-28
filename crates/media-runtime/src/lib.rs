//! Role-neutral native media-session owner. Concrete IO is injected through ports.
pub mod ports;
mod runtime;
mod types;

pub use runtime::NativeMediaClient;
pub use types::*;
