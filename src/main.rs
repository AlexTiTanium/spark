//! Spark binary entry point.
//!
//! `LogPlugin` registers the tracing subscriber install as a startup
//! closure; [`SandboxPlugin`] seeds the world with demo components +
//! resources, registers five `Query` / `Res` / `ResMut` systems, and
//! installs a tiny five-tick runner.
//!
//! Swap `SandboxPlugin` for `WindowPlugin` (commented out below) to
//! open a real window instead — the sandbox runner returns cleanly
//! so the binary exits after its loop.

mod sandbox;

use spark_core::{Application, EngineError};
use spark_log::LogPlugin;

use crate::sandbox::SandboxPlugin;

fn main() -> Result<(), EngineError> {
    Application::new()
        .add_plugin(LogPlugin)
        .add_plugin(SandboxPlugin)
        .run()
}

// To run with the OS window instead of the sandbox, re-enable
// `WindowPlugin` and drop `SandboxPlugin`:
//
// use spark_window::{WindowConfig, WindowPlugin};
//
// Application::new()
//     .add_plugin(LogPlugin)
//     .add_plugin(WindowPlugin {
//         config: WindowConfig::default()
//             .with_title("Spark")
//             .with_size(1280, 720)
//             .with_resizable(true),
//     })
//     .run()
