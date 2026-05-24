# spark-common

Engine-wide shared resources for Spark. Today it provides one thing: the
**`Time`** resource — the single clock that every other system reads to learn
how much time has passed this frame.

> **What is "delta time" and a "fixed timestep"?** Two ideas you'll meet here:
> - **Delta time** — how long the *last* frame took, in seconds. Movement is
>   written as `position += velocity * delta` so things move at the same real
>   speed whether the game runs at 30 or 240 frames per second.
> - **Fixed timestep** — the simulation advances in constant 1/60-second steps,
>   no matter the display rate. A slow frame runs the step several times to
>   catch up; a fast one runs it zero times and banks the leftover. Constant
>   steps are what make the simulation *deterministic* — replay the same inputs
>   and you get the same result.

`Time` holds both: a **real** wall clock, a **virtual** clock you can pause and
speed up, and the fixed-timestep accumulator that drives the 60 Hz simulation.

## Setup

Register `TimePlugin` after `LogPlugin` and **before** `WindowPlugin`. The
window runner reads the clock every frame, so the resource has to exist first:

```rust
use spark_core::Application;
use spark_common::TimePlugin;

Application::new()
    // .add_plugin(LogPlugin)            // from spark-log — register first
    .add_plugin(TimePlugin)              // must be before WindowPlugin
    // .add_plugin(WindowPlugin::default())
    .run()
    .unwrap();
```

That inserts a `Time` resource and two infrastructure systems: `advance_time`
(first in `Stage::PreUpdate`, samples the wall clock once per frame) and
`advance_fixed_step` (first in `Stage::FixedUpdate`, counts simulation steps).

## Using it from the game (`src/`)

Ask for the clock with `Res<Time>` in any system and multiply by `delta_secs()`.
The default delta is the **virtual** one — it honors pause and speed
automatically, so you don't sprinkle `if paused` checks through gameplay:

```rust
use spark_common::Time;

// In a real system the parameter is `time: Res<Time>`; the math is the same.
let time = Time::default();
let speed = 64.0_f32;             // pixels per second
let dx = speed * time.delta_secs();
assert_eq!(dx, 0.0);              // no frame has elapsed yet
```

Pause and speed are just setters on the resource — wire them to a key or a HUD
button:

```rust
use spark_common::Time;

let mut time = Time::default();
time.set_scale(2.0);             // 2× — one in-game second per half real second
assert_eq!(time.scale(), 2.0);

time.pause();                    // freeze the gameplay clock
assert!(time.is_paused());
time.unpause();
```

## Using it from an engine crate (`lib/*`)

A crate that needs the clock — for example a `WindowPlugin` runner deciding how
many fixed steps to run — depends on `spark-common` and reads the resource:

```toml
[dependencies]
spark-common = { path = "../common" }
```

```rust
use spark_common::Time;
use spark_core::{Application, Stage};

let mut app = Application::new();
app.add_plugin(spark_common::TimePlugin);
app.run_stage(Stage::PreUpdate);                 // advance_time runs

let steps = app.world().resource::<Time>().fixed_steps_this_frame();
for _ in 0..steps {
    app.run_stage(Stage::FixedUpdate);           // dispatch the sim
}
```

Crates *below* `spark-common` in the dependency graph (e.g. `spark-core`,
`spark-ecs`) must not depend on it — that would cycle. `Time` is for the layers
that sit on top.

## Configuration

None. There are no environment variables or Cargo features. The fixed-timestep
rate (1/60 s) and the spiral-of-death clamp (250 ms) are compile-time constants
in `spark-common`; changing them is a code change, by design.

## Common patterns

**Variable-rate gameplay (the usual case).** Animations, camera lerps, and
display-rate movement run in `Stage::Update` and read `delta_secs()` — scaled,
so pause and speed Just Work:

```rust
use spark_common::Time;

let time = Time::default();
let turn_rate = 90.0_f32;                 // degrees per second
let _step = turn_rate * time.delta_secs();
```

**Deterministic simulation.** Physics, AI, and grid logic run in
`Stage::FixedUpdate` and read `fixed_delta_secs()` — a constant, so the result
is identical on every machine and every replay:

```rust
use spark_common::Time;

let time = Time::default();
let gravity = 9.81_f32;                   // m/s²
let dv = gravity * time.fixed_delta_secs(); // same value every step, forever
assert!((dv - 9.81 / 60.0).abs() < 1e-6);
```

## Errors / pitfalls

- **Use `fixed_delta_secs()` in `FixedUpdate`, never `delta_secs()`.** The
  variable `delta_secs()` is wall-clock time and changes every frame; feeding it
  into the simulation makes physics frame-rate-dependent and breaks the
  save/replay/determinism guarantee. The constant `fixed_delta_secs()` is the
  only correct delta inside `Stage::FixedUpdate`.
- **`Time` only advances if something drives `Stage::PreUpdate`.** `TimePlugin`
  registers the systems, but a headless `Application::run()` with no runner
  never ticks them — the clock stays at zero. The window runner is what pumps
  it. In a test, call `app.run_stage(Stage::PreUpdate)` to step it yourself.
- **First-frame delta is `0`.** There's no previous frame to measure against, so
  `frame 1` reports a zero delta. Code that divides by `delta_secs()` must guard
  against zero.
- **`frame()` vs `fixed_step()`.** `frame()` counts render frames (one per
  `PreUpdate`); `fixed_step()` counts simulation steps (one per `FixedUpdate`
  invocation, which may be 0..N per frame). Index save/replay by `fixed_step()`,
  UI and animation by `frame()`.
- **Pause does not stop the simulation (M1).** Pausing freezes the *virtual*
  clock (`delta_secs()` goes to `0`), but real time still banks fixed steps, so
  `Stage::FixedUpdate` keeps running. Making pause/speed gate the simulation
  deterministically is deferred until simulation systems land (M7+).
