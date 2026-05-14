# Spark

A Rust learning project: a custom 2D game engine built module-by-module, plus **Spark** itself — a power-grid + city simulator with indirect, "designate, don't control" gameplay.

> **Status:** design-first, pre-M1. Only a `Hello, world!` stub exists in `src/main.rs`. The architecture, milestones, and game design are spec'd in [`docs/`](./docs).

## The game

You are an energy planner, not a hands-on builder. You designate *what* power infrastructure to build and *where*; workers from your cities travel, construct, and operate it. Cities grow when their demand is met and decline when it isn't. Progression runs along the energy stack: water wheel → combustion → hydro → wind/solar → fossil → nuclear.

Think SimCity's growth pressure × Settlers × Factorio × Anno's supply chains × Dwarf Fortress's designation model, focused entirely on the energy stack.

Full pitch and mechanics: [`docs/GAME_DESIGN.md`](./docs/GAME_DESIGN.md).

## The engine

- **ECS-centric.** All long-lived state lives in a `World` as either a `Resource` (singleton) or an `Entity` (many). No global statics.
- **Custom ECS.** Bevy-style function-parameter system extraction + Shipyard-inspired named `Workload`s, sparse-set storage. No external ECS dependency. See [`docs/ECS_DESIGN.md`](./docs/ECS_DESIGN.md).
- **Plugin pattern.** Each engine crate (`core`, `ecs`, `window`, `input`, `render`, `assets`, `ui`, `editor`, `audio`) exposes a `Plugin` that registers its resources and systems with the `App`.
- **Two clocks.** Fixed-timestep 60 Hz simulation; variable-rate update and render.
- **Deterministic simulation.** No `HashMap` iteration in sim systems — keeps the door open for save/replay/multiplayer.
- **Modern Rust.** Edition 2024, `rust-version = "1.95"`, `wgpu` + WGSL, `tracing`, `thiserror`/`anyhow`, no `lazy_static!` / `extern crate` / `mod.rs`.

Guiding rule: **build the engine the game needs, not a general-purpose engine.**

## Repo layout (planned)

```
Cargo.toml          # workspace manifest
docs/               # design docs — source of truth
lib/
  core/             # math, time, error, log, ids
  ecs/              # custom ECS
  window/           # winit
  input/            # input → Resource per frame
  render/           # wgpu pipelines, sprite batcher, tilemap
  assets/           # textures, atlases, hot reload
  ui/               # egui + wgpu plumbing
  editor/           # dev-only inspector (feature-flagged)
  audio/            # kira wrapper
src/                # the Spark game on top of the engine
assets/             # textures, shaders, audio
```

Workspace conversion happens when the first sub-crate lands; see [`docs/PLAN.md`](./docs/PLAN.md).

## Commands

```bash
cargo build           # compile
cargo run             # run
cargo test            # run tests
cargo check           # type-check
cargo clippy          # lint
cargo fmt             # format
```

## Docs

- [`docs/PLAN.md`](./docs/PLAN.md) — overall plan, module graph, milestones M1–M6+
- [`docs/ECS_DESIGN.md`](./docs/ECS_DESIGN.md) — full ECS architecture, phased build plan
- [`docs/GAME_DESIGN.md`](./docs/GAME_DESIGN.md) — game concept, mechanics, MVP scope
- [`docs/UI_DESIGN.md`](./docs/UI_DESIGN.md) — egui-based UI strategy, editor vs game UI split

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](./LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
