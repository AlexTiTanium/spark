# spark-window

The Spark engine's window plugin. Opens a single OS window (built on
[`winit`]) and drives the platform event loop on the main thread,
emitting a structured `tracing` event for every OS interaction.

> **What does a window plugin do?** It owns the connection between
> your application and the operating system's windowing layer —
> opens a window, listens for resize / focus / keyboard / mouse /
> close events, and turns each one into a log line (and, in later
> milestones, an ECS event). With `LogPlugin` registered before this
> one, you can watch every OS interaction in your terminal.

> **Main thread, blocking call.** `WindowPlugin`'s runner — and the
> free [`run`] function it delegates to — take over the main thread
> until the user closes the window. This is a `winit` requirement on
> macOS and Windows, not a Spark choice. Anything that needs to run
> in parallel goes on other threads.

[`winit`]: https://docs.rs/winit
[`run`]: fn.run.html

## Setup

`WindowPlugin` opens the window. Register it after `LogPlugin` so the
OS-event log lines it emits actually reach your terminal:

```rust,no_run
use spark_core::Application;
use spark_window::WindowPlugin;

Application::new()
    // .add_plugin(LogPlugin)               ← register first (see spark-log)
    .add_plugin(WindowPlugin::default())
    .run()
    .unwrap();
```

The defaults open a resizable **1280×720** window titled **"Spark"**.
Once the user closes it, `Application::run` returns `Ok(())` and your
program exits normally.

## Configuring the window

[`WindowConfig`] is a plain struct with chainable builders. Pass it
inline as the `config` field of `WindowPlugin` — it reads top-to-
bottom and keeps the wiring in one expression:

```rust
use spark_core::Application;
use spark_window::{WindowConfig, WindowPlugin};

let _app = Application::new().add_plugin(WindowPlugin {
    config: WindowConfig::default()
        .with_title("Spark")
        .with_size(1280, 720)
        .with_resizable(true),
});
```

The defaults (`WindowConfig::default()`):

| Field | Default | Notes |
|-|-|-|
| `title` | `"Spark"` | Text shown in the OS title bar. |
| `size` | `(1280, 720)` | Initial inner size, in **logical** pixels. |
| `resizable` | `true` | Whether the user can drag the borders. |

> **Logical vs physical pixels.** `size` is in *logical* pixels — a
> DPI-independent unit. On a 2× ("retina"/4K) display, requesting
> `(1280, 720)` becomes `2560 × 1440` physical pixels under the hood.
> You almost always want logical units; the OS handles the scaling.

`WindowConfig` is `#[non_exhaustive]`, so always build it via
`default()` + `with_*` rather than a struct literal — that keeps
future fields additive.

[`WindowConfig`]: struct.WindowConfig.html

## What ends up in the logs

Every OS event the window cares about is emitted as a `tracing`
event. Run with `RUST_LOG=spark_window=debug cargo run -p spark` to
see most of them; `trace` for the noisy ones.

| Level | Event | When |
|-|-|-|
| `info` | `window created` | First-time window construction. Fields: `title`, `requested_size`, `actual_size`, `scale_factor`. |
| `info` | `window resized` | User drags the border or system resizes. `minimised=true` when width or height is zero. |
| `info` | `window focus changed` | Window gains or loses keyboard focus (field: `focused=true/false`). |
| `info` | `window scale factor changed` | DPI / scaling changed (e.g. dragged to a different monitor). |
| `info` | `close requested; exiting event loop` | User clicked the close button. `run` returns shortly after. |
| `debug` | `keyboard input` | Each press / release. Fields: `state`, `key`. |
| `debug` | `mouse input` | Each press / release. Fields: `state`, `button`. |
| `trace` | `cursor moved` | High-frequency cursor position updates. Hidden unless you ask for `trace`. |

Lifecycle bookends surface at `info` too:

```text
INFO spark-window starting event loop version="0.1.0"
… (events while running) …
INFO spark-window event loop exited
```

If you only want to see this crate's logs, scope the filter:

```bash
RUST_LOG=spark_window=debug cargo run -p spark
```

## The event loop and the per-frame tick

A window isn't just a rectangle — it's a stream of events from the
operating system. Key presses, mouse moves, redraw requests, window
resizes, the "close" click: they all arrive as callbacks. The code
that receives and dispatches those callbacks is the **event loop**.

`spark-window` doesn't implement an event loop itself; it builds on
[`winit`], the standard Rust cross-platform windowing library. `winit`
papers over the per-OS differences (Win32, Cocoa, X11/Wayland, …) and
gives us a uniform Rust interface:

```text
                        OS  (keyboard, mouse, repaint, …)
                                    │
                                    ▼
                     winit::event_loop::EventLoop
                                    │  callbacks
                                    ▼
              EventLoopRunner (lives in lib/window)
                                    │
                  ┌─────────────────┼──────────────────┐
                  ▼                 ▼                  ▼
            tracing events  Application::run_stage   request_redraw
                                  (per frame)
```

### Per-frame schedule, today

The loop is built once inside [`run`], then handed to winit via
`run_app(&mut runner)`. The runner owns the [`Application`] that was
handed to it from the [`set_runner`](`Application::set_runner`)
closure. On every `WindowEvent::RedrawRequested`, the runner ticks
the per-frame stages, then asks winit for the next redraw:

```text
   ┌────────────────────────────────────────────────┐
   │  one frame (on RedrawRequested) — TODAY        │
   │                                                │
   │   1. app.run_stage(PRE_UPDATE)                 │  ◀── input / time
   │      → flush queued commands                   │
   │                                                │
   │   2. app.run_stage(UPDATE)                     │  ◀── game logic
   │      → flush queued commands                   │       (movement,
   │                                                │        spawning)
   │   3. app.run_stage(POST_UPDATE)                │  ◀── settled-state
   │      → flush queued commands                   │       bookkeeping
   │                                                │
   │   4. window.request_redraw()                   │  ◀── queue next
   │                                                │       frame
   └────────────────────────────────────────────────┘
```

Each `run_stage(_)` call runs every system registered to that stage
in registration order, then drains pending [`spark_ecs::Commands`]
into the [`spark_ecs::World`]. So a system that calls
`commands.spawn().insert(Position { … })` in `UPDATE` has its entity
visible in `POST_UPDATE` of the same frame, and to every system in
every later frame.

The control-flow mode is `ControlFlow::Wait`: the OS thread sleeps
between frames and `window.request_redraw()` is what wakes it for
the next tick. This is **temporary** — it's the minimum that makes
the per-frame stages tick at all without a render path. It works
because the OS compositor schedules the redraw at roughly its native
cadence, so frames don't free-spin and CPU stays cool. The cost: no
sub-frame precision and no way to drain queued input events between
ticks. Both come back when the loop flips to `Poll` (see below).

### Where we're headed

The shipping shape is a stepping stone. The target loop — the one
[`docs/PLAN.md`](../../docs/PLAN.md) calls for — separates input,
fixed-timestep simulation, variable-rate game logic, and rendering
into distinct slots driven by two different winit hooks:

```text
   ┌───────────────────────────────────────────────────┐
   │  one frame — TARGET                               │
   │                                                   │
   │  about_to_wait  ┐    (ControlFlow::Poll)          │
   │                 │                                 │
   │   1. drain queued WindowEvents into an            │  ◀── Input
   │      `InputState` resource                        │       collection
   │                                                   │
   │   2. run FIXED_UPDATE schedule N times            │  ◀── Simulation
   │      (60 Hz fixed timestep — deterministic)       │       (60 Hz)
   │                                                   │
   │   3. run UPDATE schedule once                     │  ◀── Per-frame
   │      (variable rate — animations, ECS commands)   │       game logic
   │                                                   │
   │  RedrawRequested  ┐                               │
   │                   │                               │
   │   4. run RENDER schedule                          │  ◀── Rendering
   │      (push a frame to the GPU)                    │       (variable)
   │                                                   │
   └───────────────────────────────────────────────────┘
```

What's different from today:

- **`ControlFlow::Poll`** instead of `Wait`. Poll asks winit to fire
  `about_to_wait` as fast as the OS allows. That's where the
  per-frame *simulation* lives — independent of when the GPU is ready
  to draw. With `Wait`, sim and render are conflated into the single
  `RedrawRequested` handler; with `Poll`, they separate cleanly.
- **Two hooks, not one.** `about_to_wait` runs input + sim every
  iteration; `RedrawRequested` only fires the render schedule. The
  swapchain (via `wgpu`) is what gates `RedrawRequested` cadence, so
  rendering paces against vsync without us doing anything special.
- **A real `FIXED_UPDATE` stage** with an accumulator, so simulation
  stays deterministic across hardware (a save/replay/multiplayer
  prerequisite).
- **Per-frame `KeyboardInput` / `MouseInput` events** drain into an
  `InputState` resource before any sim runs, instead of being
  consumed inline as `tracing` events.

Each piece grows into its own crate as the milestones land:

| Capability | Where it'll live | Milestone |
|-|-|-|
| Input collection — drain `KeyboardInput` / `MouseInput` into an `InputState` resource | `spark-input` | M3 follow-up |
| `FIXED_UPDATE` stage + accumulator (60 Hz, deterministic) | `spark-core` | M3 follow-up |
| `ControlFlow::Wait → Poll` flip + `about_to_wait` driver | `spark-window` | lands with `FIXED_UPDATE` |
| `RENDER` stage that pushes a frame to the GPU | `spark-render` (`wgpu` + WGSL) | M5 |
| Multi-threaded scheduler | `spark-ecs` parallel executor | M4 |

The per-stage pattern — *run every system, then flush pending
`Commands`* — is settled and won't change. New stages slot in around
the existing three; the body of `run_stage` stays the same.

## How `WindowPlugin` plugs into `Application`

By default, `Application::run` is a normal loop: it executes startup
systems and then runs the configured schedule. That works for
headless tools. For a windowed game it can't, because the OS event
loop has to own the main thread — `winit` will panic if you call its
`run_app` from anywhere else on macOS or Windows.

The solution is [`Application::set_runner`]. A *runner* is a single
closure that takes the fully-built `Application` and is responsible
for running it to completion. `WindowPlugin` installs one in its
`build`:

```rust,ignore
impl Plugin for WindowPlugin {
    fn build(&self, app: &mut Application) {
        let config = self.config.clone();
        app.set_runner(move |app: Application| -> Result<(), EngineError> {
            event_loop::run(app, config)?;
            Ok(())
        });
    }
}
```

When `Application::run()` fires, it hands itself to that closure
instead of returning early. The closure's job is to drive the
application until exit — for `WindowPlugin`, that means moving the
`Application` into the `EventLoopRunner` and handing the main thread
to `winit::EventLoop::run_app`. Each `RedrawRequested` ticks the
per-frame stages on that owned `Application`. When the user closes
the window, `run_app` returns, the closure returns `Ok(())`, and
`Application::run` returns to the caller.

Things worth knowing:

- **One runner per `Application`.** `set_runner` overwrites. Plugins
  that install startup systems (like `LogPlugin`) don't conflict, but
  registering two runner-installing plugins means the last one wins.
- **The runner owns the `Application`.** The closure takes
  `Application` by value; the `EventLoopRunner` stores it as a field
  and ticks `PRE_UPDATE → UPDATE → POST_UPDATE` on every redraw.
- **`set_runner` is what makes the plugin model windowed-game-
  friendly.** Without it, anyone wanting to use winit would have to
  bypass `Application` entirely. With it, windowed apps stay inside
  the regular `add_plugin` flow.

[`Application::set_runner`]: ../spark_core/struct.Application.html#method.set_runner

## Running without the plugin

Most code goes through `WindowPlugin`. If you're writing a tiny
example or a tool that doesn't use the full `add_plugin` chain,
[`run`] is the same function the plugin delegates to. It takes an
already-built [`Application`] plus a [`WindowConfig`]:

```rust,no_run
use spark_core::Application;
use spark_window::{WindowConfig, run};

let app = Application::new();  // add systems / resources here first
run(app, WindowConfig::default().with_title("Tiny example"))?;
# Ok::<(), spark_window::WindowError>(())
```

It still blocks on the main thread, ticks the per-frame stages on the
owned `Application` (see *Per-frame schedule* above), and returns
once the user closes the window.

## Errors

`run` returns a typed [`WindowError`]; `WindowPlugin` converts it to
[`spark_core::EngineError`] via `?` before surfacing from
`Application::run`.

| Variant | Cause |
|-|-|
| `WindowError::EventLoop` | `winit` could not create or drive the OS event loop. Usually a platform integration problem — missing X11/Wayland on Linux, sandbox restrictions, no display in CI. |
| `WindowError::Os` | The OS refused to create the window. Rare — usually means the requested attributes (size, etc.) are invalid for the current display. |

Both variants wrap the underlying `winit` error via `#[from]`, so the
full error chain is preserved through `std::error::Error::source`.

[`WindowError`]: enum.WindowError.html
