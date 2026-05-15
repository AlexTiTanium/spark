//! Spark binary entry point.
//!
//! `LogPlugin` registers the tracing subscriber install as a startup
//! closure; `WindowPlugin` installs the OS event-loop runner. `run()`
//! drains startup, then hands the main thread to the runner.

use spark_core::{Application, EngineError};
use spark_log::LogPlugin;
use spark_window::{WindowConfig, WindowPlugin};

fn main() -> Result<(), EngineError> {
    Application::new()
        .add_plugin(LogPlugin)
        .add_plugin(WindowPlugin {
            config: WindowConfig::default()
                .with_title("Spark")
                .with_size(1280, 720)
                .with_resizable(true),
        })
        .run()
}
