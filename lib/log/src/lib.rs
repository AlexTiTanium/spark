//! `tracing` subscriber install, packaged as a [`Plugin`]. Register
//! `LogPlugin` first so logging is live before any other plugin emits.
//!
//! Filter priority: `RUST_LOG` → [`DEFAULT_FILTER`].
//!
//! # Example
//!
//! ```
//! use spark_core::Application;
//! use spark_log::LogPlugin;
//!
//! Application::new().add_plugin(LogPlugin).run().unwrap();
//! ```

use spark_core::{Application, Plugin};
use thiserror::Error;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Default filter when `RUST_LOG` is unset.
///
/// `spark=info` is a byte-prefix match (see
/// [`tracing_subscriber::EnvFilter`]) — it covers every `spark*` crate
/// without listing them by name.
pub const DEFAULT_FILTER: &str = "spark=info,warn";

/// Installs the global `tracing` subscriber during startup. Unit
/// struct: pass it as `LogPlugin` (no `::default()`). Customise via
/// `RUST_LOG`.
///
/// # Examples
///
/// ```
/// spark_core::Application::new()
///     .add_plugin(spark_log::LogPlugin)
///     .run()
///     .unwrap();
/// ```
#[derive(Debug, Default)]
pub struct LogPlugin;

impl Plugin for LogPlugin {
    fn build(&self, app: &mut Application) {
        app.add_startup_system(|| {
            install_subscriber()?;
            Ok(())
        });
    }
}

/// Crate-private error type. Converted to
/// [`spark_core::EngineError`] via `?` before leaving the plugin.
#[derive(Debug, Error)]
#[non_exhaustive]
pub(crate) enum LogError {
    #[error("invalid tracing filter directive: {0}")]
    InvalidFilter(#[from] tracing_subscriber::filter::ParseError),

    #[error("global tracing subscriber is already installed: {0}")]
    AlreadyInstalled(#[from] tracing_subscriber::util::TryInitError),
}

fn install_subscriber() -> Result<(), LogError> {
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));

    tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer())
        .try_init()?;

    Ok(())
}
