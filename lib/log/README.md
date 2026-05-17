# spark-log

The Spark engine's logging plugin. Installs a global `tracing`
subscriber so every crate in the engine — and your game code in
`src/` — can emit log messages that appear in the terminal.

> **What is `tracing`?** Two ideas you'll meet here:
> - **Events** — single log lines, like `println!` with levels and
>   typed fields. `info!`, `warn!`, `error!`, etc.
> - **Spans** — a chunk of time (a frame, a system tick, a function
>   call) during which any events you emit inherit a shared context.

> **Where do log lines go?** To **stderr**, which the terminal mixes
> in with stdout when you run `cargo run`. To capture them
> separately, redirect with `2>game.log` — see *Saving and analyzing
> logs* below.

## Setup

`LogPlugin` is the first plugin you register. Anything emitted before
the startup phase runs is dropped, so it goes at the top of the chain.

```rust
use spark_core::Application;
use spark_log::LogPlugin;

Application::new()
    .add_plugin(LogPlugin)
    // .add_plugin(WindowPlugin::default())
    // .add_plugin(...other plugins...)
    .run()
    .unwrap();
```

From game code (anything in `src/`), import macros directly from
`spark_log` — no direct `tracing` dependency required:

```rust
use spark_log::info;

info!("game started");
// → INFO game started
```

Engine crates under `lib/*` work slightly differently — see *Using
from an engine crate* near the end.

## Levels

Five macros, ordered from quietest to loudest. The default filter
(`spark=info,warn`) hides `trace!` and `debug!`; switch them on at
runtime with `RUST_LOG` (see *Controlling output* below).

| Macro | When to reach for it |
|-|-|
| `trace!` | Per-frame or per-iteration noise. Hidden by default. |
| `debug!` | Helpful during development — startup steps, branch picked, parsed config. Hidden by default. |
| `info!`  | High-level state changes a normal session should show: "world loaded", "save written". |
| `warn!`  | Something's off but the game keeps running — fallback used, asset missing. |
| `error!` | A real failure that matters; usually paired with a `Result::Err`. |

## Events: attaching data to a log line

An **event** is a single log record. Beyond a plain message, you can
attach typed `key = value` fields — that's the headline reason to use
`tracing` over `println!`. The macros accept several shapes; pick
whichever reads best.

```rust
use spark_log::info;

let count = 42;
let elapsed_ms = 12;

// 1. Plain message
info!("world loaded");

// 2. Format placeholders, like println!
info!("loaded {} entities in {} ms", count, elapsed_ms);

// 3. Named fields (the structured form — most useful long-term)
info!(count = 42, elapsed_ms = 12, "world loaded");

// 4. Bare identifier — field name inferred from the variable
info!(count, elapsed_ms, "world loaded");
// → INFO world loaded count=42 elapsed_ms=12
```

The named-field form is the one that pays off later: fields can be
filtered and searched separately from the message text.

### Your own structs (`?` for Debug, `%` for Display)

Two field markers control how `tracing` formats a value:

- `?value` — uses the [`Debug`] trait. Add `#[derive(Debug)]` to your
  struct and you're done.
- `%value` — uses the [`Display`] trait. Built-in types like `String`,
  `&str`, `i32`, `u32`, `f64`, `bool` already implement it; your own
  types need an `impl Display`.

```rust
use spark_log::info;

#[derive(Debug)]              // required so `?player` works below
struct Player { name: String, hp: u32 }

let player = Player { name: "Alex".into(), hp: 100 };

info!(?player, "spawned");
// → INFO spawned player=Player { name: "Alex", hp: 100 }

info!(name = %player.name, hp = player.hp, "joined");
// → INFO joined name=Alex hp=100
```

> **Heads-up.** Forget the `#[derive(Debug)]` and the `?player` line
> won't compile — the compiler tells you `Player` doesn't implement
> `Debug`. Same story with `%` and `Display`.

[`Debug`]: https://doc.rust-lang.org/std/fmt/trait.Debug.html
[`Display`]: https://doc.rust-lang.org/std/fmt/trait.Display.html

### Mixing forms

Every form above combines in a single call:

```rust
use spark_log::info;

#[derive(Debug)]
struct Player { name: String, hp: u32 }

let player = Player { name: "Alex".into(), hp: 100 };
let frame = 42;

info!(frame, ?player, zone = "boss-1", "tick {} processed", frame);
// → INFO tick 42 processed frame=42 player=… zone="boss-1"
```

## Spans: context across multiple events

An event is one log line. A **span** is a chunk of time — a frame, a
system tick, a function call — during which any events you emit
automatically inherit shared context (frame number, system name,
player id) without your repeating it on every line.

The pattern is always: create a span with fields, *enter* it, do work,
exit when the guard drops.

### A basic span

```rust
use spark_log::{info, info_span};

let frame_id: u32 = 42;
let span = info_span!("frame", id = frame_id);
let _guard = span.enter();   // span is current until `_guard` drops

info!("entities updated");
info!("physics stepped");
// → INFO frame{id=42}: entities updated
// → INFO frame{id=42}: physics stepped
```

The `_guard` variable holds the span entered. When it drops at end of
scope, the span exits.

### Nested spans

Spans stack. Events inside an inner span carry context from every
active span:

```rust
use spark_log::{info, info_span};

let _frame = info_span!("frame", id = 7).entered();
info!("started");

let _system = info_span!("physics").entered();
info!("collision");
// → INFO frame{id=7}: started
// → INFO frame{id=7}:physics: collision
```

`.entered()` is a shorthand that returns the guard directly — use it
when you don't need to keep the span value separately.

### One macro per level

Spans come in the same five flavours as events: `trace_span!`,
`debug_span!`, `info_span!`, `warn_span!`, `error_span!`. A span's
level controls whether it (and its inherited fields) shows up under
the current `RUST_LOG` filter — same rules as events.

## `#[instrument]`: spans for whole functions

`#[instrument]` wraps an entire function call in a span automatically.
Every event inside the function — and any nested instrumented
functions — inherits the function name and its arguments.

```rust
use spark_log::{info, instrument};

#[instrument]
fn load_player(path: &str, retry: bool) {
    info!("reading file");
    info!("parsing");
}

load_player("saves/run1.world", false);
// → INFO load_player{path="saves/run1.world" retry=false}: reading file
// → INFO load_player{path="saves/run1.world" retry=false}: parsing
```

Reach for it when you'd otherwise paste the same `info_span!(...)` at
the top of every function. Skip arguments you don't want recorded with
`#[instrument(skip(big_value))]`.

## Controlling output with `RUST_LOG`

By default Spark prints `info` and above for every `spark*` crate, and
`warn` and above for everything else. Override at runtime with the
`RUST_LOG` environment variable — these are the commands you'll
actually type:

```bash
# Show debug-level and up across every spark crate
RUST_LOG=spark=debug cargo run -p spark

# Crank one specific crate to trace, leave the rest alone
RUST_LOG=spark_render=trace cargo run -p spark

# Quiet mode: only warnings and errors, everywhere
RUST_LOG=warn cargo run -p spark

# Multiple filters joined by commas (last match wins)
RUST_LOG=spark=info,spark_render=debug,wgpu=warn cargo run -p spark
```

The `spark=info` form is a **byte-prefix match** on the target name —
it matches every record whose target starts with `spark` (so
`spark_log`, `spark_window`, `spark_render`, …) without listing each.
The baseline default is defined in `DEFAULT_FILTER` (`spark=info,warn`).

### Custom targets

Every log record has a **target** — by default, the crate it was
emitted from. Override it to scope a record to a subsystem name that
`RUST_LOG` can match independently:

```rust
use spark_log::info;

info!(target: "physics", "collision detected");
info!(target: "physics::narrow_phase", "AABB overlap");
```

Now `RUST_LOG=physics=trace cargo run -p spark` lights up every
`physics::*` record without affecting the rest of the crate it lives
in.

## Saving and analyzing logs

Spark writes log records to **stderr**, so capturing them is a
shell-redirection trick — no code change needed.

### Send everything to a file

```bash
RUST_LOG=spark=debug cargo run -p spark 2> game.log
```

The `2>` operator captures stderr only, so cargo's status messages
(stdout) still print to your terminal while every log line lands in
`game.log`.

### See it live *and* save it

Use `tee` to fan the stream to both the terminal and a file:

```bash
RUST_LOG=spark=debug cargo run -p spark 2>&1 | tee game.log
```

- `2>&1` merges stderr into stdout (so cargo's build output and your
  logs end up in the same stream).
- `tee` writes that merged stream to `game.log` while passing it
  through to your terminal.

### Timestamp each run

Avoids overwriting yesterday's log when you start a session today:

```bash
RUST_LOG=spark=debug cargo run -p spark 2> "game-$(date +%Y%m%d-%H%M%S).log"
```

### Analyze it afterwards

Because structured fields are written as `key=value`, plain text tools
are enough for most investigations:

```bash
# Pull every line at warn or error level
grep -E ' (WARN|ERROR) ' game.log

# Show only frames where fps was reported
grep 'fps=' game.log

# Tally the distinct entity counts that were logged
grep -oE 'entity_count=[0-9]+' game.log | sort | uniq -c

# Tail the file while the game is still running (in another terminal)
tail -f game.log
```

## Using from an engine crate (`lib/*`)

Engine crates **do not** depend on `spark-log`. The logging plugin is
an application-level concern; an internal crate like `spark-render`
shouldn't know it exists. Instead, depend on `tracing` directly. In
that crate's `Cargo.toml`:

```toml
[dependencies]
tracing.workspace = true
```

Then use the same macros, imported from `tracing`:

```rust
use tracing::{info, debug, info_span, instrument};

info!("renderer ready");
debug!("vsync enabled");

let _span = info_span!("draw_call", pass = "shadow").entered();
```

Both styles end up at the same subscriber that `LogPlugin` installed —
the split is about *who depends on what*, not about the records
themselves.

## Errors

Installing the subscriber twice in the same process returns a
`LogError::AlreadyInstalled`, which the plugin converts to a
`spark_core::EngineError` before surfacing from `run`. You should only
hit this if another crate has already called
`tracing_subscriber::*::init`. Sticking to one `LogPlugin` at the top
of `Application::new()` avoids it entirely.
