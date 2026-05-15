//! Error type returned by [`crate::run`].

use thiserror::Error;

/// Errors that can occur while opening or driving the application
/// window. Wraps the two `winit` failure modes we hit (event-loop
/// construction, window creation).
///
/// # Examples
///
/// ```
/// fn assert_error<E: std::error::Error>(_: &E) {}
/// fn check(e: &spark_window::WindowError) { assert_error(e); }
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
