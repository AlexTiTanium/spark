# spark-core

The foundation crate of the Spark engine. Every other engine crate
(`spark-log`, `spark-window`, `spark-render`, …) sits on top of this
one. Brings four things together:

- **[`Application`]** — the composition root. Holds the ECS [`World`],
  the registered **sequential systems** and **parallel-capable
  workloads** per stage, and (optionally) the runner that takes the main
  thread after startup.
- **[`Plugin`]** — the trait every engine subsystem implements. A
  plugin is a *registrar*: it tells the `Application` what resources
  to insert, what systems to run, and (optionally) what runner to
  use.
- **[`Stage`]** — the closed enum of per-frame phases (`Startup`,
  `PreUpdate`, `Update`, `PostUpdate`, …) in a fixed execution order.
  `run_stage` flushes pending
  [`Commands`](../spark_ecs/struct.Commands.html) after a stage's
  sequential systems and at every workload boundary within it.
- **[`EngineError`]** — the erased error type that flows through every
  plugin seam (a re-export of [`anyhow::Error`]).

ECS items (`World`, `Res`, `ResMut`, `IntoSystem`, `SystemParam`) are
**not** re-exported from this crate — import them from
[`spark-ecs`](../spark_ecs/) directly. Any crate (engine or game) that
writes systems adds `spark-ecs` to its `Cargo.toml` alongside
`spark-core`.

> **What does "composition root" mean?** It's the one place in a
> program that wires everything together. In Spark that's your
> `fn main` — you build an `Application`, add plugins, and call
> `.run()`. Plugins never wire themselves; the responsibility lives
> at the top, where it's visible.

[`Application`]: struct.Application.html
[`Plugin`]: trait.Plugin.html
[`Stage`]: enum.Stage.html
[`EngineError`]: type.EngineError.html
[`World`]: ../spark_ecs/struct.World.html

## Setup

The shortest possible Spark program does nothing and exits:

```rust
spark_core::Application::new().run().unwrap();
```

In practice you'll write or register plugins, and they'll register
their own systems. Here's a one-file plugin that prints a banner at
startup:

```rust
use spark_core::{Application, Plugin};

struct HelloPlugin;

impl Plugin for HelloPlugin {
    fn build(&self, app: &mut Application) {
        app.add_startup_system(|| {
            println!("hello from a Spark plugin");
            Ok(())
        });
    }
}

Application::new()
    .add_plugin(HelloPlugin)
    .run()
    .unwrap();
```

`add_plugin` calls `plugin.build(&mut app)` immediately — there's no
separate "initialise" pass. A plugin *is* a function that registers
things; that's the whole abstraction.

For real-world wiring you'd typically register
[`LogPlugin`](../spark_log/) (subscriber install) and
[`WindowPlugin`](../spark_window/) (OS window + event loop) before
any of your own plugins. See each crate's README for details.

## Plugins

A plugin is anything that implements the [`Plugin`] trait — one
method, `build(&self, app: &mut Application)`. The plugin uses
`&mut Application` to register what it wants the engine to do; it
doesn't do real work inside `build`.

```rust
use spark_core::{Application, Plugin};
use spark_ecs::Resource;

#[derive(Resource)]
struct ClearColor((f32, f32, f32));

struct GraphicsPlugin {
    clear: (f32, f32, f32),
}

impl Plugin for GraphicsPlugin {
    fn build(&self, app: &mut Application) {
        // Stash config as a resource for systems to read.
        app.add_resource(ClearColor(self.clear));

        // Schedule fallible one-time setup.
        app.add_startup_system(|| {
            // …open a wgpu device, load shaders, etc.
            Ok(())
        });
    }
}

let _app = Application::new().add_plugin(GraphicsPlugin {
    clear: (0.1, 0.1, 0.2),
});
```

Why this pattern? Plugins isolate each subsystem behind a single
type. Adding `WindowPlugin` to your `Application` is one line, and
swapping it for a headless renderer is also one line — the rest of
your game doesn't know which one is installed.

## Resources: singleton state

A **resource** is one value of one type, stored in the engine's
[`World`]. Use it for anything that's globally accessible:
configuration, the current frame number, the input state, the
renderer handle. Every type can be a resource exactly once; a second
insert of the same type replaces the first.

```rust
use spark_core::Application;
use spark_ecs::Resource;

#[derive(Resource)]
struct GameTime { elapsed: f32, dt: f32 }
#[derive(Resource)]
struct Score(u32);

let _app = Application::new()
    .add_resource(GameTime { elapsed: 0.0, dt: 0.016 })
    .add_resource(Score(0));
```

Systems read and write resources via `Res<T>` (read-only) and
`ResMut<T>` (read-write) — see *Systems* below.

> **Resources, not globals.** They look like globals (one per type,
> always available), but they live inside the `Application` you
> built. Two `Application`s in the same process keep two independent
> sets of resources. That's what makes tests trivial to write.

### Escape hatch: `app.world_mut()`

`add_resource` wires one value at a time, which is perfect when a
plugin has a handful of singletons to seed. When a plugin instead
needs to *pre-populate* the [`World`] with many entities and
components — loading a level, seeding a tile grid, instantiating a
fixture for tests — reach for [`Application::world_mut`] to get a
plain `&mut World`:

```rust
use spark_core::{Application, Plugin};
use spark_ecs::Component;

#[derive(Component)]
struct Tile { x: i32, y: i32 }
#[derive(Component)]
struct Walkable;

struct LevelPlugin;
impl Plugin for LevelPlugin {
    fn build(&self, app: &mut Application) {
        let world = app.world_mut();
        for y in 0..10 {
            for x in 0..10 {
                world.spawn().insert(Tile { x, y }).insert(Walkable);
            }
        }
    }
}

Application::new().add_plugin(LevelPlugin).run().unwrap();
```

Use it from inside `Plugin::build` for the registration path, not
from inside a running system — systems should pull state through
`Res<T>` / `ResMut<T>` / `Query<…>` / `Commands` system params.

[`Application::world_mut`]: struct.Application.html#method.world_mut

## Systems: code that runs

A **system** is a regular Rust function. Its parameters tell the
engine what to inject when it's called:

```rust
use spark_core::{Application, Stage};
use spark_ecs::{Res, ResMut, Resource};

#[derive(Resource)]
struct GameTime { elapsed: f32, dt: f32 }
#[derive(Resource)]
struct Score(u32);

// `Res<T>` borrows the resource immutably, `ResMut<T>` mutably.
fn integrate_time(mut time: ResMut<GameTime>) {
    time.elapsed += time.dt;
}

fn high_score_alarm(time: Res<GameTime>, score: Res<Score>) {
    if time.elapsed > 60.0 && score.0 > 100 {
        // …emit a high-score event, print a banner, etc.
    }
}

let mut app = Application::new();
app.add_resource(GameTime { elapsed: 0.0, dt: 0.016 })
   .add_resource(Score(42))
   .add_system(Stage::Update, integrate_time)
   .add_system(Stage::Update, high_score_alarm);
```

The engine figures out which resources each system needs from its
function signature and hands them in — no `app.get_resource::<…>()`
calls in your system body. This is the "Bevy-style" pattern: the
*function signature* is the interface.

Today, systems accept 0 to 4 [`SystemParam`] arguments of types
`Res<T>` and `ResMut<T>`. Queries over entities (the other half of
an ECS) land later as additional parameter types — same signature
mechanism.

> **`add_startup_system` vs `add_system(Stage::Startup, …)`.** Both
> fire during the startup phase of `run()`, but they're not the same:
>
> - `add_startup_system(|| Ok(()))` — a fallible *closure* with no
>   parameters, runs *before* Startup-stage systems. Use it for
>   world-independent setup that can fail: installing a global
>   subscriber, opening a file, parsing a config.
> - `add_system(Stage::Startup, my_fn)` — a regular system with
>   `Res`/`ResMut` params, runs *after* the closures. Use it for
>   initialising state that lives in the `World`.

[`SystemParam`]: ../spark_ecs/trait.SystemParam.html

## Workloads: parallel-capable groups

`add_system` is **sequential** — its systems run in registration order, in
the calling thread. When you instead want the scheduler to *extract
parallelism*, register a **workload** with
`add_workload(label, stage, |w| { … })`. A workload is a named group whose
systems the scheduler partitions into access-disjoint **batches** (a
sequential batch walk today, Rayon-backed at M4). The two are separate
mechanisms sharing only the `Stage`; within a stage, sequential systems run
first, then the stage's workloads.

```rust
use spark_core::{Application, Stage};
use spark_ecs::{ResMut, Resource, WorkloadLabel};

// One enum per subsystem; each variant names a workload.
#[derive(WorkloadLabel)]
enum Grid { Supply, Distribute }

#[derive(Resource)]
struct Power(u32);

fn collect(mut p: ResMut<Power>) { p.0 += 1; }
fn route(mut p: ResMut<Power>) { p.0 += 1; }

let mut app = Application::new();
app.add_resource(Power(0));
app.add_workload(Grid::Supply, Stage::Update, |w| {
    w.add_system(collect);
});
// Both write Power, so declare the order; workloads order against each
// other by label on the builder `add_workload` returns.
app.add_workload(Grid::Distribute, Stage::Update, |w| {
    w.add_system(route);
})
.after(Grid::Supply);

app.run_stage(Stage::Update);
assert_eq!(app.world().resource::<Power>().0, 2);
```

Inside the closure, `w.add_system(fn)` returns a handle for
`.after(handle)` / `.before(handle)` / `.any_order_with(handle)`, and
`w.add_systems((a, b, c))` adds an unordered group. Two systems (or two
workloads) that conflict on the same data with **no** order declared are a
registration error, surfaced on the first `run_stage` for that stage —
declare an order, or assert commutativity with `.any_order_with`. The full
workload API lives in [`spark-ecs`](../spark_ecs/).

## Stages: when systems run

A **stage** is one phase of the frame. The stages are the variants of a
single closed enum, [`Stage`], and the engine runs them in a fixed
order. Spark defines eight; four are driven automatically today:

- `Stage::Startup` — fires once during `Application::run`, after every
  `add_startup_system` closure has finished.
- `Stage::First` — the very first per-frame stage. *(Reserved — defined
  now, nothing drives it yet.)*
- `Stage::PreUpdate` — input gather, time tick, anything that prepares
  state the rest of the frame consumes.
- `Stage::FixedUpdate` — fixed-timestep simulation, meant to run N
  times per frame off an accumulator. *(Reserved — not yet auto-driven.)*
- `Stage::Update` — main per-frame stage. The bulk of game logic
  (movement, AI, spawning, despawning) lives here.
- `Stage::PostUpdate` — cleanup and bookkeeping that should run after
  `Update`'s commands have flushed and the world has settled.
- `Stage::Render` — build draw lists, submit GPU work. *(Reserved — not
  yet auto-driven.)*
- `Stage::Last` — the very last per-frame stage. *(Reserved — not yet
  auto-driven.)*

`WindowPlugin`'s runner ticks `PreUpdate → Update → PostUpdate` on every
`RedrawRequested`; `Startup` runs once inside `run()`. The reserved
stages are defined so call sites (and the editor's future stage view)
can name them — their automatic drivers land with the scheduler. With no
window driver you can tick any stage yourself with `app.run_stage(...)`
— useful for headless tests, and the only way the reserved stages run
today:

```rust
use spark_core::{Application, Stage};
use spark_ecs::{ResMut, Resource};

#[derive(Resource)]
struct Tick(u32);

fn bump(mut t: ResMut<Tick>) {
    t.0 += 1;
}

let mut app = Application::new();
app.add_resource(Tick(0))
   .add_system(Stage::FixedUpdate, bump);

app.run_stage(Stage::FixedUpdate);      // Tick = 1
app.run_stage(Stage::FixedUpdate);      // Tick = 2
```

Each `run_stage` call **runs the stage's sequential systems in registration
order, then flushes pending
[`Commands`](../spark_ecs/struct.Commands.html) into the world, then runs
that stage's parallel-capable workloads (if any)** — which flush again at
every workload boundary. A `commands.spawn().insert(…)` queued by a
sequential system in `Update` is visible to that stage's workloads, to
`PostUpdate` of the same frame, and to every system in every later frame.

> **Why a closed enum, not string labels?** There is exactly one frame
> timeline, so there is one shared set of phases. A closed enum makes a
> `match` over stages exhaustive and turns a misspelled stage into a
> *compile error* — rather than a system silently registered to a slot
> nothing ever runs, which is what a `&'static str` label allowed. (The
> variants are *listed* in run order for readability; the order itself
> comes from the `run_stage` call sequence, not from the enum.) A subsystem that needs
> its own internal ordering groups related systems into a *workload* inside
> a stage (via [`add_workload`](struct.Application.html#method.add_workload)),
> not by inventing a new global stage.

## The `run()` lifecycle

`Application::run` is the single entry point. Four phases, in order:

```text
   1. drain `add_startup_system` closures, in registration order
      └─ first Err short-circuits — `run` returns the error
   2. run `Stage::Startup`: its sequential systems (registration
      order), then any Startup-stage workloads
   3. if a runner is installed, hand the `Application` to it
      └─ runner blocks until exit (typically winit's event loop)
   4. when the runner returns, `run` returns its Result
```

With no runner installed, `run` returns `Ok(())` right after step 2
— useful for headless tests that just want to verify startup fires
cleanly:

```rust
use spark_core::{Application, Stage};
use spark_ecs::{ResMut, Resource};

#[derive(Resource)]
struct Counter(u32);

fn bump(mut c: ResMut<Counter>) {
    c.0 += 1;
}

let mut app = Application::new();
app.add_resource(Counter(0))
   .add_system(Stage::Startup, bump);

app.run().unwrap();
// Counter is now 1. No runner ran — `run` returned right after the
// Startup stage because nothing called `set_runner`.
```

## Runners: taking over the main thread

Most plugins just register startup closures and systems. A *runner*,
in contrast, takes the main thread once startup is done — useful
when you need to plug into a foreign event loop (OS windowing, an
async runtime, a server's main accept loop) that has its own "run
until done" call.

Install one with `Application::set_runner`:

```rust
use spark_core::{Application, EngineError, Stage};

Application::new()
    .set_runner(|mut app: Application| -> Result<(), EngineError> {
        // …drive the application here. For a windowed game this is
        // where `winit::EventLoop::run_app(&mut handler)` goes, and
        // the handler calls `app.run_stage(Stage::Update)` on every
        // RedrawRequested. For tests, a tick-N loop suffices:
        for _ in 0..3 {
            app.run_stage(Stage::PreUpdate);
            app.run_stage(Stage::Update);
            app.run_stage(Stage::PostUpdate);
        }
        Ok(())
    })
    .run()
    .unwrap();
```

Two things to know:

- **`set_runner` is last-write-wins.** Only one plugin should install
  a runner. Today that's [`WindowPlugin`](../spark_window/); tomorrow
  it might be a server runtime or a headless replay driver instead.
- **The runner owns the `Application`.** The closure receives the
  built `Application` by value, so any resources, systems, or
  schedule it wants to drive afterwards have to live inside the
  closure. That ownership transfer is what lets winit's `run_app`
  take the main thread without `Application` outliving it on the
  caller's side.

The closure's signature — `FnOnce(Application) -> Result<(), EngineError>` —
is stable: M3's `WindowPlugin` uses it to own the `Application` and
tick `PreUpdate → Update → PostUpdate` on every winit redraw.

## Errors

Every plugin seam — and `fn main` — speaks `EngineError`. It's
aliased from [`anyhow::Error`], which means any typed error
(`WindowError`, `LogError`, …) converts to it automatically via `?`
thanks to anyhow's blanket `From<E: std::error::Error + Send + Sync + 'static>`:

```rust,ignore
fn install() -> Result<(), spark_core::EngineError> {
    failable_call()?;                  // returns Result<_, SomeTypedError>
    Ok(())                             // typed error → EngineError via `?`
}
```

The convention across the codebase:

- **Library crates** (`spark-log`, `spark-window`, …) define their
  own typed error enum with [`thiserror`](https://docs.rs/thiserror)
  (`LogError`, `WindowError`, …).
- **At the plugin → application seam** (returning from a startup
  closure or runner closure), use `?` and the typed error becomes
  `EngineError` automatically.
- **`fn main`** returns `Result<(), spark_core::EngineError>` — or
  calls `.unwrap()` in toy code.

You never construct an `EngineError` directly; the conversion through
`?` and anyhow's blanket `From` impl does the work for you.

## Where this crate fits

```text
                 ┌─────────────────────┐
                 │  src/ (game binary) │
                 └──────────┬──────────┘
                            │  depends on
              ┌─────────────┴─────────────┐
              ▼             ▼             ▼
       spark-log    spark-window    other engine crates
              │             │             │
              └─────────────┴─────────────┘
                            │  all depend on
                            ▼
                       spark-core          ◀── this crate
                            │  depends on
                            ▼
                        spark-ecs          ◀── stdlib only
```

`spark-core` is the only crate that sits *above* `spark-ecs` and
*below* every other engine crate. It depends on `spark-ecs` so
`Application` can embed a [`World`]; crates above that need ECS items
(`World`, `Res`, `ResMut`, `IntoSystem`, `SystemParam`) add a direct
`spark-ecs` dependency rather than going through `spark-core`. See
[`docs/PLAN.md`](../../docs/PLAN.md) for the full module dependency
graph and milestone plan.
