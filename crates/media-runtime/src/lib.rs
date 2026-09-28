//! Role-neutral native data-session owner. Concrete IO is injected through ports.
pub mod ports;
mod runtime;
mod types;

pub use runtime::NativeDataClient;
pub use types::*;
