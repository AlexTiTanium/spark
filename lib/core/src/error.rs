//! Engine-wide erased error type.

/// The error type that flows through every Plugin →
/// [`Application`](crate::Application) seam and out of `fn main`.
///
/// Aliased from [`anyhow::Error`]. Typed library errors
/// (`WindowError`, `LogError`, …) convert into `EngineError`
/// automatically via `?` thanks to anyhow's blanket `From` impl for
/// any `std::error::Error + Send + Sync + 'static`. Plugins never
/// construct an `EngineError` themselves.
pub use anyhow::Error as EngineError;
