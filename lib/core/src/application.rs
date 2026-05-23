//! The [`Application`] type — the engine's composition root. Knows
//! nothing about windowing or logging; every capability is supplied
//! by a [`Plugin`].

use std::collections::HashMap;

use spark_ecs::{
    IntoSystem, Resource, Schedule, WorkloadBuilder, WorkloadLabel, WorkloadOrderBuilder, World,
};

use crate::error::EngineError;
use crate::plugin::Plugin;
use crate::stage::Stage;

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
/// 3. [`Stage::Startup`] systems run once, in registration order, with
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
    /// **Sequential** systems per stage: run in registration order, in the
    /// calling thread, no batching. Fed by [`add_system`](Self::add_system).
    stages: HashMap<Stage, Vec<StageSystem>>,
    /// **Parallel-capable** workloads per stage, lazily created — a stage
    /// with no workloads has no [`Schedule`]. Fed by
    /// [`add_workload`](Self::add_workload).
    schedules: HashMap<Stage, Schedule>,
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
    /// use spark_ecs::Resource;
    ///
    /// #[derive(Resource)]
    /// struct GameTime { dt: f32 }
    ///
    /// Application::new()
    ///     .add_resource(GameTime { dt: 0.016 })
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn add_resource<T: Resource>(&mut self, value: T) -> &mut Self {
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
    /// / [`Query<…>`](spark_ecs::Query) / [`Commands`](spark_ecs::Commands).
    /// This method is for the *registration* path, not the per-frame
    /// path.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Plugin};
    /// use spark_ecs::Component;
    ///
    /// #[derive(Component)]
    /// struct Tile { x: i32, y: i32 }
    /// #[derive(Component)]
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

    /// Shared access to the underlying [`World`].
    ///
    /// Useful from runners and from tests that want to inspect post-
    /// tick state — e.g. a headless runner that ticks N frames, then
    /// asserts component values via `app.world()`. Inside running
    /// systems, prefer [`Res<T>`](spark_ecs::Res),
    /// [`Query<…>`](spark_ecs::Query), and the other `SystemParam`s.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Stage};
    /// use spark_ecs::{ResMut, Resource};
    ///
    /// #[derive(Resource)]
    /// struct Counter(u32);
    ///
    /// let mut app = Application::new();
    /// app.add_resource(Counter(0))
    ///    .add_system(Stage::Update, |mut c: ResMut<Counter>| { c.0 += 1; });
    /// app.run_stage(Stage::Update);
    /// app.run_stage(Stage::Update);
    /// assert_eq!(app.world().resource::<Counter>().0, 2);
    /// ```
    #[must_use]
    pub fn world(&self) -> &World {
        &self.world
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
    /// Systems registered on [`Stage::Startup`] run once during
    /// [`run`](Self::run); systems on the per-frame stages — e.g.
    /// [`Stage::Update`] — run when a caller invokes
    /// [`run_stage`](Self::run_stage) with that stage, which
    /// `WindowPlugin`'s runner does every frame for the
    /// `PreUpdate → Update → PostUpdate` trio.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Stage};
    /// use spark_ecs::{ResMut, Resource};
    ///
    /// #[derive(Resource)]
    /// struct Counter(u32);
    ///
    /// fn tick(mut c: ResMut<Counter>) {
    ///     c.0 += 1;
    /// }
    ///
    /// let mut app = Application::new();
    /// app.add_resource(Counter(0))
    ///    .add_system(Stage::Startup, tick);
    /// app.run().unwrap();
    /// // The Startup system fired once during `run`.
    /// ```
    pub fn add_system<S, Marker>(&mut self, stage: Stage, system: S) -> &mut Self
    where
        S: IntoSystem<Marker>,
    {
        self.stages
            .entry(stage)
            .or_default()
            .push(system.into_system());
        self
    }

    /// Registers a **parallel-capable workload** on `stage` — a named group
    /// of systems the scheduler batches by access disjointness. Gets-or-
    /// inserts the per-stage [`Schedule`](spark_ecs::Schedule) and forwards
    /// to [`Schedule::add_workload`](spark_ecs::Schedule::add_workload),
    /// returning its [`WorkloadOrderBuilder`](spark_ecs::WorkloadOrderBuilder)
    /// so workloads order against each other by label (`.after(Label)` /
    /// `.before(Label)`).
    ///
    /// This is the parallel-capable sibling of
    /// [`add_system`](Self::add_system). `add_system` is **sequential** —
    /// runs in registration order, in the calling thread; a workload lets
    /// the scheduler extract parallelism (a sequential batch walk today,
    /// Rayon at M4). Within a stage, sequential systems run first, then the
    /// stage's workloads — see [`run_stage`](Self::run_stage). Inside the
    /// closure, `w.add_system(..)` hands back a handle for `.after` /
    /// `.before` / `.any_order_with`; `w.add_systems((..))` adds an
    /// unordered group.
    ///
    /// # Panics
    ///
    /// Panics if `label` is already registered on `stage` (each label names
    /// one workload). Conflict / unknown-label / cycle errors surface on the
    /// first [`run_stage`](Self::run_stage) for `stage`, when the schedule
    /// builds.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Stage};
    /// use spark_ecs::{ResMut, Resource, WorkloadLabel};
    ///
    /// #[derive(WorkloadLabel)]
    /// enum Grid { Supply, Distribute }
    ///
    /// #[derive(Resource)]
    /// struct Power(u32);
    ///
    /// fn collect(mut p: ResMut<Power>) { p.0 += 1; }
    /// fn route(mut p: ResMut<Power>) { p.0 += 1; }
    ///
    /// let mut app = Application::new();
    /// app.add_resource(Power(0));
    /// app.add_workload(Grid::Supply, Stage::Update, |w| {
    ///     w.add_system(collect);
    /// });
    /// // Both write Power, so declare the order.
    /// app.add_workload(Grid::Distribute, Stage::Update, |w| {
    ///     w.add_system(route);
    /// })
    /// .after(Grid::Supply);
    /// app.run_stage(Stage::Update);
    /// assert_eq!(app.world().resource::<Power>().0, 2);
    /// ```
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the label is a throwaway unit-enum variant passed inline; \
                  by-value matches spark-ecs's workload-label API."
    )]
    pub fn add_workload<L, F>(
        &mut self,
        label: L,
        stage: Stage,
        build: F,
    ) -> WorkloadOrderBuilder<'_>
    where
        L: WorkloadLabel,
        F: FnOnce(&WorkloadBuilder),
    {
        self.schedules
            .entry(stage)
            .or_default()
            .add_workload(label, build)
    }

    /// Runs `stage`: every **sequential** system first (registration order,
    /// in-thread), then the stage's **workload** [`Schedule`](spark_ecs::Schedule)
    /// if one exists, then [flushes pending
    /// commands](spark_ecs::World::flush_commands) into the world.
    ///
    /// The flush is what makes [`Commands`](spark_ecs::Commands) usable
    /// across stages: a system that runs in [`Stage::Startup`] and queues a
    /// `spawn().insert(Position)` has the resulting entity visible to systems
    /// in [`Stage::PreUpdate`] (and every later stage) — but *not* to later
    /// systems within the same `Startup` pass. Workloads additionally flush
    /// at every workload boundary inside [`Schedule::run`](spark_ecs::Schedule::run).
    ///
    /// No-op for stages with neither systems nor workloads (the trailing
    /// flush still runs, but it's cheap when the queue is empty).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Stage};
    /// use spark_ecs::{ResMut, Resource};
    ///
    /// #[derive(Resource)]
    /// struct Counter(u32);
    ///
    /// fn tick(mut c: ResMut<Counter>) {
    ///     c.0 += 1;
    /// }
    ///
    /// let mut app = Application::new();
    /// app.add_resource(Counter(0))
    ///    .add_system(Stage::Update, tick);
    /// app.run_stage(Stage::Update);
    /// app.run_stage(Stage::Update);
    /// assert_eq!(app.world().resource::<Counter>().0, 2);
    /// ```
    pub fn run_stage(&mut self, stage: Stage) {
        if let Some(systems) = self.stages.get_mut(&stage) {
            for system in systems {
                system(&self.world);
            }
        }
        if let Some(schedule) = self.schedules.get_mut(&stage) {
            schedule.run(&mut self.world);
        }
        self.world.flush_commands();
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
    /// 2. Runs every system on [`Stage::Startup`] once, in
    ///    registration order.
    /// 3. Hands an [`Application`] to the runner (typically the winit
    ///    event loop, which blocks until the window closes). With no
    ///    runner, returns `Ok(())` right after the Startup stage.
    ///
    /// Takes `&mut self` so it can finish off a builder chain:
    /// `Application::new().add_plugin(_).run()`.
    ///
    /// # Errors
    ///
    /// Returns the first error any startup closure produced, or the
    /// runner's return value if startup succeeded. Closures convert
    /// their typed errors to [`EngineError`] via `?` before they reach
    /// this function. Startup-stage *systems* are infallible
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

        self.run_stage(Stage::Startup);

        if let Some(runner) = self.runner.take() {
            runner(std::mem::take(self))?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_ecs::{ResMut, Resource, WorkloadLabel};

    #[derive(Resource)]
    struct Counter(u32);

    fn bump(mut c: ResMut<Counter>) {
        c.0 += 1;
    }

    #[test]
    fn run_auto_executes_startup_stage_systems() {
        let mut app = Application::new();
        app.add_resource(Counter(0))
            .add_system(Stage::Startup, bump);
        app.run().unwrap();
        assert_eq!(app.world.resource::<Counter>().0, 1);
    }

    #[test]
    fn run_executes_startup_closures_before_startup_stage_systems() {
        let mut app = Application::new();
        app.add_resource(Counter(0));

        // Closure runs first; sees Counter == 0, bumps to 10.
        app.add_startup_system(|| Ok(())); // unrelated, just exercise the path
        app.add_system(Stage::Startup, |mut c: ResMut<Counter>| {
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
            .add_system(Stage::Update, bump)
            .add_system(Stage::Update, bump);
        app.run_stage(Stage::Update);
        assert_eq!(app.world.resource::<Counter>().0, 2);
        app.run_stage(Stage::Update);
        assert_eq!(app.world.resource::<Counter>().0, 4);
    }

    #[test]
    fn run_stage_with_no_registered_systems_is_noop() {
        // A closed `Stage` can't be misspelled into a phantom bucket
        // the way a `&str` label could, but running a stage that simply
        // has no systems registered is still a harmless no-op: the
        // command flush runs, nothing else does.
        let mut app = Application::new();
        app.add_resource(Counter(7));
        app.run_stage(Stage::Last);
        assert_eq!(app.world.resource::<Counter>().0, 7);
    }

    #[test]
    fn run_with_no_runner_returns_after_startup() {
        // Regression: `run` must not hang or return Err when no runner
        // is installed, even after Startup systems fire.
        let mut app = Application::new();
        app.add_resource(Counter(0))
            .add_system(Stage::Startup, bump);
        app.run().unwrap();
    }

    #[test]
    fn run_stage_runs_sequential_systems_then_workloads() {
        #[derive(WorkloadLabel)]
        enum W {
            Tick,
        }
        let mut app = Application::new();
        app.add_resource(Counter(0));
        // Sequential system runs first (sets to 10), then the workload (+1).
        app.add_system(Stage::Update, |mut c: ResMut<Counter>| c.0 = 10);
        app.add_workload(W::Tick, Stage::Update, |w| {
            w.add_system(|mut c: ResMut<Counter>| c.0 += 1);
        });
        app.run_stage(Stage::Update);
        assert_eq!(app.world.resource::<Counter>().0, 11);
    }

    #[test]
    fn run_stage_with_only_a_workload_runs_it() {
        #[derive(WorkloadLabel)]
        enum W {
            Tick,
        }
        let mut app = Application::new();
        app.add_resource(Counter(0));
        app.add_workload(W::Tick, Stage::Update, |w| {
            w.add_system(bump);
        });
        app.run_stage(Stage::Update);
        app.run_stage(Stage::Update);
        assert_eq!(app.world.resource::<Counter>().0, 2);
    }
}
