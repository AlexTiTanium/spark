//! Spark binary entry point.
//!
//! Composes the engine plugins:
//!
//! - `LogPlugin` installs the `tracing` subscriber as a startup
//!   closure.
//! - `SparkMcpPlugin` (debug builds only) spawns the local MCP HTTP
//!   server on `127.0.0.1:9123` so AI agents can drive the game. The
//!   crate root is `#![cfg(debug_assertions)]`, so release builds
//!   compile it to nothing — see `lib/mcp/README.md`.
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
    // Bound to `let mut app` rather than chained because the
    // `SparkMcpPlugin` add below is `#[cfg(debug_assertions)]` — a
    // mid-chain method call cannot be conditionally compiled out.
    let mut app = Application::new();
    app.add_plugin(LogPlugin);

    // Debug-only MCP server. The plugin crate's root is itself
    // `#![cfg(debug_assertions)]`, so the symbol is absent in release
    // builds — this gate matches that, keeping the call site clean.
    #[cfg(debug_assertions)]
    app.add_plugin(spark_mcp::SparkMcpPlugin::default());

    app.add_plugin(TimePlugin);
    app.add_plugin(InputPlugin);
    app.add_plugin(SandboxPlugin);
    app.add_plugin(WindowPlugin {
        config: WindowConfig::default()
            .with_title("Spark")
            .with_size(1280, 720)
            .with_resizable(true),
    });
    app.run()
}
