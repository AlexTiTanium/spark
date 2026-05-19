//! Spark binary entry point.
//!
//! Composes four plugins:
//!
//! - `LogPlugin` installs the `tracing` subscriber as a startup
//!   closure.
//! - `SandboxPlugin` queues four demo entities through `Commands` in
//!   STARTUP and registers the per-frame systems
//!   (`integrate_movement`, `decay_health`, …) on `PRE_UPDATE` /
//!   `UPDATE` / `POST_UPDATE`.
//! - `WindowPlugin` opens the OS window and installs the runner that
//!   ticks `PRE_UPDATE → UPDATE → POST_UPDATE` on every winit
//!   `RedrawRequested`.
//!
//! Run it: `cargo run -p spark`. Filter logs with `RUST_LOG=spark=info`
//! (default — startup only) or `RUST_LOG=spark=debug` to see
//! per-frame position updates.

mod sandbox;

use spark_core::{Application, EngineError};
use spark_log::LogPlugin;
use spark_window::{WindowConfig, WindowPlugin};

use crate::sandbox::SandboxPlugin;

fn main() -> Result<(), EngineError> {
    Application::new()
        .add_plugin(LogPlugin)
        .add_plugin(SandboxPlugin)
        .add_plugin(WindowPlugin {
            config: WindowConfig::default()
                .with_title("Spark")
                .with_size(1280, 720)
                .with_resizable(true),
        })
        .run()
}
