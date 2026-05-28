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
| `trace` | `mouse wheel` | Scroll-wheel movement, normalized to pixels. Fields: `x`, `y`. |

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

## Forwarding input into the world

Logging is for you, the developer. The window *also* turns each input event
into a typed [`spark-input`] event and forwards it into the ECS world, so other
systems can react to it:

| OS event | Forwarded as (`spark_input`) |
|-|-|
| keyboard press / release | `KeyboardInput { key, pressed }` |
| mouse press / release | `MouseButtonInput { button, pressed }` |
| cursor move | `CursorMoved { x, y }` |
| scroll wheel | `MouseWheel { x, y }` (normalized to pixels) |
| focus lost | `FocusLost` |

Two things to know:

- **The window doesn't assume anyone reads these.** It sends each event
  *only if* something already registered the matching `Events<T>` buffer (via
  `add_event::<T>()`) — normally [`spark-input`]'s `InputPlugin`. With no such
  consumer, the send is a silent no-op. The window depends on `spark-input`
  only to *name* the event types it emits; `spark-input` never depends back on
  the window.
- **Keys are physical and de-repeated.** A `KeyboardInput` carries a
  `spark_core::KeyCode` (position-based, so WASD is WASD on any layout), and OS
  auto-repeat is filtered out — a held key produces exactly one
  `pressed: true`. Keys outside the curated `KeyCode` set are dropped.

To consume these as queryable state (`is_pressed`, cursor position, scroll),
add [`spark-input`]'s `InputPlugin`; see that crate's README.

[`spark-input`]: ../spark_input/index.html

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
`run_app(&mut runner)` under `ControlFlow::Poll`. The runner owns the
[`Application`] that was handed to it from the
[`set_runner`](../spark_core/struct.Application.html#method.set_runner)
closure, and splits the frame across **two winit hooks**:

- `about_to_wait` runs once per loop iteration and drives the *simulation*
  block — swapping event buffers in `Input` first, advancing the
  [`Time`](../spark_common/struct.Time.html) clock in `PreUpdate`, then reading
  `Time::fixed_steps_this_frame()` to step the simulation — then asks winit for
  a redraw.
- `RedrawRequested` drives only `Stage::Render`. It's a no-op today (no render
  systems are registered); a `wgpu` swapchain will pace it at vsync in M5.

```text
   ┌──────────────────────────────────────────────────────┐
   │  about_to_wait — every loop iteration (Poll)         │
   │                                                      │
   │   1. app.run_stage(Stage::Input)                     │  ◀── event-buffer
   │      → flush queued commands                         │       swap (FIRST)
   │                                                      │
   │   2. app.run_stage(Stage::PreUpdate)                 │  ◀── advance_time:
   │      → advance_time updates Time                     │       sample clock,
   │      → flush queued commands                         │       bank accumulator
   │                                                      │
   │   3. steps = time.fixed_steps_this_frame()           │  ◀── fixed-timestep
   │      for _ in 0..steps:                              │       simulation
   │        app.run_stage(Stage::FixedUpdate)             │       (0..N steps)
   │                                                      │
   │   4. app.run_stage(Stage::Update)                    │  ◀── game logic
   │      → flush queued commands                         │       (movement,
   │                                                      │        spawning)
   │   5. app.run_stage(Stage::PostUpdate)                │  ◀── settled-state
   │      → flush queued commands                         │       bookkeeping
   │                                                      │
   │   6. window.request_redraw()                         │  ◀── queue a frame
   └──────────────────────────────────────────────────────┘
   ┌──────────────────────────────────────────────────────┐
   │  RedrawRequested — gated by the swapchain (M5)       │
   │                                                      │
   │   7. app.run_stage(Stage::Render)                    │  ◀── rendering
   │                                                      │       (no-op today)
   └──────────────────────────────────────────────────────┘
```

Each `run_stage(_)` call runs every system registered to that stage
in registration order, then drains pending [`spark_ecs::Commands`]
into the [`spark_ecs::World`]. So a system that calls
`commands.spawn().insert(Position { … })` in `Update` has its entity
visible in `PostUpdate` of the same frame, and to every system in
every later frame.

**`Stage::Input` runs first.** It pumps the per-event swap systems that
[`Application::add_event`](../spark_core/struct.Application.html#method.add_event)
registers, rotating each `spark_ecs::Events<T>` double-buffer so this
frame's readers see last frame's sends before any other stage touches
them. It's also the future home of input-state collection (see *Where
we're headed*) — keeping the swap and input concerns in one named phase.

**The fixed-timestep loop.** The 60 Hz accumulator lives in the
[`Time`](../spark_common/struct.Time.html) resource, not in this crate. In
`PreUpdate`, `spark-common`'s `advance_time` system samples the wall clock,
banks the elapsed time, and computes how many whole 1/60 s steps this frame
covers — carrying the sub-step remainder to the next frame. The runner reads
that count with `Time::fixed_steps_this_frame()` and ticks `Stage::FixedUpdate`
exactly that many times. A fast frame runs zero steps; a slow one runs several
— so simulation advances at a steady 60 Hz no matter the display rate, which is
what keeps it deterministic across hardware. The per-frame time is clamped to
250 ms inside `Time::tick` first: without that cap, one long stall (a
breakpoint, a dragged title bar) would bank seconds and fire hundreds of
catch-up steps at once — the "spiral of death". Keeping the math in a
`Duration`-only method (no window, no OS clock) is what lets it be unit-tested
directly: carry-over across frames, the clamp, and the inclusive step boundary
all have table-driven tests in `spark-common`.

The control-flow mode is `ControlFlow::Poll`: winit begins a new loop
iteration immediately rather than sleeping, so `about_to_wait` fires
every iteration and the simulation steps independently of when the GPU
is ready to draw. This separates sim from render cleanly across the two
hooks — but it comes with a **known, deferred cost**: nothing paces the
loop yet. With `Wait`, the OS compositor throttled redraws and the
thread slept between them; under `Poll`, until a `wgpu` swapchain gates
`RedrawRequested` at vsync (M5), the loop **busy-spins a CPU core**.

That regression is accepted deliberately. This flip
([issue #50](https://github.com/AlexTiTanium/spark/issues/50)) shipped
ahead of its trigger conditions — there's no render path yet — so the
sim/render split is in place for when rendering lands; the swapchain's
present-rate will gate `RedrawRequested` and the spin disappears with
it. Sim correctness is unaffected meanwhile: `advance_time` banks
*wall-clock* time, so `FixedUpdate` still steps at a steady 60 Hz no
matter how fast the loop iterates.

### Where we're headed

The two-hook split is in place; what's still missing is the swapchain
that makes it pay off. As the milestones land:

| Capability | Where it'll live | Milestone |
|-|-|-|
| ✅ Input collection — forward OS input as events into `KeyboardState` / `MouseState` | `spark-input` | **shipped** |
| ✅ `Stage::FixedUpdate` driver — reads `Time::fixed_steps_this_frame()` (accumulator owned by `Time`) | `spark-window` runner + `spark-common` | **shipped** |
| ✅ `ControlFlow::Wait → Poll` flip + `about_to_wait` driver (relocates `FixedUpdate`) | `spark-window` | **shipped** |
| `Stage::Render` driver that pushes a frame to the GPU — gates `RedrawRequested` at vsync, ending the busy-spin | `spark-render` (`wgpu` + WGSL) | M5 |
| Sub-frame render interpolation between fixed ticks | `spark-render` | later |
| Multi-threaded scheduler | `spark-ecs` parallel executor | M4 |

Until the swapchain lands, `RedrawRequested` runs an empty `Stage::Render`
and the `Poll` loop is unthrottled — see the busy-spin note above.

The per-stage pattern — *run every system, then flush pending
`Commands`* — is settled and won't change. The `Stage` enum is closed:
the still-reserved phase (`Render`) gains its *driver* as the milestone
lands, but the enum and the body of `run_stage` stay the same.

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
to `winit::EventLoop::run_app`. The loop ticks the simulation stages in
`about_to_wait` and `Stage::Render` in `RedrawRequested` on that owned
`Application`. When the user closes
the window, `run_app` returns, the closure returns `Ok(())`, and
`Application::run` returns to the caller.

Things worth knowing:

- **One runner per `Application`.** `set_runner` overwrites. Plugins
  that install startup systems (like `LogPlugin`) don't conflict, but
  registering two runner-installing plugins means the last one wins.
- **The runner owns the `Application`.** The closure takes
  `Application` by value; the `EventLoopRunner` stores it as a field
  and ticks the simulation stages (`Input → PreUpdate → FixedUpdate × N
  → Update → PostUpdate`) in `about_to_wait`, leaving `Stage::Render`
  for `RedrawRequested`.
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
[`Application`]: ../spark_core/struct.Application.html
[`spark_ecs::Commands`]: ../spark_ecs/struct.Commands.html
[`spark_ecs::World`]: ../spark_ecs/struct.World.html
