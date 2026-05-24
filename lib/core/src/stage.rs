//! The engine's fixed frame timeline, as the closed [`Stage`] enum.
//!
//! Every system is registered against one [`Stage`], and the engine
//! runs the stages in a fixed per-frame order. The set is *closed* on
//! purpose: there is exactly one frame timeline, so there is one shared
//! phase set. That makes a misspelled stage a compile error instead of
//! the silent no-op a `&'static str` label gave — register a system on
//! a typo'd string and it simply never runs. A subsystem that needs its
//! own ordering groups related systems into a *workload* inside a stage
//! (a later scheduler feature), rather than inventing a new stage.

/// The fixed per-frame phases, in execution order.
///
/// A **closed**, [`Copy`] enum. Being closed buys two things a
/// `&'static str` label can't: every `match` over a `Stage` is
/// exhaustive, and a typo'd stage is a *compile error* instead of a
/// system silently registered to a slot nothing ever runs. The variants
/// are listed in run order for readability, but the order is enforced by
/// the sequence of [`run_stage`](crate::Application::run_stage) calls
/// (see *When each stage runs* below) — nothing reads the discriminant.
///
/// # When each stage runs
///
/// [`Startup`](Self::Startup) fires once, inside
/// [`Application::run`](crate::Application::run), after every
/// `add_startup_system` closure. The per-frame sequence
/// [`Input`](Self::Input) → [`PreUpdate`](Self::PreUpdate) →
/// ([`FixedUpdate`](Self::FixedUpdate) × N) → [`Update`](Self::Update) →
/// [`PostUpdate`](Self::PostUpdate) is driven every frame by
/// [`WindowPlugin`](../../spark_window/struct.WindowPlugin.html)'s runner:
/// `Input` swaps double-buffered event queues (and, later, collects input
/// state), and `FixedUpdate` runs N times off a 60 Hz accumulator.
/// [`First`](Self::First), [`Render`](Self::Render), and
/// [`Last`](Self::Last) are reserved labels — defined now so call sites
/// (and the editor's future stage view) can name them, but not yet
/// auto-driven; their executors land with the scheduler (see
/// `docs/ECS_ROADMAP.md`).
///
/// Per-frame order:
/// `First → Input → PreUpdate → (FixedUpdate × N) → Update → PostUpdate → Render → Last`.
///
/// # Examples
///
/// ```
/// use spark_core::Stage;
///
/// // `Copy` + `Eq`: cheap to pass by value and to compare.
/// let stage = Stage::Update;
/// assert_eq!(stage, Stage::Update);
/// assert_ne!(Stage::Update, Stage::PreUpdate);
///
/// // Closed + exhaustive: this `match` must cover every variant, so a
/// // caller can never silently forget a stage (and a typo'd variant
/// // won't compile).
/// let name = match stage {
///     Stage::Startup => "startup",
///     Stage::First => "first",
///     Stage::Input => "input",
///     Stage::PreUpdate => "pre_update",
///     Stage::FixedUpdate => "fixed_update",
///     Stage::Update => "update",
///     Stage::PostUpdate => "post_update",
///     Stage::Render => "render",
///     Stage::Last => "last",
/// };
/// assert_eq!(name, "update");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    /// Once, before the main loop begins — world seeding, resource
    /// init. Auto-run by [`Application::run`](crate::Application::run)
    /// after the `add_startup_system` closures.
    Startup,
    /// Very first thing each frame. Reserved label; not yet auto-driven.
    First,
    /// Top of each frame, before [`PreUpdate`](Self::PreUpdate): swaps the
    /// double-buffered event queues (and, later, collects input state).
    /// Driven by `WindowPlugin`'s runner, pumped first in the frame.
    Input,
    /// Time tick and per-frame setup — prepares the state the rest of the
    /// frame consumes.
    PreUpdate,
    /// Fixed-timestep simulation; runs N times per frame off a real-time
    /// accumulator (60 Hz). Driven by `WindowPlugin`'s runner.
    FixedUpdate,
    /// Main game logic at display rate — movement, AI, spawn/despawn.
    Update,
    /// Cleanup and bookkeeping over the settled world for the frame.
    PostUpdate,
    /// Build draw lists, submit GPU work. Reserved label; not yet
    /// auto-driven.
    Render,
    /// Very last thing each frame. Reserved label; not yet auto-driven.
    Last,
}
