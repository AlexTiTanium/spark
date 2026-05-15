//! Window and OS-event-loop layer for the Spark engine.
//!
//! Owns the platform window and the OS event loop, built on [`winit`].
//! Two entry points:
//!
//! - [`WindowPlugin`] — idiomatic. Installs [`run`] as the
//!   [`Application`](spark_core::Application)'s runner.
//! - [`run`] — the free function the plugin delegates to. Useful for
//!   low-level callers that bypass the `Application` scaffolding.
//!
//! Subscriber install lives in `spark-log`.
//!
//! # Examples
//!
//! ```
//! use spark_core::Application;
//! use spark_window::WindowPlugin;
//!
//! let _app = Application::new().add_plugin(WindowPlugin::default());
//! ```
//!
//! Verbose logs via `RUST_LOG`:
//!
//! ```bash
//! RUST_LOG=spark_window=debug cargo run -p spark
//! ```

mod config;
mod error;
mod event_loop;
mod plugin;

pub use config::WindowConfig;
pub use error::WindowError;
pub use event_loop::run;
pub use plugin::WindowPlugin;
