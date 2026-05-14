# Spark UI — Architecture & Decisions

This document records the UI-layer decisions for Spark: which UI library, which add-ons, and how the editor UI and the game UI relate.

See **[PLAN.md](./PLAN.md)** for the overall project plan and **[ECS_DESIGN.md](./ECS_DESIGN.md)** for the ECS layer this UI sits on top of.

## TL;DR

- **UI library: `egui`** (MIT). Selected after surveying the Rust UI landscape; nothing else fits "retained-feeling UI as a layer inside a custom wgpu engine, permissive license".
- **No fork of egui.** Everything we initially thought needed a fork is achievable via add-on crates plus a thin orchestration crate of our own.
- **Two distinct UIs, two distinct rule sets.** The **editor UI** is a developer tool: vanilla egui, ugly is fine, lives behind a feature flag and is excluded from release builds. The **game UI** is a player-facing surface: vanilla egui + `egui_taffy` (layout) + our own theme/widget/animation crates, iterated over time.
- **No UI work blocks game work.** Milestones M7–M13 ship on vanilla egui. The custom game-UI stack is layered on at M14, *after* real screens exist to inform what we actually need.

## Why `egui` (and why not the others)

The bar was: render *as a layer* inside a custom wgpu engine; permissive license (MIT/Apache-2.0); production-credible; alive and maintained.

| Library | Verdict | Reason |
|---|---|---|
| `slint` | Rejected | Mature and beautiful, but GPLv3 default — non-starter for permissive licensing. |
| `dioxus` | Rejected | Webview-based overlay; doesn't compose with a custom wgpu pipeline. |
| `gpui` | Rejected | Framework-as-root (wants to *be* the app, not be a layer); no Windows support; pre-1.0; Metal-only, not wgpu. |
| `violet` / `quirky` / `blinc` | Rejected | All at 0.x, solo maintainers, integration paths undocumented — same class of risk we already hit with `orange`. |
| **`egui`** | **Selected** | MIT, immediate-mode (we orchestrate, the UI library doesn't), well-documented and well-trodden wgpu integration path (`egui-wgpu` + `egui-winit`), large active community. |

**Conclusion:** "Beautiful retained-mode UI as a layer in a custom wgpu engine, with a permissive license" is a genuine gap in the 2026 Rust ecosystem. We accept that gap, ship on egui, and add what we need on top.

## No fork — additions on top, not modifications to the base

A fork looked tempting for layout, theming, and animation. It is not necessary:

- **Layout** — `egui_taffy` provides Flexbox/Grid layout via egui's public multi-pass API (egui ≥ 0.29). No internal patches required.
- **Animation** — egui core has the primitive animation helpers we need; richer orchestration (timelines, easing curves, sequencing) is a thin wrapper crate of our own on top of the public API.
- **Theming, custom widgets, 9-slice art frames** — all achievable via `egui::Painter` and the public style/visuals APIs.

**Engineering trade-off:** an add-on degrades gracefully — if a layer breaks, we lose *that* layer. A fork degrades catastrophically — if it diverges, we lose *everything* and pay a permanent merge tax against upstream egui.

## Two UIs, two rule sets

This is the central organising decision. We explicitly split the editor UI from the game UI; they live in different crates, have different dependencies, ship under different conditions, and are held to different design standards.

### Editor UI — developer tool

A developer-facing inspector and debugger. Consumes the reflection APIs that the ECS exposes in Phase 2 (`inspect_entity`, `FrameTrace`, `ChangeLog`, `CommandLog`).

**Surface area:**

- Entity inspector (list entities, drill into components on a single entity)
- Resource list with debug-formatted state
- Workload / system graph (the DAG built by the scheduler)
- Per-system timings from `FrameTrace`
- Command log per frame
- Component-mutation change log for `#[derive(Trace)]` types

**Rules:**

- Vanilla `egui`. **Visual polish is explicitly out of scope.**
- No custom widgets, no theme work, no art assets, no animations.
- Lives in its own crate (`lib/editor/`) as an opt-in `Plugin`.
- **Feature-flagged out of release builds.** The editor is a development convenience, not a shipped surface.
- Cost is near-zero — it is a pure consumer of reflection APIs the ECS exposes anyway.

### Game UI — player-facing surface

The actual interface Spark players interact with: HUD, planning panels, building inspectors, power-grid overlays, tech tree, alerts.

**Surface area** (driven by `GAME_DESIGN.md`):

- HUD: capital, time, power supply/demand ratio, alerts
- Planning panels: pick a building, place a plan, lay a transmission line
- Building inspectors: state, throughput, workers, cost lines
- Power-grid overlay: producers, consumers, line health
- Tech tree (later milestones)
- Alert / event surface

**Rules:**

- Custom-styled, brand-consistent, animated where it serves clarity.
- Lives under `src/game/ui/` (game-side, not engine-side).
- Built on the stack below.

### Game-UI stack (composed at M14)

Bottom to top:

| Layer | Source | Role |
|---|---|---|
| `egui` | external (MIT) | Immediate-mode UI core, wgpu integration, input handling |
| `egui_taffy` | external | Flexbox/Grid layout via egui's multi-pass API |
| `spark-ui-theme` | ours | Centralised theme, color tokens, typography, 9-slice frame art |
| `spark-ui-widgets` | ours | Domain widgets (capital meter, supply/demand gauge, building card, plan-placement cursor, tech-node, …) using `egui::Painter` |
| `spark-ui-anim` | ours | Thin orchestration layer for timelines, easing, sequenced reveals |

The three `spark-ui-*` crates are deliberately small and additive. Each can be skipped initially and added when an actual game screen demands it.

### Visual ceiling — honest expectations

- **SimCity-level polish: confidently achievable.**
- **Static Anno-style screenshot: achievable with good art.**
- **A fully "alive" animated Anno-style UI: close but not free** — bounded by immediate-mode constraints and, more than anything, by art investment.

**The dominant factor is art, not the UI library.** Roughly ~70% of what makes Anno- or SimCity-class UIs feel premium is illustration, iconography, and motion design — work that lives outside the UI framework.

## Timing — when each piece lands

The game-UI stack is layered in *after* we have real screens to inform what's actually needed. Building widgets and animations against imagined requirements is wasted work.

- **M7–M13 (game-loop milestones):** vanilla `egui` only. We're building the game, not fighting the UI.
- **M14 (UI stack decision point):** by this milestone there are concrete Spark screens (planning, inspectors, overlays, alerts). The custom stack — `egui_taffy` for layout, `spark-ui-theme`, `spark-ui-widgets`, `spark-ui-anim` — is composed onto a *working* game, not invented up front.
- **Editor UI:** stays on vanilla `egui` forever. It is a tool, not a product.

If a particular screen at M10 or M11 demands custom widgets earlier, we pull that work forward — but the default is to *defer until the screen exists*.

## Crate layout

Engine-side (`lib/`):

```
lib/
├── ui/                 # engine-side egui plugin: wgpu integration,
│                       # input plumbing, frame lifecycle, font/cursor setup.
│                       # Provides EguiContext as a Resource.
└── editor/             # developer UI plugin (feature-flagged).
                        # Vanilla egui. Consumes ECS reflection APIs.
```

Game-side (`src/game/ui/`):

```
src/game/ui/
├── theme/              # spark-ui-theme: tokens, typography, 9-slice frames
├── widgets/            # spark-ui-widgets: domain widgets via egui::Painter
├── anim/               # spark-ui-anim: timelines, easing, orchestration
└── screens/            # actual game screens (HUD, planning, inspectors, …)
```

`lib/ui` does the heavy plumbing once: wgpu integration, surface handoff, input forwarding, font setup. Both the editor and the game UI consume an `EguiContext` resource from it. Layout/theme/widgets/animation are composed on top inside `src/game/ui/`.

## Open questions

- Whether `spark-ui-theme`, `spark-ui-widgets`, and `spark-ui-anim` should each be their own crate under `lib/`, or sub-modules under `src/game/ui/`. Default for now: sub-modules under `src/game/ui/` until any of them needs to be reused outside the game.
- Whether the editor crate should expose a stable reflection-driven schema (so tooling can be regenerated) or stay an ad-hoc inspector. Default: ad-hoc until the editor proves useful.
- Whether to evaluate switching to a retained-mode library if/when one matures with a permissive license. Re-check at M14 and again at v1.0.
