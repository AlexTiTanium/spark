//! Window and OS-event-loop layer for the Spark engine.
//!
//! Owns the platform window and the operating-system event loop, built on
//! [`winit`]. Exposes a free function — [`run`] — that opens a window,
//! drives the event loop, and emits `tracing` events for every OS event
//! of interest. In M4 the same internals move behind a `WindowPlugin`
//! once `spark-ecs` lands the canonical `App` / `Plugin` traits (see
//! `docs/ECS_DESIGN.md` stage 14).
//!
//! Public surface: [`WindowConfig`], [`WindowError`], [`run`],
//! [`init_tracing`].
//!
//! # Examples
//!
//! ```no_run
//! fn main() -> Result<(), spark_window::WindowError> {
//!     spark_window::init_tracing();
//!     spark_window::run(
//!         spark_window::WindowConfig::default()
//!             .with_title("Spark")
//!             .with_size(1280, 720),
//!     )
//! }
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
mod log;

pub use config::WindowConfig;
pub use error::WindowError;
pub use event_loop::run;
pub use log::init_tracing;
