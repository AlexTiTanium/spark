# spark-core

The foundation crate of the Spark engine. Every other engine crate
(`spark-log`, `spark-window`, `spark-render`, …) sits on top of this
one. Brings four things together:

- **[`Application`]** — the composition root. Holds the ECS [`World`],
  the list of registered systems, and (optionally) the runner that
  takes the main thread after startup.
- **[`Plugin`]** — the trait every engine subsystem implements. A
  plugin is a *registrar*: it tells the `Application` what resources
  to insert, what systems to run, and (optionally) what runner to
  use.
- **[`stages`]** — named slots in the schedule (`STARTUP`, `UPDATE`,
  plus any custom names you introduce).
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
[`stages`]: stages/index.html
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

struct GameTime { elapsed: f32, dt: f32 }
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

## Systems: code that runs

A **system** is a regular Rust function. Its parameters tell the
engine what to inject when it's called:

```rust
use spark_core::{Application, stages};
use spark_ecs::{Res, ResMut};

struct GameTime { elapsed: f32, dt: f32 }
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
   .add_system(stages::UPDATE, integrate_time)
   .add_system(stages::UPDATE, high_score_alarm);
```

The engine figures out which resources each system needs from its
function signature and hands them in — no `app.get_resource::<…>()`
calls in your system body. This is the "Bevy-style" pattern: the
*function signature* is the interface.

Today, systems accept 0 to 4 [`SystemParam`] arguments of types
`Res<T>` and `ResMut<T>`. Queries over entities (the other half of
an ECS) land later as additional parameter types — same signature
mechanism.

> **`add_startup_system` vs `add_system(stages::STARTUP, …)`.** Both
> fire during the startup phase of `run()`, but they're not the same:
>
> - `add_startup_system(|| Ok(()))` — a fallible *closure* with no
>   parameters, runs *before* STARTUP-stage systems. Use it for
>   world-independent setup that can fail: installing a global
>   subscriber, opening a file, parsing a config.
> - `add_system(stages::STARTUP, my_fn)` — a regular system with
>   `Res`/`ResMut` params, runs *after* the closures. Use it for
>   initialising state that lives in the `World`.

[`SystemParam`]: ../spark_ecs/trait.SystemParam.html

## Stages: when systems run

A **stage** is a named slot in the schedule — just a `&'static str`.
Spark ships two:

- `stages::STARTUP` — fires once during `Application::run`, after
  every `add_startup_system` closure has finished.
- `stages::UPDATE` — fires every time you call
  `Application::run_stage(stages::UPDATE)`. The per-frame driver
  inside `WindowPlugin`'s event loop ticks it automatically in M3+.

You can introduce your own stages by writing a string constant. No
registry, no enum, no central list to update:

```rust
use spark_core::Application;
use spark_ecs::ResMut;

const FIXED_UPDATE: &str = "fixed_update";

struct Tick(u32);

fn bump(mut t: ResMut<Tick>) {
    t.0 += 1;
}

let mut app = Application::new();
app.add_resource(Tick(0))
   .add_system(FIXED_UPDATE, bump);

app.run_stage(FIXED_UPDATE);            // Tick = 1
app.run_stage(FIXED_UPDATE);            // Tick = 2
```

Systems on `STARTUP` fire automatically inside `run()`; systems on
`UPDATE` and any custom stage need a caller to invoke
`run_stage(name)` to drive them. Until M3 lands, that caller is *you*
(via the snippet above).

## The `run()` lifecycle

`Application::run` is the single entry point. Four phases, in order:

```text
   1. drain `add_startup_system` closures, in registration order
      └─ first Err short-circuits — `run` returns the error
   2. run every system on `stages::STARTUP` once, in registration
      order (these are infallible — `fn(&World) -> ()`)
   3. if a runner is installed, hand the `Application` to it
      └─ runner blocks until exit (typically winit's event loop)
   4. when the runner returns, `run` returns its Result
```

With no runner installed, `run` returns `Ok(())` right after step 2
— useful for headless tests that just want to verify startup fires
cleanly:

```rust
use spark_core::{Application, stages};
use spark_ecs::ResMut;

struct Counter(u32);

fn bump(mut c: ResMut<Counter>) {
    c.0 += 1;
}

let mut app = Application::new();
app.add_resource(Counter(0))
   .add_system(stages::STARTUP, bump);

app.run().unwrap();
// Counter is now 1. No runner ran — `run` returned right after
// STARTUP because nothing called `set_runner`.
```

## Runners: taking over the main thread

Most plugins just register startup closures and systems. A *runner*,
in contrast, takes the main thread once startup is done — useful
when you need to plug into a foreign event loop (OS windowing, an
async runtime, a server's main accept loop) that has its own "run
until done" call.

Install one with `Application::set_runner`:

```rust
use spark_core::{Application, EngineError};

Application::new()
    .set_runner(|_app: Application| -> Result<(), EngineError> {
        // …drive the application here. For a windowed game this is
        // where `winit::EventLoop::run_app(&mut handler)` goes.
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
is stable through M4, when the runner will use its supplied
`Application` to drive per-frame schedules (`UPDATE`, eventual
`FIXED_UPDATE` / `RENDER`) inside the event loop.

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
