# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project state

**Spark** is a Rust learning project: a custom 2D game engine built module-by-module, plus **Spark** itself — a power-grid + city simulator with indirect ("designate, don't control") gameplay.

The repo is in early **M1 (workspace foundations)**. The Cargo workspace is set up (`Cargo.toml` is a virtual manifest, members = `lib/*` + `src`); the plugin harness lives in `spark-core` (`Application`, `Plugin`, `EngineError`), `spark-log` provides `LogPlugin`, `spark-window` provides `WindowPlugin`, and `spark-ecs` exposes a `World` + `add_resource` foundation that `Application` embeds. `src/` holds the `spark` binary, which composes those plugins. Remaining engine crates (`spark-input`, `spark-render`, …) and game modules under `src/game/` are not yet created. The design docs in `docs/` are still the source of truth for what to build next.

Before writing code, read the design docs:

- `docs/PLAN.md` — overall plan, repo structure, module dependency graph, milestones (M1–M6+), outdated patterns to avoid
- `docs/ECS_DESIGN.md` — full architecture for the roll-your-own ECS in `lib/ecs/`, including phased build plan (stages 1–24)
- `docs/GAME_DESIGN.md` — game concept, mechanics, MVP scope (v1)
- `docs/UI_DESIGN.md` — UI strategy (egui-based, no fork; editor UI vs game UI split)

## Commands

Standard Cargo, run from the workspace root. CI runs the same commands on every push/PR.

```bash
cargo build                       # build every workspace member
cargo run -p spark                # run the binary (src/main.rs)
cargo test --workspace            # run all tests + doc tests
cargo test -p spark-core <name>   # run one crate's tests by name substring
cargo check --workspace           # fast type-check across the workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings  # lint
cargo fmt --all                   # format every member
```

Workspace layout matches `docs/PLAN.md`: members = `lib/*` + `src`. Shared settings (edition, MSRV, license, lints) live in `[workspace.package]` and `[workspace.lints]` in the root `Cargo.toml`; each member opts in with `<field>.workspace = true` and `[lints] workspace = true`. New engine crates land under `lib/<name>/` and are picked up automatically by the `lib/*` glob.

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

## Documentation requirements (tiered)

This is a **study project**: the code should teach the reader. But teaching value comes from *insight*, not from ceremonial headings on every getter. Match doc depth to code complexity.

**Every** function, struct, enum, trait, `impl` block, and module carries a rustdoc comment (`///` on items, `//!` at modules). The only exemption is `#[derive]`-generated boilerplate. Pick the tier from the table below.

| Tier | Applies to | What to write |
|---|---|---|
| **1 — Trivial** | Builders, simple getters, obvious wrappers, one-line conversions, plain re-export modules | One-line summary + `# Examples` doc test. Nothing else. |
| **2 — Meaningful** | Functions returning `Result`, types with an invariant, anything with a real footgun, module headers | Summary + 1–3 short paragraphs covering whichever of {how it works, why this design, key pitfall} actually has content + `# Examples`. Add `# Errors` if it returns `Result` (clippy requires it). |
| **3 — Teaching** | Load-bearing engine code where the *why* is the entire point: `EntityAllocator`, `ComponentStorage`, schedulers, query planners, the workload graph | Full deep dive: Summary, Logic, Memory layout (ASCII), Why it works (the invariant), How to use, How NOT to use, Examples. |

**Rules of thumb:**

- If a section would say "this is obvious from the signature," delete it.
- Don't fabricate "How NOT to use" when there's no real footgun. Don't write a "Memory layout" block for a struct with three independent fields.
- "How to use" and "Examples" usually duplicate each other — keep only `# Examples` unless the prose adds real context the doc test can't show.
- Doc tests must compile and pass (`cargo test --doc`). Use `no_run` only when running the code would block (event loops) or require the network.

### Tier 1 example

```rust
/// Sets the window title. Accepts `&str` or `String` via `Into<String>`.
///
/// # Examples
///
/// ```
/// let cfg = WindowConfig::default().with_title("Hello");
/// assert_eq!(cfg.title, "Hello");
/// ```
#[must_use]
pub fn with_title(mut self, title: impl Into<String>) -> Self {
    self.title = title.into();
    self
}
```

### Tier 2 example

```rust
/// Opens a window and drives the OS event loop until the user closes it.
///
/// Builds a `winit` event loop, constructs an internal handler that owns
/// the window, and hands the loop to `EventLoop::run_app`. Blocks the
/// calling thread until the user closes the window.
///
/// # Errors
///
/// Returns [`WindowError::EventLoop`] if the OS event loop cannot be
/// created, or [`WindowError::Os`] if the window cannot be created.
///
/// # Examples
///
/// ```no_run
/// spark_window::run(spark_window::WindowConfig::default())?;
/// # Ok::<(), spark_window::WindowError>(())
/// ```
pub fn run(config: WindowConfig) -> Result<(), WindowError> { /* … */ }
```

### Tier 3 example (use this depth only when the design *is* the lesson)

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

### Config files (TOML, YAML)

Same tiering applies, scaled down:

- **File-level header**: 2–4 lines. What the file is, what's special about it. No multi-paragraph essays.
- **Per-key comments**: only when the key choice or value is non-obvious. `edition = "2024"` needs no comment; `resolver = "3"` does (because *why* we set it explicitly is the interesting part). For pinned deps, one short line per dep saying what it does and why it's here.
- Skip comments on plain workspace inheritance (`edition.workspace = true`, `license.workspace = true`) — group them under a single one-line header like `# Inherited from [workspace.package].`

### Enforcement

- After writing or editing code, re-read your docs and ask of each section: *would deleting this lose information a reader needs?* If no, delete it.
- Run `cargo test --doc` after editing doc tests.
- Module headers (`//!`) follow the same tiering as items: trivial re-export modules get one line; modules with real responsibility get Tier 2.

## Project conventions

These are non-obvious and worth keeping in mind:

- **Modern Rust only.** Edition 2024, `rust-version = "1.95"`. Avoid all "orange-era" patterns documented in `PLAN.md` § "Outdated patterns to avoid": no `extern crate`, no `mod.rs` (use `foo.rs` + `foo/`), no `lazy_static!` (use `std::sync::LazyLock`), no `try!`, no `Box<dyn Error>` (use `thiserror` for libs, `anyhow` for apps), no `failure`/`error-chain`, no hand-rolled GL (use `wgpu` + WGSL), no `log`+`env_logger` (use `tracing` + `tracing-subscriber`).
- **Pinned exact deps.** Dependency versions are pinned to exact versions (e.g. `"1.1.0"`, not `"1.1"` or `"^1.1.0"`).
- **Workspace lints.** `[workspace.lints.rust] unsafe_code = "warn"` and `[workspace.lints.clippy] pedantic = "warn"`. No `unsafe` until profiling demands it.
- **Crate naming.** Engine crates are named `spark-<module>` (e.g. `spark-ecs`, `spark-ecs-macros`) per the `Cargo.toml` examples in `ECS_DESIGN.md`. Internal paths use the unprefixed module name.
- **Reference repo.** The author's earlier [AlexTiTanium/orange](https://github.com/AlexTiTanium/orange) is **reference material only** — borrow layout ideas, but write everything fresh. Do not import its code patterns or dependency versions.
- **No Dependabot. Ever.** Dependabot is *persona non grata* on this project — do not propose it, do not add `.github/dependabot.yml`, do not suggest enabling Dependabot security updates in repo settings. The owner has made an explicit, standing decision against it; dependency bumps are handled manually. If a different automation surfaces the same need later (e.g. `cargo audit` in CI), that may be discussed, but Dependabot itself is off the table.
- **Separation of concerns.** Process-wide state (panic hook, root error type) lives in `spark-core`. Per-layer events and typed errors live in the layer crate (`WindowError` in `spark-window`, future `RenderError` in `spark-render`, …). Logging install lives in a dedicated `spark-log` crate. The binary composes everything via `Application::new().add_plugin(...).run()` — every subsystem is a `Plugin`. Libraries never install global state on their own.
- **Crates vs modules.** A new crate under `lib/` is justified by a **distinct architectural concern with its own deps and lifecycle** — windowing, logging, input, rendering, ECS, audio. Pure foundation types with no third-party deps and no lifecycle behaviour (math, ids, time newtypes, IDs) live as **modules inside `spark-core`**. Rule of thumb: if it would naturally implement `Plugin`, give it its own crate.
  - **Proc-macro carve-out.** Rust forces proc macros into a separate `proc-macro = true` crate (they can export nothing else), but a proc-macro crate is *not* a distinct architectural concern — it's a build-time artefact of its parent. Nest it inside the parent at `lib/<parent>/macros/` (e.g. `spark-ecs-macros` lives at `lib/ecs/macros/`) and re-export its derives from the parent so consumers depend on and import only the parent crate. The single outward trace is one mandatory entry in the workspace `members` list.
  - **Dep-graph note.** `spark-ecs` is the *deepest* crate (stdlib only); `spark-core` depends on it because `Application` embeds a `World`. This mirrors Bevy's `bevy_ecs → bevy_app` layering and keeps the cycle-free shape Cargo requires. Crates above `core` reach `World` through the `spark_core::World` re-export, not by adding a direct `spark-ecs` dep.
- **`Application` is plugin-driven from day one.** Through M1–M3 the binary uses `spark_core::Application` — an ordered list of plugins grouped by stage. Public surface is `new`, `add_plugin<P: Plugin>(P)`, `add_stage(name)`, `run()`. No `with_window` / `with_log_filter`-style methods — every subsystem is a `Plugin`. When `spark-ecs` lands in M4, `Plugin::run` gains `&mut World` access; the trait extends, doesn't break.
- **Error-handling rule (engine-wide).** *Inside* any `spark-*` library crate, define a typed `XError` enum with `thiserror` (`WindowError`, future `InputError`, `RenderError`). *At* every Plugin → Application seam, and in every `fn main` signature, use `spark_core::EngineError` — a `pub use anyhow::Error as EngineError` alias. The typed-to-erased conversion happens via `?` inside each plugin's `run()`, automatically (anyhow has a blanket `From<E: StdError + Send + Sync + 'static>`). Plugins never construct `EngineError` themselves. Never `Box<dyn Error>` in public APIs; never a typed `EngineError` enum with per-layer variants (would force a `spark-core` → every-layer cycle).
- **Bare-name imports at call sites.** Pull names in with `use` (`use spark_core::Application; use spark_log::LogPlugin;`); avoid the fully-qualified `spark_core::Application::new()` at call sites — it makes builders unreadable. On a name collision, alias at the import (`use spark_log::LogPlugin as LogPluginLog;`), don't drag the full path into call sites.
