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
3. **Modern Rust.** Edition 2024, current dependency versions, idiomatic patterns. No `lazy_static!`, no `extern crate`, no `mod.rs`, etc.

Guiding rule: **build the engine the game needs, not a general-purpose engine.** Engine and game grow together.

## Architecture principles

- **ECS-centric.** Everything that lives in the world is either an Entity (many, dynamic) or a Resource (one, unique). No global state outside the ECS.
- **Modular by Cargo crate.** Each engine module is a separate `lib/*` crate with its own dependencies; the game binary in `src/` sits on top. A new crate is justified by a distinct architectural layer (windowing, rendering, input, ECS, audio) — engine-wide infrastructure (logging, errors, math, time, ids) lives as modules inside `spark-core`, not as standalone crates.
- **Boot harness, then plugins.** Pre-ECS (M1–M3) the binary uses `spark_core::Application` — a small builder owning config and the boot sequence (logging init, window startup, top-level error type). Post-ECS (M4+) `Application` gains `add_plugin`, `world`, and the schedule driver; engine and game crates expose `Plugin`s that register their Resources and Systems. The API grows additively; nothing gets ripped out.
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
embed a `World` without inverting Cargo's no-cycle rule. Every crate
above `core` reaches `World` through `spark_core::World`, keeping
`spark-ecs` a transitive dep.

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

### M1–M3 — `spark_core::Application` boot harness (no ECS yet)

Without ECS there are no Resources or Schedules for a `Plugin` trait to register with, so the "plugin" idea is collapsed into a small boot harness — `spark_core::Application`. It owns config + boot order (tracing init, root error type, window startup):

```rust
// /src/main.rs (M1)
fn main() -> Result<(), spark_core::EngineError> {
    spark_core::Application::new()
        .with_window(
            WindowConfig::default()
                .with_title("Spark")
                .with_size(1280, 720),
        )
        .run()
}
```

The builder grows additively as M1→M3 progresses: M1 adds `.with_window`; the input PR adds `.with_input`; M2 adds `.with_render`. Earlier methods stay valid through every later milestone.

### M4 onward — formal `App` + `Plugin` (with ECS)

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
- `mod.rs` files → use `foo.rs` next to `foo/` directory
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
  Resources, Systems, Schedule, App+Plugin, FixedUpdate, Events
- Migrate `window` and `input` plugins to ECS-style (state in Resources)
- App main loop runs the schedule each frame

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
- **A2** — Parallel system execution via Rayon
- **A3** — Archetype storage refactor (replace sparse-set internals)
- **A4** — Save/load (custom serialization)
- **A7** — Day/night cycle affecting solar + lighting demand

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
