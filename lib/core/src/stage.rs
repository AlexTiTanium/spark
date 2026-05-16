//! Named lifecycle slots in the engine schedule.
//!
//! Two stages exist today: [`stages::STARTUP`] — auto-run inside
//! [`Application::run`](crate::Application::run) once after every
//! `add_startup_system` closure has fired — and [`stages::UPDATE`],
//! reachable explicitly via
//! [`Application::run_stage`](crate::Application::run_stage). The
//! per-frame loop that ticks `UPDATE` every iteration lands with the
//! next PR; finer-grained per-frame stages (`PRE_UPDATE` /
//! `POST_UPDATE` / `RENDER`) deliberately wait until that driver
//! exists.

/// Stage name constants. Stages are `&'static str` so callers can
/// introduce their own without modifying core.
pub mod stages {
    /// The startup stage. Systems registered here via
    /// [`add_system`](crate::Application::add_system) run exactly once,
    /// in registration order, during
    /// [`Application::run`](crate::Application::run) — *after* every
    /// `add_startup_system` closure has fired and *before* the runner
    /// takes the thread.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(spark_core::stages::STARTUP, "startup");
    /// ```
    pub const STARTUP: &str = "startup";

    /// The per-frame update stage. Systems registered here are reached
    /// today via explicit
    /// [`Application::run_stage`](crate::Application::run_stage)
    /// calls; the per-frame loop that ticks them every iteration lands
    /// in the follow-up PR.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(spark_core::stages::UPDATE, "update");
    /// ```
    pub const UPDATE: &str = "update";
}
