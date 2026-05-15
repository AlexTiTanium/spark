//! Named lifecycle slots in the engine schedule.
//!
//! Only [`stages::STARTUP`] exists in M1; per-frame stages (`UPDATE` /
//! `RENDER` / …) ship with their executor in M4.

/// Stage name constants. Stages are `&'static str` so callers can
/// introduce their own without modifying core.
pub mod stages {
    /// The startup stage — the only stage with an executor in M1.
    ///
    /// Closures registered with
    /// [`Application::add_startup_system`](crate::Application::add_startup_system)
    /// run during this stage, in registration order, before the runner
    /// blocks.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(spark_core::stages::STARTUP, "startup");
    /// ```
    pub const STARTUP: &str = "startup";
}
