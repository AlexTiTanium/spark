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

[`WindowConfig`] is a plain struct with chainable builders. Hand a
custom value to `WindowPlugin` via the public `config` field:

```rust
use spark_core::Application;
use spark_window::{WindowConfig, WindowPlugin};

let cfg = WindowConfig::default()
    .with_title("My Spark Game")
    .with_size(1920, 1080)
    .with_resizable(false);

let _app = Application::new().add_plugin(WindowPlugin { config: cfg });
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

## Running without the plugin

Most code goes through `WindowPlugin`. If you're writing a tiny
example or a tool that doesn't use the full `Application` scaffolding,
[`run`] is the same function the plugin delegates to:

```rust,no_run
use spark_window::{WindowConfig, run};

run(WindowConfig::default().with_title("Tiny example"))?;
# Ok::<(), spark_window::WindowError>(())
```

It still blocks on the main thread and still returns once the user
closes the window.

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
