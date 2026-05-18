//! The [`Application`] type — the engine's composition root. Knows
//! nothing about windowing or logging; every capability is supplied
//! by a [`Plugin`].

use std::collections::HashMap;

use spark_ecs::{IntoSystem, World};

use crate::error::EngineError;
use crate::plugin::Plugin;
use crate::stage::stages;

type StartupSystem = Box<dyn FnOnce() -> Result<(), EngineError>>;
type StageSystem = Box<dyn FnMut(&World) + 'static>;
type Runner = Box<dyn FnOnce(Application) -> Result<(), EngineError>>;

/// Ordered list of plugins' registered work, plus the optional runner
/// that takes the main thread after startup.
///
/// Four phases per [`run`](Self::run):
///
/// 1. Plugins are *built* (each one pushes startup closures, registers
///    systems, and/or installs a runner).
/// 2. Startup closures are *drained* in registration order with `?`
///    short-circuiting on the first error.
/// 3. `stages::STARTUP` systems run once, in registration order, with
///    access to the populated [`World`].
/// 4. The runner — if any — receives an [`Application`] and blocks.
///
/// With no runner, `run` returns right after STARTUP, useful for
/// headless plugin tests.
///
/// [`set_runner`](Self::set_runner) is last-write-wins; only one
/// plugin should install a runner in a normal program.
///
/// # Examples
///
/// ```
/// use spark_core::{Application, Plugin};
///
/// struct CounterPlugin;
/// impl Plugin for CounterPlugin {
///     fn build(&self, app: &mut Application) {
///         app.add_startup_system(|| Ok(()));
///     }
/// }
///
/// Application::new().add_plugin(CounterPlugin).run().unwrap();
/// ```
#[derive(Default)]
pub struct Application {
    world: World,
    startup: Vec<StartupSystem>,
    stages: HashMap<&'static str, Vec<StageSystem>>,
    runner: Option<Runner>,
}

impl Application {
    /// Creates an empty `Application` — no plugins, no startup
    /// closures, no systems, no runner.
    ///
    /// # Examples
    ///
    /// ```
    /// spark_core::Application::new().run().unwrap();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a plugin by invoking its [`Plugin::build`] immediately.
    /// Returns `&mut Self` so plugins chain.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Plugin};
    ///
    /// struct NoOp;
    /// impl Plugin for NoOp {
    ///     fn build(&self, _: &mut Application) {}
    /// }
    ///
    /// Application::new().add_plugin(NoOp).add_plugin(NoOp).run().unwrap();
    /// ```
    // Spec mandates `plugin: P` by value with `Plugin::build` taking
    // `&self`; the allow records the deliberate ergonomic trade.
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_plugin<P: Plugin>(&mut self, plugin: P) -> &mut Self {
        plugin.build(self);
        self
    }

    /// Inserts a resource into the engine's [`World`]. Chainable; a
    /// second insert of the same type overwrites the previous value.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::Application;
    ///
    /// struct GameTime { dt: f32 }
    ///
    /// Application::new()
    ///     .add_resource(GameTime { dt: 0.016 })
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn add_resource<T: 'static>(&mut self, value: T) -> &mut Self {
        self.world.add_resource(value);
        self
    }

    /// Mutable access to the underlying [`World`] — an escape hatch
    /// for plugin `build()` methods that need to pre-populate state
    /// directly: spawning a level full of entities, seeding several
    /// resources in a loop, or anything else for which
    /// [`add_resource`](Self::add_resource) is too narrow.
    ///
    /// Inside a running system, prefer
    /// [`Res<T>`](spark_ecs::Res) / [`ResMut<T>`](spark_ecs::ResMut)
    /// (and the `Query` / `Commands` params that land in the next
    /// PRs). This method is for the *registration* path, not the
    /// per-frame path.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Plugin};
    ///
    /// struct Tile { x: i32, y: i32 }
    /// struct Walkable;
    ///
    /// struct LevelPlugin;
    /// impl Plugin for LevelPlugin {
    ///     fn build(&self, app: &mut Application) {
    ///         let world = app.world_mut();
    ///         for y in 0..3 {
    ///             for x in 0..3 {
    ///                 world.spawn().insert(Tile { x, y }).insert(Walkable);
    ///             }
    ///         }
    ///     }
    /// }
    ///
    /// Application::new().add_plugin(LevelPlugin).run().unwrap();
    /// ```
    pub fn world_mut(&mut self) -> &mut World {
        &mut self.world
    }

    /// Registers a closure to run during startup. Closures fire in
    /// registration order; the first `Err` short-circuits
    /// [`run`](Self::run).
    ///
    /// Prefer [`add_system`](Self::add_system) for systems that take
    /// [`Res`](spark_ecs::Res) / [`ResMut`](spark_ecs::ResMut) params
    /// and return `()`; this closure form is reserved for fallible,
    /// world-independent initialisation (installing a global tracing
    /// subscriber, opening files, etc.).
    ///
    /// # Examples
    ///
    /// ```
    /// spark_core::Application::new()
    ///     .add_startup_system(|| Ok(()))
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn add_startup_system<F>(&mut self, f: F) -> &mut Self
    where
        F: FnOnce() -> Result<(), EngineError> + 'static,
    {
        self.startup.push(Box::new(f));
        self
    }

    /// Registers a system on a named stage. Accepts any Rust fn whose
    /// parameters are all [`SystemParam`](spark_ecs::SystemParam) —
    /// today, [`Res<T>`](spark_ecs::Res) and
    /// [`ResMut<T>`](spark_ecs::ResMut) — for arities 0..=4.
    ///
    /// Systems registered on [`stages::STARTUP`](crate::stages::STARTUP)
    /// run once during [`run`](Self::run); systems on other stages
    /// (e.g. [`stages::UPDATE`](crate::stages::UPDATE)) run when a
    /// caller invokes [`run_stage`](Self::run_stage) with that stage
    /// name. The per-frame driver that ticks `UPDATE` automatically
    /// lands with the next PR.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{stages, Application};
    /// use spark_ecs::ResMut;
    ///
    /// struct Counter(u32);
    ///
    /// fn tick(mut c: ResMut<Counter>) {
    ///     c.0 += 1;
    /// }
    ///
    /// let mut app = Application::new();
    /// app.add_resource(Counter(0))
    ///    .add_system(stages::STARTUP, tick);
    /// app.run().unwrap();
    /// // The STARTUP system fired once during `run`.
    /// ```
    pub fn add_system<S, Marker>(&mut self, stage: &'static str, system: S) -> &mut Self
    where
        S: IntoSystem<Marker>,
    {
        self.stages
            .entry(stage)
            .or_default()
            .push(system.into_system());
        self
    }

    /// Runs every system registered to `stage` once, in registration
    /// order. No-op for stages that have no registered systems.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{stages, Application};
    /// use spark_ecs::ResMut;
    ///
    /// struct Counter(u32);
    ///
    /// fn tick(mut c: ResMut<Counter>) {
    ///     c.0 += 1;
    /// }
    ///
    /// let mut app = Application::new();
    /// app.add_resource(Counter(0))
    ///    .add_system(stages::UPDATE, tick);
    /// app.run_stage(stages::UPDATE);
    /// app.run_stage(stages::UPDATE);
    /// // Counter is now 2; the runner never has to wake up to drive UPDATE
    /// // explicitly in this PR.
    /// ```
    pub fn run_stage(&mut self, stage: &str) {
        if let Some(systems) = self.stages.get_mut(stage) {
            for system in systems {
                system(&self.world);
            }
        }
    }

    /// Installs the runner — the closure that takes the main thread
    /// after startup. Last call wins; one plugin (today
    /// `WindowPlugin`) installs it. The `FnOnce(Application)`
    /// signature is stable across M4, where the runner will use the
    /// supplied `Application` to drive per-frame schedules.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, EngineError};
    ///
    /// Application::new()
    ///     .set_runner(|_app: Application| -> Result<(), EngineError> { Ok(()) })
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn set_runner<R>(&mut self, runner: R) -> &mut Self
    where
        R: FnOnce(Application) -> Result<(), EngineError> + 'static,
    {
        self.runner = Some(Box::new(runner));
        self
    }

    /// Runs the startup phase, the STARTUP-stage systems, then the
    /// runner phase.
    ///
    /// 1. Drains every closure registered with
    ///    [`add_startup_system`](Self::add_startup_system) in
    ///    registration order, propagating the first error.
    /// 2. Runs every system on
    ///    [`stages::STARTUP`](crate::stages::STARTUP) once, in
    ///    registration order.
    /// 3. Hands an [`Application`] to the runner (typically the winit
    ///    event loop, which blocks until the window closes). With no
    ///    runner, returns `Ok(())` right after STARTUP.
    ///
    /// Takes `&mut self` so it can finish off a builder chain:
    /// `Application::new().add_plugin(_).run()`.
    ///
    /// # Errors
    ///
    /// Returns the first error any startup closure produced, or the
    /// runner's return value if startup succeeded. Closures convert
    /// their typed errors to [`EngineError`] via `?` before they reach
    /// this function. STARTUP-stage *systems* are infallible
    /// (`FnMut(&World)`) — failures must be surfaced through a
    /// resource or a startup closure instead.
    ///
    /// # Examples
    ///
    /// ```
    /// spark_core::Application::new().run().unwrap();
    /// ```
    pub fn run(&mut self) -> Result<(), EngineError> {
        let startup = std::mem::take(&mut self.startup);
        for system in startup {
            system()?;
        }

        self.run_stage(stages::STARTUP);

        if let Some(runner) = self.runner.take() {
            runner(std::mem::take(self))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_ecs::ResMut;

    struct Counter(u32);

    fn bump(mut c: ResMut<Counter>) {
        c.0 += 1;
    }

    #[test]
    fn run_auto_executes_startup_stage_systems() {
        let mut app = Application::new();
        app.add_resource(Counter(0))
            .add_system(stages::STARTUP, bump);
        app.run().unwrap();
        assert_eq!(app.world.resource::<Counter>().0, 1);
    }

    #[test]
    fn run_executes_startup_closures_before_startup_stage_systems() {
        let mut app = Application::new();
        app.add_resource(Counter(0));

        // Closure runs first; sees Counter == 0, bumps to 10.
        app.add_startup_system(|| Ok(())); // unrelated, just exercise the path
        app.add_system(stages::STARTUP, |mut c: ResMut<Counter>| {
            // System runs after closures; sets to 10.
            c.0 = 10;
        });
        app.run().unwrap();
        assert_eq!(app.world.resource::<Counter>().0, 10);
    }

    #[test]
    fn run_stage_update_runs_registered_systems_in_order() {
        let mut app = Application::new();
        app.add_resource(Counter(0))
            .add_system(stages::UPDATE, bump)
            .add_system(stages::UPDATE, bump);
        app.run_stage(stages::UPDATE);
        assert_eq!(app.world.resource::<Counter>().0, 2);
        app.run_stage(stages::UPDATE);
        assert_eq!(app.world.resource::<Counter>().0, 4);
    }

    #[test]
    fn run_stage_unknown_is_noop() {
        let mut app = Application::new();
        app.add_resource(Counter(7));
        app.run_stage("does-not-exist");
        assert_eq!(app.world.resource::<Counter>().0, 7);
    }

    #[test]
    fn run_with_no_runner_returns_after_startup() {
        // Regression: `run` must not hang or return Err when no runner
        // is installed, even after STARTUP systems fire.
        let mut app = Application::new();
        app.add_resource(Counter(0))
            .add_system(stages::STARTUP, bump);
        app.run().unwrap();
    }
}
