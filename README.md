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

## Browsing the API docs

Every engine crate ships its public-API guide as a `README.md` next to its `Cargo.toml` (e.g. [`lib/log/README.md`](./lib/log/README.md)), and that same file is the rustdoc front page for the crate. To render every workspace crate's docs into an interlinked HTML site and open it in your browser:

```bash
cargo doc --workspace --no-deps --open
```

- `--workspace` includes every member under `lib/*` plus the `spark` binary in `src/`.
- `--no-deps` skips third-party dependencies (`tracing`, `winit`, `thiserror`, …) so the build is fast and the sidebar only lists your crates.
- `--open` launches `target/doc/spark/index.html` in your default browser.

For a single crate, swap `--workspace` for `-p <crate>`:

```bash
cargo doc -p spark-log --no-deps --open
```

### Refreshing after changes

`cargo doc` only rebuilds what it thinks is stale — it does **not** delete old files from `target/doc/`. If you previously ran without `--no-deps` (or renamed/removed an item), the sidebar will still show those leftovers. Wipe the doc cache and rebuild:

```bash
cargo clean --doc                          # deletes target/doc only
cargo doc --workspace --no-deps --open
```

`cargo clean --doc` leaves your compiled binaries in `target/debug` and `target/release` untouched — it's cheap and safe.

## Design docs

- [`docs/PLAN.md`](./docs/PLAN.md) — overall plan, module graph, milestones M1–M6+
- [`docs/ECS_DESIGN.md`](./docs/ECS_DESIGN.md) — full ECS architecture, phased build plan
- [`docs/GAME_DESIGN.md`](./docs/GAME_DESIGN.md) — game concept, mechanics, MVP scope
- [`docs/UI_DESIGN.md`](./docs/UI_DESIGN.md) — egui-based UI strategy, editor vs game UI split

## License

Dual-licensed under either of

- Apache License, Version 2.0 ([`LICENSE-APACHE`](./LICENSE-APACHE) or <http://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([`LICENSE-MIT`](./LICENSE-MIT) or <http://opensource.org/licenses/MIT>)

at your option.
