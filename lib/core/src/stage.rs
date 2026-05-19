//! Named lifecycle slots in the engine schedule.
//!
//! Four stages exist today: [`stages::STARTUP`] — auto-run inside
//! [`Application::run`](crate::Application::run) once after every
//! `add_startup_system` closure has fired — and the per-frame trio
//! [`stages::PRE_UPDATE`] → [`stages::UPDATE`] →
//! [`stages::POST_UPDATE`], ticked every frame by
//! [`WindowPlugin`](../../spark_window/struct.WindowPlugin.html)'s
//! runner on each winit `RedrawRequested`. `FIRST` / `LAST` /
//! `FIXED_UPDATE` / `RENDER` still wait — they earn their constants
//! when their executors land.

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

    /// First per-frame stage. Convention: input gather, time tick,
    /// anything that prepares state the rest of the frame consumes.
    /// Run by
    /// [`WindowPlugin`](../../spark_window/struct.WindowPlugin.html)'s
    /// runner ahead of [`UPDATE`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(spark_core::stages::PRE_UPDATE, "pre_update");
    /// ```
    pub const PRE_UPDATE: &str = "pre_update";

    /// Main per-frame stage. The bulk of game logic — movement, AI,
    /// spawning, despawning — lives here. Run by
    /// [`WindowPlugin`](../../spark_window/struct.WindowPlugin.html)'s
    /// runner between [`PRE_UPDATE`] and [`POST_UPDATE`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(spark_core::stages::UPDATE, "update");
    /// ```
    pub const UPDATE: &str = "update";

    /// Last per-frame stage. Convention: bookkeeping that has to see
    /// the *settled* world for the frame — cleanup, reporting,
    /// off-screen culling, anything that should run after [`UPDATE`]'s
    /// commands have flushed. Run by
    /// [`WindowPlugin`](../../spark_window/struct.WindowPlugin.html)'s
    /// runner after [`UPDATE`].
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(spark_core::stages::POST_UPDATE, "post_update");
    /// ```
    pub const POST_UPDATE: &str = "post_update";
}
