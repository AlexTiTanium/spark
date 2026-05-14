//! Error type returned by [`crate::run`].
//!
//! # Summary
//!
//! [`WindowError`] is the single error type this crate surfaces to callers.
//! It wraps the two `winit` failure modes we can hit (event-loop
//! construction and window creation) so the binary never has to deal with
//! `Box<dyn Error>` or two different error types.
//!
//! # Logic
//!
//! Implemented as a `thiserror`-derived enum with one variant per
//! underlying `winit` error type. `#[from]` provides automatic
//! `From<winit::error::*>` conversions, so internal code can use the `?`
//! operator without manual wrapping.
//!
//! # Why it works
//!
//! `thiserror` generates `std::error::Error` + `Display` + the `From`
//! impls. The variant data carries the original error so a caller can
//! downcast and inspect platform-specific detail if it wants to. Our own
//! `Display` strings stay short and human-readable; the underlying error's
//! `Display` is appended by `thiserror`'s `{0}` placeholder.
//!
//! # How to use
//!
//! ```
//! fn maybe_window() -> Result<(), spark_window::WindowError> {
//!     // ...real code calls spark_window::run(...) here...
//!     Ok(())
//! }
//! ```
//!
//! # How NOT to use
//!
//! - Do not match on the variants for control flow (e.g. retry-on-`Os`);
//!   they are reported once at startup and there is no useful recovery —
//!   propagate them out of `main`.
//! - Do not add a `String` variant ("Other"). If a new failure mode
//!   appears, add a typed variant with `#[from]`.

use thiserror::Error;

/// Errors that can occur while constructing or running the application
/// window.
///
/// # Logic
///
/// Two variants, one per `winit` error type encountered in this crate:
///
/// - [`WindowError::EventLoop`] — wraps
///   [`winit::error::EventLoopError`], returned when the OS event loop
///   itself cannot be created or driven (rare; usually means another
///   library has already claimed the main thread).
/// - [`WindowError::Os`] — wraps
///   [`winit::error::OsError`], returned when the OS refuses to create
///   the window (invalid size, missing permissions, etc.).
///
/// # Why it works
///
/// `#[from]` gives free `From` impls so `?` works inside [`crate::run`]
/// without `.map_err` boilerplate. The wrapped error carries the
/// platform-specific detail; this crate's `Display` only adds a short
/// human-readable prefix.
///
/// # How to use
///
/// Propagate it out of `main`:
///
/// ```no_run
/// fn main() -> Result<(), spark_window::WindowError> {
///     spark_window::init_tracing();
///     spark_window::run(spark_window::WindowConfig::default())
/// }
/// ```
///
/// # How NOT to use
///
/// - Do not `.unwrap()` it; `winit` errors are user-visible at process
///   start and should be printed cleanly via `Result` propagation.
/// - Do not stringify and re-parse it; if you need to react to a specific
///   failure mode, match the variant.
///
/// # Examples
///
/// ```
/// use spark_window::WindowError;
/// // The error implements `Display` and `Error` via `thiserror`.
/// fn assert_impls_error<E: std::error::Error>(_: &E) {}
/// // We can't easily construct one without invoking winit, but we can
/// // confirm the trait bounds compile:
/// fn _check(e: &WindowError) {
///     assert_impls_error(e);
/// }
/// ```
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum WindowError {
    /// The OS event loop could not be created or driven.
    #[error("failed to create or drive the OS event loop: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),

    /// The OS refused to create the window.
    #[error("failed to create the OS window: {0}")]
    Os(#[from] winit::error::OsError),
}
