# Spark — High-Level Plan

A Rust learning project: build a small game engine module-by-module, and use it to build **Spark**, a power-grid + city simulator with indirect control.

See **[GAME_DESIGN.md](./GAME_DESIGN.md)** for the game concept, **[ECS_DESIGN.md](./ECS_DESIGN.md)** for the ECS architecture, and **[UI_DESIGN.md](./UI_DESIGN.md)** for the UI layer (editor vs game UI).

## Context

- **Spark** is the project codename. The game-design vision is in `GAME_DESIGN.md`.
- This is a **new repo** (`spark`). The earlier [AlexTiTanium/orange](https://github.com/AlexTiTanium/orange) project is **reference material only** — we'll borrow its workspace layout patterns but write everything fresh, using modern Rust (edition 2024, current crates, ECS-centric design).
- Single monorepo. Engine modules live under `lib/`; the game binary lives under `src/`. If the engine ever matures into a standalone product we can extract it later — premature splitting wastes effort.

## Goals

1. **Engine-as-learning.** Touch every layer of a modern Rust game engine — windowing, wgpu, ECS, assets, audio, UI — and understand it from the inside.
2. **Ship Spark.** A small but complete power + city game (see `GAME_DESIGN.md` for scope).
3. **Modern Rust.** Edition 2024, current dependency versions, idiomatic patterns. No `lazy_static!`, no `extern crate`, etc.

Guiding rule: **build the engine the game needs, not a general-purpose engine.** Engine and game grow together.

## Architecture principles

- **ECS-centric.** Everything that lives in the world is either an Entity (many, dynamic) or a Resource (one, unique). No global state outside the ECS.
- **Modular by Cargo crate.** Each engine module is a separate `lib/*` crate with its own dependencies; the game binary in `src/` sits on top. A new crate is justified by a distinct architectural layer (windowing, rendering, input, ECS, audio) — engine-wide infrastructure (logging, errors, math, time, ids) lives as modules inside `spark-core`, not as standalone crates.
- **Plugin-driven from day one.** The binary uses `spark_core::Application` — an ordered list of `Plugin`s plus the boot sequence (logging init, window startup, top-level error type). `add_plugin`, `world`, and `add_resource` are there from M1; what M4+ adds is the ECS read-side — `Res`/`ResMut` system params, the `Workload` machinery, and the schedule driver — so each `Plugin` can register Systems, not just Resources. The API grows additively; nothing gets ripped out.
- **Separation of concerns.** Process-wide state (`tracing` subscriber, panic hook, root `EngineError`) lives in `spark-core`. Per-layer events and errors live in their layer crate (`WindowError` in `spark-window`, etc.). Libraries never install global state — that's the boot harness's job.
- **Two clocks.** Fixed-timestep simulation (60 Hz) for game logic; variable-rate render for display.
- **Deterministic simulation.** No `HashMap` iteration in sim systems — keeps the door open for save/replay/multiplayer later.

## Repo structure

```
/
├── Cargo.toml                  # workspace manifest
├── docs/
|   ├── README.md
|   ├── PLAN.md                     # this file
|   ├── GAME_DESIGN.md
|   ├── ECS_DESIGN.md
|   ├── UI_DESIGN.md
├── lib/
│   ├── core/                   # math, time, error, log, ids
│   ├── ecs/                    # roll-your-own ECS (see ECS_DESIGN.md)
│   ├── window/                 # winit integration, surface lifecycle
│   ├── input/                  # input collection → Resource each frame
│   ├── render/                 # wgpu pipelines, sprite batcher, tilemap renderer
│   ├── assets/                 # texture/atlas/mesh loading, hot reload
│   ├── ui/                     # egui + wgpu plumbing, EguiContext Resource
│   ├── editor/                 # dev-only inspector UI (feature-flagged)
│   └── audio/                  # kira wrapper
├── src/
│   ├── Cargo.toml
│   ├── main.rs             # wire plugins, run app
│   └── game/
│       ├── map/              # tile map, terrain, geography
│       ├── items/             # build blocs types (water wheel, coal, wires, generators ...)
│       ├── grid/               # power network: producers, transmission, consumers
│       ├── cities/             # city entities, demand, growth, tiers
│       ├── workers/            # worker AI, job queue, assignments
│       ├── economy/            # capital, costs, income
│       ├── simulation/         # Simulations logic
│       ├── ui/                 # game-side UI: theme, widgets, anim, screens
│       └── plugins/            # game-side ECS plugins
└── assets/
    ├── textures/
    ├── shaders/
    └── audio/
```

## Module dependency graph

```
ecs     ── (stdlib only — deepest foundation crate; no external ECS dep)
core    ── ecs + (glam, thiserror, tracing)
window  ── core + (winit)
input   ── core + window + (gilrs)
render  ── core + window + (wgpu, image)
assets  ── core + render + (notify)
ui      ── core + window + input + render + (egui, egui-wgpu, egui-winit)
editor  ── core + ui                    (feature-flagged, dev-only)
audio   ── core + (kira)
game    ── all of the above
```

`spark-ecs` is the deepest crate so `Application` (in `spark-core`) can
embed a `World` without inverting Cargo's no-cycle rule. `spark-core`
does **not** re-export ECS items — any crate above `core` that needs
`World`, `Res`, `ResMut`, `IntoSystem`, or `SystemParam` adds a direct
`spark-ecs` dep alongside `spark-core`.

Each sub-crate owns its `Cargo.toml`.

## Workspace `Cargo.toml`

```toml
[workspace]
resolver = "3"
members = ["lib/*", "src"]

[workspace.package]
edition = "2024"
rust-version = "1.95"
license = "MIT OR Apache-2.0"

[workspace.dependencies]
# Pinned exact versions, e.g. winit = "0.30.13", tracing = "0.1.44".

[workspace.lints.rust]
unsafe_code = "warn"

[workspace.lints.clippy]
pedantic = { level = "warn", priority = -1 }
```

Each sub-crate's manifest:

```toml
[package]
name = "spark-render"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
spark-core = { path = "../core" }
spark-ecs  = { path = "../ecs" }
wgpu.workspace  = true
glam.workspace  = true
winit.workspace = true
```

## ECS approach

Roll our own. Sparse-set storage, generational entity IDs, plugin/app pattern. Full architecture and step-by-step build plan in **[ECS_DESIGN.md](./ECS_DESIGN.md)**.

## UI approach

`egui` (MIT) as the UI library, no fork. Two distinct UIs with two distinct rule sets:

- **Editor UI** — vanilla `egui`, lives in `lib/editor/`, feature-flagged out of release builds, consumes ECS reflection APIs. Visual polish explicitly out of scope.
- **Game UI** — vanilla `egui` + `egui_taffy` (layout) + `spark-ui-theme` + `spark-ui-widgets` + `spark-ui-anim` (our own thin crates on top). Lives in `src/game/ui/`.

Game UI is built on vanilla `egui` through M7–M13; the custom stack is composed at M14 once real game screens exist. Full rationale, library survey, and crate breakdown in **[UI_DESIGN.md](./UI_DESIGN.md)**.

## ECS design: Resources vs Entities for Spark

**Resource** = exactly one of this thing in the world. Accessed by type.
**Entity** = many of these, created and destroyed dynamically.

### Resources

- `Window` — single window handle
- `RenderContext` — wgpu device, queue, surface
- `Input` — current frame's keyboard/mouse state
- `Time` — delta, elapsed, fixed-timestep accumulator, in-game clock
- `AssetServer` — texture/atlas/sound storage
- `TileMap` — the world grid (terrain, resource deposits)
- `Settings` / `Config`

### Entities

- Plants (water wheel, coal burner, windmill, ...) — each placed instance
- Transmission line segments (or nodes; graph stored in `PowerNetwork`)
- Workers — individuals with AI state, location, current job
- Particle/visual effects, smoke from plants, etc.

## Plugin pattern

### M1–M3 — `spark_core::Application`, plugin-driven from day one

There's no ECS read-side yet (no `Res`/`ResMut` system params, no schedule driver), but the binary is **already plugin-driven**: `spark_core::Application` owns an ordered list of `Plugin`s plus the boot order (tracing init, root error type, window startup). Every subsystem is a `Plugin` — there are no `.with_window` / `.with_log`-style builder methods.

```rust
// /src/main.rs (M1–M3)
fn main() -> Result<(), spark_core::EngineError> {
    spark_core::Application::new()
        .add_plugin(LogPlugin)
        .add_plugin(WindowPlugin {
            config: WindowConfig::default()
                .with_title("Spark")
                .with_size(1280, 720),
        })
        .run()
}
```

The set of plugins grows additively as milestones land — the log PR adds `LogPlugin`, the window PR adds `WindowPlugin`, an input PR adds `InputPlugin` — but the shape (`new().add_plugin(...).run()`) is fixed from M1.

### M4 onward — Systems + the schedule driver (the ECS read-side)

`spark-ecs` already exposes the `World` + `add_resource` value-verb that `Application` embeds today; `Plugin`/`add_plugin` are wired in `spark-core`. What still lands in M4 is the read side — `Res<T>` / `ResMut<T>` system params, the `Workload` machinery, and the schedule driver (ECS_DESIGN.md stage 14). At that point each engine crate exposes a `Plugin` that registers its Resources and Systems:

```rust
// /src/main.rs (M4+)
fn main() {
    App::new()
        .add_plugin(CorePlugin)
        .add_plugin(WindowPlugin)
        .add_plugin(InputPlugin)
        .add_plugin(RenderPlugin)
        .add_plugin(AssetsPlugin)
        .add_plugin(EditorPlugin)
        .add_plugin(AudioPlugin)
        // game-side plugins
        .add_plugin(WorldPlugin)         // tile map, terrain
        .add_plugin(WorkerPlugin)        // worker AI, job dispatch
        .add_plugin(EconomyPlugin)       // capital, costs
        .add_plugin(TechPlugin)          // research tree
        .add_plugin(UiPlugin)            // overlays, panels
        .run();
}
```

The migration is additive on the engine side — `Application` and its helpers stay (as a thin wrapper around the new `App` if useful), so binaries written against M1 still compile.

## Outdated patterns to avoid (lessons from orange)

- `edition = "2018"` → **2024**
- `extern crate foo;` → delete, unnecessary since 2018
- `mod.rs` files → **prefer** `foo.rs` next to `foo/` directory (better editor-tab labels and symmetric refactor when a single-file module grows into a folder), but `mod.rs` is acceptable when contributor preference outweighs the ergonomics — pick one style per module, don't mix within a single subtree
- `lazy_static!` → `std::sync::LazyLock` (stable since 1.80)
- `try!(...)` → `?`
- `failure` / `error-chain` → `thiserror` (libs) + `anyhow` (apps)
- Hand-rolled OpenGL + GLSL → **wgpu + WGSL**
- `Box<dyn Error>` everywhere → typed errors via `thiserror`
- `log` + `env_logger` (still fine) → **`tracing` + `tracing-subscriber`**
- `cargo-bundle` (semi-abandoned) → `cargo-dist` or platform-native
- `authors` in `Cargo.toml` (no longer auto-filled) → optional, can drop
- Only Pinned exact deps (`"1.1.0"`)

## Milestones

### Engine-foundation (no game content yet)

#### M1 — Hello window
- Set up workspace, `core`, `window`, `input` crates
- `spark_core::Application` boot harness: owns config + boot order (tracing init, root `EngineError`, window startup). Pre-ECS stand-in for the canonical `App` arriving in M4 — same API surface, extended additively as later crates land.
- Window opens, logs input events
- No render, no ECS yet

#### M2 — Hello triangle
- Add `render` crate, wgpu setup
- Clear color, then a triangle in WGSL
- Resize handling
- Hard-coded vertex buffer — no ECS integration yet

#### M3 — ECS foundations
- Build `ecs` crate steps 1–6 from [ECS_DESIGN.md](./ECS_DESIGN.md):
  Entity allocator, ComponentStorage, World, queries (single & multi-component)
- Tests passing for each step
- Pure library, no game integration

#### M4 — ECS feature-complete
- Build `ecs` crate steps 7–11:
  Resources, Systems, Stage, App+Plugin, FixedUpdate, Events
- Introduce the `Resource` and `Component` traits, both
  `Send + Sync + 'static`. `SystemParam` impls thread the bound
  through. This is the committed contract — Spark targets a heavy
  simulation and parallel system execution is a hard requirement, not
  an optional extra.
- Scheduler executes independent systems in parallel (Rayon), using
  the per-system read/write access set + the `Send + Sync` bound as
  the safety proof for lockless execution. Conflicts are caught at
  registration time (no `RefCell` runtime-panic safety net).
- Migrate `window` and `input` plugins to ECS-style (state in Resources)
- App main loop runs the stages each frame

#### M5 — Sprites via ECS
- Add `assets` crate with texture loading
- Sprite renderer in `render`: textured quad as entity with `Position` + `Sprite`
- Camera as a Resource (orthographic, 2D top-down)
- One draw call per atlas (batched)

### Game-foundation (world is interactive but empty)

#### M6 — Terrain & camera
- `TileMap` resource: grid of tile types (grass, forest, river, ...)
- Tile renderer reads `TileMap` + atlas, batched
- Camera pan with WASD / middle-mouse, zoom with wheel
- Recognisable map on screen

### Stretch / advanced

- **A1** — Change detection in ECS (`Changed<T>` filters)
- **A3** — Archetype storage refactor (replace sparse-set internals)
- **A4** — Save/load (custom serialization)
- **A7** — Day/night cycle affecting solar + lighting demand

(Parallel system execution is no longer listed here — it moved into M4
as a committed requirement, not a stretch goal.)

## Repo hygiene

- `clippy.toml` + `rustfmt.toml` at root
- `[workspace.lints]` in `Cargo.toml` — central lint config
- `rust-version` declared — pinned MSRV
- `.github/workflows/ci.yml` — `cargo check`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt -- --check`
- `CHANGELOG.md` from M5 onward
- `README.md` with quickstart + screenshot once M6 is visible

## Open questions

Parked until they become blockers — see also the open questions in `GAME_DESIGN.md`:

- Camera angle: pure top-down vs isometric (top-down is current default, simpler)
- Save format: `bincode` (fast, fragile) vs `ron` (human-readable, slower). RON for dev, bincode for shipped saves
- Worker visual representation: dots, pawns, full sprites? Affects render perf and aesthetic
- Power line rendering: pylons + sagging lines, or abstract glow lines?
