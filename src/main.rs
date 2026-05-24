//! Spark binary entry point.
//!
//! Composes six plugins:
//!
//! - `LogPlugin` installs the `tracing` subscriber as a startup
//!   closure.
//! - `TimePlugin` inserts the `Time` resource and advances it each
//!   frame in `PreUpdate`. Registered before `WindowPlugin`, whose
//!   runner reads `Time::fixed_steps_this_frame()` to drive
//!   `FixedUpdate` dispatch.
//! - `InputPlugin` inserts `KeyboardState` / `MouseState` and the
//!   collection systems on `Stage::Input`. `WindowPlugin` forwards OS
//!   input into the events those systems read, so registering it here
//!   makes the state available to every later stage.
//! - `SandboxPlugin` (in `crate::sandbox`) is the umbrella demo
//!   plugin. It inserts the shared resources every sub-sandbox uses
//!   (`TickCount`), then nests each sub-sandbox plugin via
//!   `app.add_plugin(...)`: `EcsSandboxPlugin` (queues demo entities
//!   via `Commands` in Startup and registers `physics_step` /
//!   `decay_health` / `player_regen` / …) and `InputSandboxPlugin`
//!   (logs live `KeyboardState` / `MouseState`).
//! - `WindowPlugin` opens the OS window and installs the runner that
//!   ticks `Input → PreUpdate → (FixedUpdate × N) → Update → PostUpdate`
//!   on every winit `RedrawRequested`, reading
//!   `Time::fixed_steps_this_frame()` to drive the `FixedUpdate` count.
//!
//! Run it: `cargo run -p spark`. Filter logs with `RUST_LOG=spark=info`
//! (default — startup only) or `RUST_LOG=spark=debug` to see
//! per-frame position updates.

mod sandbox;

use spark_common::TimePlugin;
use spark_core::{Application, EngineError};
use spark_input::InputPlugin;
use spark_log::LogPlugin;
use spark_window::{WindowConfig, WindowPlugin};

use crate::sandbox::SandboxPlugin;

fn main() -> Result<(), EngineError> {
    Application::new()
        .add_plugin(LogPlugin)
        .add_plugin(TimePlugin)
        .add_plugin(InputPlugin)
        .add_plugin(SandboxPlugin)
        .add_plugin(WindowPlugin {
            config: WindowConfig::default()
                .with_title("Spark")
                .with_size(1280, 720)
                .with_resizable(true),
        })
        .run()
}
