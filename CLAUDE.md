# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project state

**Spark** is a Rust learning project: a custom 2D game engine built module-by-module, plus **Spark** itself — a power-grid + city simulator with indirect ("designate, don't control") gameplay.

The repo is in a **design-first, pre-M1** state. Only `src/main.rs` (a `Hello, world!` stub) and a top-level single-package `Cargo.toml` exist. The `lib/` directory is empty. The intended workspace layout, crate breakdown, milestones, and architecture are specified in `docs/` — those documents are the source of truth for what to build next, not the current filesystem.

Before writing code, read the design docs:

- `docs/PLAN.md` — overall plan, repo structure, module dependency graph, milestones (M1–M6+), outdated patterns to avoid
- `docs/ECS_DESIGN.md` — full architecture for the roll-your-own ECS in `lib/ecs/`, including phased build plan (stages 1–24)
- `docs/GAME_DESIGN.md` — game concept, mechanics, MVP scope (v1)
- `docs/UI_DESIGN.md` — UI strategy (egui-based, no fork; editor UI vs game UI split)

## Commands

Standard Cargo — no workspace, custom tooling, or CI is in place yet.

```bash
cargo build           # compile
cargo run             # run main.rs
cargo test            # run all tests
cargo test <name>     # run a single test by name substring
cargo check           # type-check without codegen
cargo clippy          # lint
cargo fmt             # format
```

The workspace structure described in `docs/PLAN.md` (members = `lib/*`, `src`) is **planned but not yet realized**. When creating the first sub-crate, convert `Cargo.toml` to a workspace manifest per the template in `PLAN.md` (resolver = "2", `[workspace.package]` with edition 2024, `[workspace.lints]` with `clippy::pedantic = warn`).

## Architecture

The big picture (only meaningful once code lands; see `docs/PLAN.md` for the canonical version):

- **ECS-centric.** All long-lived state lives in the `World` as either a `Resource` (singleton, one per type) or an `Entity` (many, dynamic). No global statics, no `lazy_static!`, no side allocations. This is non-negotiable per `ECS_DESIGN.md`.
- **Custom ECS.** Bevy-style function-parameter system extraction + Shipyard-inspired named `Workload`s, sparse-set storage, single-threaded scheduler first (parallel via Rayon is Phase 3). No external ECS dependency.
- **Plugin pattern.** Each engine crate (`core`, `ecs`, `window`, `input`, `render`, `assets`, `ui`, `editor`, `audio`) exposes a `Plugin` that registers its resources, events, and systems with the `App`. `main.rs` is just `App::new().add_plugin(...).run()`.
- **Two clocks.** Fixed-timestep (60 Hz) `FixedUpdate` schedule for simulation; variable-rate `Update` + `Render` for display. Schedule order per frame: `First → PreUpdate → (FixedUpdate × N) → Update → PostUpdate → Render → Last`. Commands flush between workloads; events double-buffer across frames.
- **Deterministic simulation.** No `HashMap` iteration in sim systems — leaves the door open for save/replay/multiplayer.
- **Two UIs, two rule sets.** Editor UI = vanilla `egui`, feature-flagged out of release, lives in `lib/editor/`, consumes ECS reflection APIs. Game UI = `egui` + `egui_taffy` + custom `spark-ui-*` layers under `src/game/ui/`. Both share an `EguiContext` resource from `lib/ui/`. Game-UI stack is deferred to M14 — vanilla egui only through M7–M13.
- **Engine and game grow together.** Build the engine the game needs, not a general-purpose engine.

Module dependency graph (lower → higher):

```
core → ecs → window → input → render → assets → ui → editor
                                              ↘ audio
                                            game (in src/) sits on top
```

## Documentation requirements (STRICT — non-negotiable)

This is a **study project**. The whole point is to learn by reading the code later. Documentation is not optional polish — it is the deliverable. Code without documentation is unfinished work.

**Every** function, struct, enum, trait, `impl` block, and module **must** carry a rustdoc comment (`///` on items, `//!` at the top of modules) covering all of the sections below. The only exemption: trivial one-line getters/setters and `#[derive]`-generated boilerplate. When in doubt, document.

Required sections, in this order:

1. **Summary** — one short line in plain language. What this is, why it exists. No jargon a beginner wouldn't recognise; if a term is unavoidable, link or briefly define it.
2. **Logic** — short paragraph explaining how it works, step by step where useful. Beginner-friendly.
3. **Memory layout** — for any data structure, an ASCII schema of the fields and how they relate. Use ```` ```text ```` fenced blocks. This is mandatory for storages, allocators, queues, graphs, anything with non-trivial internal structure.
4. **Why it works** — the invariant or insight that makes the implementation correct (e.g. "generation bump invalidates stale handles", "sparse[entity.index] always points to the dense slot or is `None`").
5. **How to use** — typical call pattern, with realistic context (not just signature restated).
6. **How NOT to use** — pitfalls, panics, footguns, wrong assumptions, ordering constraints, what happens if you call it twice, etc. Be explicit.
7. **`# Examples`** — at least one runnable doc test with `assert_eq!` / `assert!`. Doc tests must compile and pass (`cargo test --doc`).

Reference template (apply to every non-trivial item):

```rust
/// Allocates a new entity, reusing a freed slot when one is available.
///
/// # Logic
///
/// Pops the most recently freed index off `free_list`; if empty,
/// extends `generation` with a new index. Returns `Entity { index,
/// generation: generation[index] }`.
///
/// # Memory layout
///
/// ```text
/// free_list:  [3, 7, 12]          ← indices ready for reuse (LIFO)
/// generation: [0, 1, 0, 2, 0, …]  ← bumped each time slot N is destroyed
/// ```
///
/// # Why it works
///
/// Destroying a slot bumps its generation. Any stale `Entity` handle
/// pointing at that index now mismatches and `is_alive` returns `false`.
/// `(index, generation)` uniquely names a live entity for all time.
///
/// # How to use
///
/// ```
/// let mut alloc = EntityAllocator::new();
/// let e = alloc.allocate();
/// assert!(alloc.is_alive(e));
/// ```
///
/// # How NOT to use
///
/// - Do not retain `Entity` handles across `World::clear()` — index
///   space is reused.
/// - Do not compare `Entity` by `index` alone; always compare the full
///   struct or use `is_alive`.
///
/// # Examples
///
/// ```
/// let mut alloc = EntityAllocator::new();
/// let a = alloc.allocate();
/// alloc.destroy(a);
/// let b = alloc.allocate();          // reuses a's slot
/// assert_eq!(a.index, b.index);
/// assert_ne!(a.generation, b.generation);
/// assert!(!alloc.is_alive(a));
/// assert!(alloc.is_alive(b));
/// ```
pub fn allocate(&mut self) -> Entity { /* … */ }
```

**Enforcement rules — apply to yourself without being asked:**

- Before reporting any code change as complete, **re-read every function, struct, and module you wrote or modified** and verify all required sections are present and accurate.
- If any item is missing documentation or has thin/placeholder sections, **do not stop — fix it**. Loop until the bar is met. Treat missing docs the same as a failing test.
- Run `cargo test --doc` after writing or editing doc tests. A doc test that doesn't compile is a broken doc.
- Module-level (`//!`) docs at the top of each file must describe what the module is for, what it exposes, and how it fits with neighbouring modules — the same logic/memory/why/use/don't structure applies, scaled up.
- "Beginner-friendly" is the test: a reader who is learning Rust should be able to follow the doc without prior context from elsewhere in the codebase.
- **Config files count too.** TOML and other config files (`Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, CI YAML, etc.) must carry the same beginner-friendly comments — file-level header (summary/logic/why/how to use/how NOT to use, with examples where it helps), plus an inline comment on every non-trivial key explaining what the key does, the set of valid values, and the effect of the chosen value. The "ASCII memory-layout schema" section is optional for config files (skip it unless the file describes a data layout); everything else applies. Use `#` for comments in TOML/YAML.

## Project conventions

These are non-obvious and worth keeping in mind:

- **Modern Rust only.** Edition 2024, `rust-version = "1.95"`. Avoid all "orange-era" patterns documented in `PLAN.md` § "Outdated patterns to avoid": no `extern crate`, no `mod.rs` (use `foo.rs` + `foo/`), no `lazy_static!` (use `std::sync::LazyLock`), no `try!`, no `Box<dyn Error>` (use `thiserror` for libs, `anyhow` for apps), no `failure`/`error-chain`, no hand-rolled GL (use `wgpu` + WGSL), no `log`+`env_logger` (use `tracing` + `tracing-subscriber`).
- **Pinned exact deps.** Dependency versions are pinned to exact versions (e.g. `"1.1.0"`, not `"1.1"` or `"^1.1.0"`).
- **Workspace lints.** `[workspace.lints.rust] unsafe_code = "warn"` and `[workspace.lints.clippy] pedantic = "warn"`. No `unsafe` until profiling demands it.
- **Crate naming.** Engine crates are named `spark-<module>` (e.g. `spark-ecs`, `spark-ecs-derive`) per the `Cargo.toml` examples in `ECS_DESIGN.md`. Internal paths use the unprefixed module name.
- **Reference repo.** The author's earlier [AlexTiTanium/orange](https://github.com/AlexTiTanium/orange) is **reference material only** — borrow layout ideas, but write everything fresh. Do not import its code patterns or dependency versions.
- **No Dependabot. Ever.** Dependabot is *persona non grata* on this project — do not propose it, do not add `.github/dependabot.yml`, do not suggest enabling Dependabot security updates in repo settings. The owner has made an explicit, standing decision against it; dependency bumps are handled manually. If a different automation surfaces the same need later (e.g. `cargo audit` in CI), that may be discussed, but Dependabot itself is off the table.
