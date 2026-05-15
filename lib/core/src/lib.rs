//! Foundation crate for the Spark engine.
//!
//! Sits at the bottom of the dependency graph (see `docs/PLAN.md`):
//! every other engine crate depends on this one. anyhow (for
//! [`EngineError`]) is the only dep in M1.
//!
//! Public surface: [`Application`], [`Plugin`], [`stages::STARTUP`],
//! [`EngineError`], [`VERSION`].
//!
//! # Example
//!
//! ```
//! use spark_core::{Application, Plugin};
//!
//! struct HelloPlugin;
//! impl Plugin for HelloPlugin {
//!     fn build(&self, app: &mut Application) {
//!         app.add_startup_system(|| Ok(()));
//!     }
//! }
//!
//! Application::new().add_plugin(HelloPlugin).run().unwrap();
//! ```

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
