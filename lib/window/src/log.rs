//! Process-wide `tracing` setup for the engine.
//!
//! # Summary
//!
//! Provides [`init_tracing`], a one-shot helper that installs a global
//! [`tracing_subscriber`] formatter on stdout. Binaries call it once,
//! before they call [`crate::run`].
//!
//! # Logic
//!
//! Builds a [`tracing_subscriber::fmt`] subscriber whose level filter
//! reads from the `RUST_LOG` environment variable (via the `env-filter`
//! feature), falling back to `info` for `spark_*` crates and `warn`
//! elsewhere. The subscriber is installed with
//! [`tracing::subscriber::set_global_default`], which can be called at
//! most once per process — subsequent calls are reported via the function
//! return value of [`tracing::subscriber::set_global_default`] and turned
//! into a `tracing::warn!` so tests that double-init don't panic.
//!
//! # Why it works
//!
//! `tracing` is the modern replacement for `log` + `env_logger`
//! (`CLAUDE.md` "Outdated patterns to avoid"). One global subscriber per
//! process is the documented model: every `tracing::info!`,
//! `tracing::debug!`, etc. macro routes through it.
//!
//! Using `try_init` (rather than `init`) and converting failure to a
//! warning means re-init in integration tests does not bring the process
//! down; it just keeps the first subscriber.
//!
//! # How to use
//!
//! ```no_run
//! spark_window::init_tracing();
//! tracing::info!("logger up");
//! ```
//!
//! # How NOT to use
//!
//! - Do not call this from a library; only the binary should install a
//!   global subscriber. Libraries should `tracing::info!(...)` and let
//!   the binary decide where the logs go.
//! - Do not call this more than once. The second call is harmless but
//!   wasted work.

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;

/// Installs a global `tracing` subscriber that writes formatted log
/// records to stdout.
///
/// # Logic
///
/// Builds a [`tracing_subscriber::fmt::Subscriber`] whose level filter is
/// taken from the `RUST_LOG` environment variable when set, and falls
/// back to a default of `info` for `spark_*` crates plus `warn` for
/// everything else. Installs it with
/// [`tracing_subscriber::util::SubscriberInitExt::try_init`], which only
/// succeeds the first time it is called per process; subsequent calls
/// emit a `warn!` and otherwise do nothing.
///
/// # Why it works
///
/// `try_init` returns `Err` instead of panicking when a subscriber is
/// already installed, so this function is safe to call from tests that
/// re-enter the binary's startup path. The default filter favours engine
/// chatter (`spark_*=info`) over third-party deps (`warn`) so a fresh
/// `cargo run -p spark` produces a useful but readable log.
///
/// # How to use
///
/// In `fn main`:
///
/// ```no_run
/// fn main() -> Result<(), spark_window::WindowError> {
///     spark_window::init_tracing();
///     spark_window::run(spark_window::WindowConfig::default())
/// }
/// ```
///
/// Override the filter at runtime:
///
/// ```bash
/// RUST_LOG=spark_window=debug,winit=warn cargo run -p spark
/// ```
///
/// # How NOT to use
///
/// - Do not call this from a library or from a doc test that does not
///   need logging — the global subscriber is process-wide state.
/// - Do not assume the first call always succeeds; in a test binary
///   another test may have initialised it already. The function will
///   `warn!` in that case and continue.
///
/// # Examples
///
/// ```
/// // Calling init_tracing in a doc test installs the subscriber for the
/// // duration of the doc-test binary. The second call is a no-op.
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
