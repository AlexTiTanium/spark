//! Process-wide `tracing` setup.
//!
//! One helper, [`init_tracing`], that binaries call once at startup
//! before [`crate::run`]. Will move to `spark_core::log` when a second
//! engine crate also needs to emit logs.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Installs a global `tracing` subscriber that writes formatted records
/// to stdout.
///
/// Level filter reads from `RUST_LOG` when set, else defaults to `info`
/// for `spark_*` crates and `warn` for everything else. Uses `try_init`
/// so a second call from inside a test binary warns and returns instead
/// of panicking.
///
/// # Examples
///
/// ```
/// spark_window::init_tracing();
/// tracing::info!("doc test log line");
/// ```
pub fn init_tracing() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("spark_window=info,spark_core=info,spark=info,warn"));

    let result = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer())
        .try_init();

    if result.is_err() {
        tracing::warn!("tracing subscriber already installed; keeping the first one");
    }
}
