//! [`WindowPlugin`] — wires [`crate::run`] into [`Application`] as the
//! runner that owns the per-frame loop.

use spark_core::{Application, EngineError, Plugin};

use crate::config::WindowConfig;
use crate::event_loop;

/// Installs [`crate::run`] as the [`Application`]'s runner — the
/// closure that owns the main thread once startup has finished and
/// ticks `PreUpdate → Update → PostUpdate` on every winit
/// `RedrawRequested`.
///
/// # Examples
///
/// ```
/// use spark_core::Application;
/// use spark_window::{WindowConfig, WindowPlugin};
///
/// // Defaults (1280×720, titled "Spark"):
/// let _app = Application::new().add_plugin(WindowPlugin::default());
///
/// // Custom config via struct literal:
/// let _app = Application::new().add_plugin(WindowPlugin {
///     config: WindowConfig::default().with_title("Demo"),
/// });
/// ```
#[derive(Debug, Default)]
pub struct WindowPlugin {
    /// Handed verbatim to [`crate::run`] when the runner fires.
    pub config: WindowConfig,
}

impl Plugin for WindowPlugin {
    fn build(&self, app: &mut Application) {
        let config = self.config.clone();
        app.set_runner(move |app: Application| -> Result<(), EngineError> {
            event_loop::run(app, config)?;
            Ok(())
        });
    }
}
