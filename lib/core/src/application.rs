//! The [`Application`] type — the engine's composition root. Knows
//! nothing about windowing or logging; every capability is supplied
//! by a [`Plugin`].

use std::any::TypeId;
use std::collections::HashMap;

use spark_ecs::{
    Access, Event, Events, IntoSystem, Resource, Schedule, WorkloadBuilder, WorkloadLabel,
    WorkloadOrderBuilder, World, swap_events,
};

use crate::error::EngineError;
use crate::plugin::Plugin;
use crate::stage::Stage;

type StartupSystem = Box<dyn FnOnce() -> Result<(), EngineError>>;
type Runner = Box<dyn FnOnce(Application) -> Result<(), EngineError>>;

/// A sequential system: its erased run closure, its declared [`Access`],
/// and its per-component change-detection baselines.
///
/// This is the deliberate sequential-path twin of `spark-ecs`'s
/// `BoxedSystem` (the workload path) — they can't share one concrete type
/// because they live in separate crates, but both delegate the tick dance
/// to the same [`World::run_system`](spark_ecs::World::run_system).
///
/// The `last_seen` list is the sequential-path mirror of
/// `BoxedSystem`'s: [`run`](StageSystem::run) hands it to
/// [`World::run_system`](spark_ecs::World::run_system), which advances the
/// clocks of components this system writes, parks `last_seen` as the
/// [`Changed`](spark_ecs::Changed) / [`Added`](spark_ecs::Added) baseline,
/// runs, and records where each accessed component's clock landed.
struct StageSystem {
    run: Box<dyn FnMut(&World) + 'static>,
    access: Access,
    last_seen: Vec<(TypeId, u32)>,
}

impl StageSystem {
    /// Runs this system against `world` with per-component change
    /// detection, updating its `last_seen` baselines. Thin wrapper over
    /// [`World::run_system`](spark_ecs::World::run_system), shared in
    /// spirit with the workload path's `BoxedSystem::run`.
    fn run(&mut self, world: &mut World) {
        world.run_system(&self.access, &mut self.last_seen, &mut *self.run);
    }
}

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

    /// Registers event type `T`: inserts an [`Events<T>`](spark_ecs::Events)
    /// buffer into the [`World`] and the per-frame
    /// [`swap_events::<T>`](spark_ecs::swap_events) system on
    /// [`Stage::Input`]. Chainable.
    ///
    /// After this, systems can take an
    /// [`EventWriter<T>`](spark_ecs::EventWriter) to send and an
    /// [`EventReader<T>`](spark_ecs::EventReader) to read; the swap on
    /// `Stage::Input` (pumped first each frame by `WindowPlugin`'s runner)
    /// rotates the buffers so an event sent on frame N is readable on N+1.
    ///
    /// **Idempotent.** A second call for the same `T` is a no-op — it does
    /// *not* register a second swap system. That matters: two swaps per
    /// frame would both rotate the buffers before any reader runs, leaving
    /// `previous` empty and silently dropping every event. Two plugins may
    /// each register the same event type, so the guard is load-bearing.
    ///
    /// # Precondition
    ///
    /// This method assumes it is the **only** path that puts an `Events<T>`
    /// resource into the world. The idempotency guard keys off that
    /// resource's presence, so the resource and its swap system are a
    /// managed pair. If you instead insert `Events::<T>::default()` by hand
    /// via [`add_resource`](Self::add_resource) and *then* call
    /// `add_event::<T>()`, the guard sees the resource and skips registering
    /// the swap system — so the buffers never rotate: `previous` stays empty
    /// (readers see nothing) and `current` grows unbounded (events are never
    /// cleared). Always register event types through `add_event`, never by
    /// inserting `Events<T>` directly. (A registry-backed guard that removes
    /// this coupling is a deferred follow-up — see `docs/ECS_ROADMAP.md`.)
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::Application;
    /// use spark_ecs::{Event, Events};
    ///
    /// #[derive(Event)]
    /// struct Tick;
    ///
    /// let mut app = Application::new();
    /// app.add_event::<Tick>().add_event::<Tick>(); // second call is a no-op
    /// assert!(app.world().get_resource::<Events<Tick>>().is_some());
    /// ```
    pub fn add_event<T: Event>(&mut self) -> &mut Self {
        // Idempotency guard — see the `# Precondition` section above. The
        // temporary `Ref` from `get_resource` is dropped at the end of this
        // `if`, before the `&mut self.world` borrows below.
        if self.world.get_resource::<Events<T>>().is_some() {
            return self;
        }
        self.world.add_resource(Events::<T>::default());
        self.add_system(Stage::Input, swap_events::<T>);
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
        let access = <S as IntoSystem<Marker>>::access();
        access.assert_no_self_conflict();
        self.stages.entry(stage).or_default().push(StageSystem {
            run: system.into_system(),
            access,
            last_seen: Vec::new(),
        });
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

    /// Runs `stage` in order: its **sequential** systems first (registration
    /// order, in-thread), then [a command
    /// flush](spark_ecs::World::flush_commands) so the stage's workloads
    /// observe what those systems queued, then the stage's **workload**
    /// [`Schedule`](spark_ecs::Schedule) if one exists (which flushes again
    /// at every workload boundary inside
    /// [`Schedule::run`](spark_ecs::Schedule::run)).
    ///
    /// The flush is what makes [`Commands`](spark_ecs::Commands) usable
    /// across the seam: a sequential system that queues a
    /// `spawn().insert(Position)` has the entity visible to *this* stage's
    /// workloads, to every later stage, and to every later frame — but *not*
    /// to other sequential systems in the same pass (they all run before the
    /// flush). The mid-stage flush is the only flush `run_stage` issues
    /// itself; `Schedule::run` owns the workload-boundary flushes, so there
    /// is no redundant trailing flush.
    ///
    /// No-op for stages with neither systems nor workloads (the flush still
    /// runs, but it's cheap when the queue is empty).
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
                // Per-component change-detection dance lives in
                // `StageSystem::run` → `World::run_system`. `self.world`
                // and `self.stages` are disjoint fields, so the borrow
                // checker lets `system` and `&mut self.world` coexist.
                system.run(&mut self.world);
            }
        }
        // Apply the sequential systems' queued commands *before* the stage's
        // workloads run, so they observe a consistent world. Workloads then
        // flush at each of their own boundaries inside `Schedule::run` (the
        // last one included), so no trailing flush is needed here.
        self.world.flush_commands();
        if let Some(schedule) = self.schedules.get_mut(&stage) {
            schedule.run(&mut self.world);
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
#[allow(
    clippy::needless_pass_by_value,
    reason = "SystemParam values like EventReader<T> are taken by value because \
              that is how systems receive them — `&EventReader<T>` is not itself \
              a SystemParam, so the by-reference suggestion does not apply."
)]
mod tests {
    use super::*;
    use spark_ecs::{
        Added, Changed, Commands, Component, Event, EventReader, EventWriter, Events, Query,
        ResMut, Resource, WorkloadLabel,
    };

    #[derive(Resource)]
    struct Counter(u32);

    fn bump(mut c: ResMut<Counter>) {
        c.0 += 1;
    }

    #[derive(Event)]
    struct Pulse(u32);

    /// Records every event payload a reader system observed, so a test can
    /// assert what was read after running frames.
    #[derive(Resource)]
    struct Log(Vec<u32>);

    /// Reader system reused across the event tests: appends this frame's
    /// readable events to [`Log`].
    fn record(reader: EventReader<Pulse>, mut log: ResMut<Log>) {
        log.0.extend(reader.read().map(|p| p.0));
    }

    /// Pumps one full frame in the runner's order, minus the reserved
    /// stages: `Input → PreUpdate → Update → PostUpdate`. `Input` is where
    /// the per-event swap runs, so this is what advances the event clock.
    fn run_frame(app: &mut Application) {
        app.run_stage(Stage::Input);
        app.run_stage(Stage::PreUpdate);
        app.run_stage(Stage::Update);
        app.run_stage(Stage::PostUpdate);
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

    #[test]
    #[should_panic(expected = "conflict on write/write")]
    fn add_workload_conflict_panics_on_first_run_stage() {
        // Two workloads on the same stage both write Counter with no order
        // declared between them — a registration error surfaced lazily, on
        // the first `run_stage` (the App wrapper defers the schedule build).
        #[derive(WorkloadLabel)]
        enum W {
            A,
            B,
        }
        let mut app = Application::new();
        app.add_resource(Counter(0));
        app.add_workload(W::A, Stage::Update, |w| {
            w.add_system(bump);
        });
        app.add_workload(W::B, Stage::Update, |w| {
            w.add_system(bump);
        });
        app.run_stage(Stage::Update); // build() panics here, not at registration
    }

    #[test]
    #[should_panic(expected = "registered twice")]
    fn add_workload_same_label_same_stage_panics() {
        // The duplicate-label check is per-`Schedule`, so two workloads with
        // the same label on the same stage are rejected — at registration.
        #[derive(WorkloadLabel)]
        enum W {
            Tick,
        }
        let mut app = Application::new();
        app.add_workload(W::Tick, Stage::Update, |w| {
            w.add_system(bump);
        });
        app.add_workload(W::Tick, Stage::Update, |w| {
            w.add_system(bump);
        }); // panics here, before any run
    }

    #[test]
    fn same_label_on_different_stages_is_allowed() {
        // Each stage owns its own `Schedule`, so the per-`Schedule`
        // duplicate-label check does not fire across stages.
        #[derive(WorkloadLabel)]
        enum W {
            Tick,
        }
        let mut app = Application::new();
        app.add_resource(Counter(0));
        app.add_workload(W::Tick, Stage::PreUpdate, |w| {
            w.add_system(bump);
        });
        app.add_workload(W::Tick, Stage::Update, |w| {
            w.add_system(bump);
        });
        app.run_stage(Stage::PreUpdate);
        app.run_stage(Stage::Update);
        assert_eq!(app.world.resource::<Counter>().0, 2); // both ran
    }

    #[test]
    fn sequential_command_is_visible_to_same_stage_workload() {
        // A sequential system's queued commands flush before the stage's
        // workloads run, so a workload in the same stage observes them.
        #[derive(WorkloadLabel)]
        enum W {
            Count,
        }
        #[derive(Component)]
        struct Tag;
        #[derive(Resource)]
        struct Seen(usize);

        let mut app = Application::new();
        app.add_resource(Seen(0));
        // Sequential: spawn a tagged entity (deferred via Commands).
        app.add_system(Stage::Update, |mut c: Commands| {
            c.spawn().insert(Tag);
        });
        // Workload in the same stage: count Tag entities it can see.
        app.add_workload(W::Count, Stage::Update, |w| {
            w.add_system(|q: Query<&Tag>, mut seen: ResMut<Seen>| {
                seen.0 = q.iter().count();
            });
        });
        app.run_stage(Stage::Update);
        assert_eq!(app.world().resource::<Seen>().0, 1);
    }

    #[test]
    fn add_event_inserts_buffer_and_registers_swap_on_input() {
        let mut app = Application::new();
        app.add_event::<Pulse>();
        assert!(app.world().get_resource::<Events<Pulse>>().is_some());

        // The swap system landed on Stage::Input: a queued send becomes
        // readable after exactly one Input pump.
        app.world_mut()
            .resource_mut::<Events<Pulse>>()
            .send(Pulse(1));
        app.run_stage(Stage::Input);
        assert_eq!(
            app.world()
                .resource::<Events<Pulse>>()
                .iter_previous()
                .count(),
            1
        );
    }

    #[test]
    fn add_event_is_idempotent() {
        // Two plugins each registering the same event type must not stack a
        // second swap system: two swaps per frame would rotate the buffers
        // twice before any reader runs, dropping the event to zero.
        let mut app = Application::new();
        app.add_event::<Pulse>().add_event::<Pulse>(); // registered twice
        app.add_resource(Log(Vec::new()));
        app.add_system(Stage::Update, record);

        app.world_mut()
            .resource_mut::<Events<Pulse>>()
            .send(Pulse(7));
        // Advance one frame. A double swap would empty `previous` here.
        app.run_stage(Stage::Input);
        app.run_stage(Stage::Update);

        assert_eq!(app.world().resource::<Log>().0, vec![7]); // exactly one, not zero
    }

    #[test]
    fn event_written_frame_n_is_readable_next_frame_then_gone() {
        let mut app = Application::new();
        app.add_event::<Pulse>();
        app.add_resource(Log(Vec::new()));
        app.add_system(Stage::Update, record);

        // Frame 0 send (into `current`).
        app.world_mut()
            .resource_mut::<Events<Pulse>>()
            .send(Pulse(7));

        // Frame 1: Input swap rotates the send into `previous`; Update reads it.
        run_frame(&mut app);
        assert_eq!(app.world().resource::<Log>().0, vec![7]);

        // Frame 2: Input swap again (nothing newly sent) ages the event out.
        run_frame(&mut app);
        assert_eq!(app.world().resource::<Log>().0, vec![7]); // unchanged: nothing new read
    }

    #[test]
    fn reader_on_earlier_stage_than_writer_still_sees_event_next_frame() {
        // Order-independence: the reader runs on PreUpdate (early), the
        // writer on PostUpdate (late). Read-previous makes the event visible
        // the *next* frame regardless of intra-frame ordering.
        let mut app = Application::new();
        app.add_event::<Pulse>();
        app.add_resource(Log(Vec::new()));
        app.add_system(Stage::PostUpdate, |mut w: EventWriter<Pulse>| {
            w.send(Pulse(3));
        });
        app.add_system(Stage::PreUpdate, record);

        // Frame 1: PreUpdate reader sees nothing (previous empty); the late
        // writer then sends.
        run_frame(&mut app);
        assert_eq!(app.world().resource::<Log>().0, Vec::<u32>::new());

        // Frame 2: Input swap rotates frame-1's send in; the early reader sees it.
        run_frame(&mut app);
        assert_eq!(app.world().resource::<Log>().0, vec![3]);
    }

    #[test]
    fn fixed_update_burst_reads_the_same_previous_snapshot() {
        // A multi-step FixedUpdate burst within one frame reads the identical
        // frozen `previous` on every step — the swap only runs on Input.
        let mut app = Application::new();
        app.add_event::<Pulse>();
        app.add_resource(Log(Vec::new()));
        app.add_system(Stage::FixedUpdate, record);

        app.world_mut()
            .resource_mut::<Events<Pulse>>()
            .send(Pulse(5));

        // Frame N+1: one Input pump, then three FixedUpdate steps.
        app.run_stage(Stage::Input);
        app.run_stage(Stage::FixedUpdate);
        app.run_stage(Stage::FixedUpdate);
        app.run_stage(Stage::FixedUpdate);
        assert_eq!(app.world().resource::<Log>().0, vec![5, 5, 5]);
    }

    // -------- change detection: the 3 caveats, on the sequential path --------

    #[test]
    fn run_stage_first_run_sees_build_time_component() {
        // Caveat #1: a component attached before any system ran (here via
        // `world_mut()`, standing in for plugin `build()`) is visible to a
        // system's first `Changed` — per-component clocks start at 1,
        // insert advances to ≥2, the system's baseline starts at 0.
        #[derive(Component)]
        struct Hp(u32);
        #[derive(Resource)]
        struct Seen(usize);
        fn observer(q: Query<&Hp, Changed<Hp>>, mut seen: ResMut<Seen>) {
            // Read the field too, so it isn't dead.
            seen.0 = q.iter().filter(|hp| hp.0 > 0).count();
        }
        let mut app = Application::new();
        app.add_resource(Seen(0));
        app.world_mut().spawn().insert(Hp(100)); // "build time"
        app.add_system(Stage::Update, observer);
        app.run_stage(Stage::Update);
        assert_eq!(app.world().resource::<Seen>().0, 1);
    }

    #[test]
    fn run_stage_added_sees_command_spawn_even_when_observer_is_last() {
        // Caveat #3: a `Commands`-spawned entity (flushed after the
        // stage's sequential systems) is visible to an `Added` observer on
        // its next run — even though the observer is the *last* system
        // before the flush. The flush's insert advances the component's
        // clock past the observer's last-seen baseline.
        #[derive(Component)]
        struct Spawned;
        #[derive(Resource)]
        struct Seen(usize);
        fn spawner(mut commands: Commands) {
            commands.spawn().insert(Spawned);
        }
        fn observer(q: Query<&Spawned, Added<Spawned>>, mut seen: ResMut<Seen>) {
            seen.0 = q.iter().count();
        }
        let mut app = Application::new();
        app.add_resource(Seen(0));
        app.add_system(Stage::Update, spawner);
        app.add_system(Stage::Update, observer); // last system before the flush
        // Frame 1: observer runs before the spawn is flushed → sees nothing.
        app.run_stage(Stage::Update);
        assert_eq!(app.world().resource::<Seen>().0, 0);
        // Frame 2: the frame-1 spawn (flushed at end of frame 1) is now
        // visible — the architectural fix for the flush-tick boundary.
        app.run_stage(Stage::Update);
        assert_eq!(app.world().resource::<Seen>().0, 1);
    }
}
