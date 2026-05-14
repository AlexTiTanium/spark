//! Window and OS-event-loop layer for the Spark engine.
//!
//! # Summary
//!
//! `spark-window` is the first engine crate above [`spark_core`]. It owns the
//! platform window and the operating-system event loop, built on top of
//! [`winit`]. Today it exposes a single free function — [`run`] — that opens
//! a window, drives the event loop, and emits `tracing` events for every OS
//! event of interest (resize, close, key, mouse, focus, …).
//!
//! In milestone M4 — once `spark-ecs` lands the canonical `App` and `Plugin`
//! traits (see `docs/ECS_DESIGN.md`, stage 14) — the same code moves behind
//! a `WindowPlugin`. The public API of *this* crate is intentionally tiny
//! so that migration is a rename, not a rewrite.
//!
//! # Logic
//!
//! Three concepts cooperate:
//!
//! 1. [`WindowConfig`] — a plain data struct (title, size, resizable) built
//!    with a fluent builder. It is `#[non_exhaustive]` so we can add fields
//!    later without a breaking change.
//! 2. [`run`] — takes a [`WindowConfig`], constructs a [`winit`]
//!    [`EventLoop`](winit::event_loop::EventLoop), then drives it with an
//!    internal `EventLoopRunner` value that implements
//!    [`ApplicationHandler`](winit::application::ApplicationHandler).
//! 3. [`init_tracing`] — a one-shot helper that installs a
//!    [`tracing_subscriber`] formatter. Binaries call it once at startup
//!    before they call [`run`]; tests skip it.
//!
//! Errors from winit are converted to [`WindowError`] (a `thiserror`-derived
//! enum) so callers never see `Box<dyn Error>`.
//!
//! # Why it works
//!
//! All state lives inside the `EventLoopRunner` value passed to
//! [`EventLoop::run_app`](winit::event_loop::EventLoop::run_app); there are
//! no globals, no `static mut`, no `lazy_static!`. The crate has nothing to
//! teardown because the OS owns the window's lifetime and `winit` cleans up
//! on `el.exit()`.
//!
//! Once [`spark_ecs`] arrives the same `EventLoopRunner` field set becomes a
//! `Window` resource on the `World`, so the migration from free-function to
//! plugin/resource is mechanical.
//!
//! # How to use
//!
//! Typical binary entry point:
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
//! With logging enabled via `RUST_LOG`:
//!
//! ```bash
//! RUST_LOG=spark_window=debug cargo run -p spark
//! ```
//!
//! # How NOT to use
//!
//! - Do not call [`run`] from a test or doc test — it blocks on the OS
//!   event loop and the test process will hang.
//! - Do not call [`init_tracing`] more than once per process. The function
//!   tolerates a re-init attempt (logs a warning instead of panicking), but
//!   the second subscriber is silently dropped.
//! - Do not store the [`winit::window::Window`] across the call boundary —
//!   it is owned by the runner and dropped when [`run`] returns.
//!
//! # Examples
//!
//! ```
//! // Building a config does not open a window — safe in doc tests.
//! let cfg = spark_window::WindowConfig::default()
//!     .with_title("doctest")
//!     .with_size(800, 600);
//! assert_eq!(cfg.title, "doctest");
//! assert_eq!(cfg.size, (800, 600));
//! ```

mod config;
mod error;
mod event_loop;
mod log;

pub use config::WindowConfig;
pub use error::WindowError;
pub use event_loop::run;
pub use log::init_tracing;
