//! Spark binary entry point.
//!
//! Composes four plugins:
//!
//! - `LogPlugin` installs the `tracing` subscriber as a startup
//!   closure.
//! - `SandboxPlugin` (in `crate::sandbox`) is the umbrella demo
//!   plugin. It inserts the shared resources every sub-sandbox uses
//!   (`TickCount`), then nests each sub-sandbox plugin via
//!   `app.add_plugin(...)`. Today that's just `EcsSandboxPlugin`
//!   (queues demo entities via `Commands` in STARTUP and registers
//!   `physics_step` / `decay_health` / `player_regen` / …); future
//!   render or input sub-sandboxes plug in next to it.
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
