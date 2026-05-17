#![doc = include_str!("../README.md")]
// `application::Application`, `plugin::Plugin`, `error::EngineError` —
// the public names deliberately echo their modules so the crate root
// reads like a Bevy-style plugin API.
#![allow(clippy::module_name_repetitions)]

mod application;
mod error;
mod plugin;
mod stage;

pub use application::Application;
pub use error::EngineError;
pub use plugin::Plugin;
pub use stage::stages;

/// Semantic version of `spark-core` from `Cargo.toml`.
///
/// # Examples
///
/// ```
/// assert!(!spark_core::VERSION.is_empty());
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
