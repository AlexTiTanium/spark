# Spark Renderer — Build Roadmap

This document turns the concept ([`RENDER_CONCEPT.md`](./RENDER_CONCEPT.md)) and the target API ([`RENDER_API.md`](./RENDER_API.md)) into an ordered, buildable plan. It answers one question: in what order do we build the renderer so that every step is additive, every step produces something visible, and the final state is the API we designed.

The renderer milestones are labelled `R0` through `R12` to keep them distinct from the engine milestones `M1`–`M6` in [`PLAN.md`](./PLAN.md). The renderer track is not a replacement for that plan; it is a sub-track that begins once its prerequisites are met.

## Prerequisites

The renderer is an ECS plugin from its first line. It therefore cannot begin until the ECS can host a plugin — that is, until the ECS has `App`, the `Plugin` trait, ordered execution phases, function-systems, and `Commands`. In the terms of [`ECS_DESIGN.md`](./ECS_DESIGN.md) that is the full Phase 1 of the ECS build plan; in the terms of [`PLAN.md`](./PLAN.md) it is the ECS-foundation milestones. `R0` starts the day that is done. The exact scheduling mechanism that drives the renderer's per-frame Workload — what the renderer's phases are formally called and how they are ordered — is an ECS-layer concern; the roadmap below assumes only that an init phase and an ordered per-frame Workload exist, and leaves their precise spelling to the ECS.

## How to read the roadmap

Three rules govern every milestone.

The first rule is that **each milestone is additive.** It extends exactly one layer, or adds one component type, system, or pass node. It never changes the meaning of something an earlier milestone built. Where a milestone introduces something new, the description below also states explicitly what stays untouched — because "what does not change" is as much a part of the plan as what does.

The second rule is that **each milestone ends in a screenshot.** A milestone is not finished when the code compiles; it is finished when the game's debug scene visibly uses it. This is the discipline that keeps a custom renderer on a custom ECS from disappearing into months of invisible plumbing.

The third rule is the **sandbox pattern.** Renderer experiments — the first triangle above all — live in a `src/sandbox/` plugin, not in the renderer crate. The renderer crate `lib/render` stays sterile: it receives only production infrastructure. The triangle is an experiment, so it lives in the sandbox; when an experiment matures, it graduates into game code.

The milestones are grouped into four phases. Phase one builds the skeleton. Phase two signs the contract that keeps 3D reachable. Phase three completes the mature 2D pipeline. Phase four bends the renderer beyond 2D, and is taken up only opportunistically.

## Phase one — the skeleton (R0–R3)

**R0 — One hardcoded triangle.** This milestone builds the Gpu layer and the bare bones of the Pass layer, and nothing else. The Gpu layer comes up as a Resource wrapping the `wgpu` device, queue, and surface. The render graph comes up as a Resource holding a single pass node whose record step does nothing but set a pipeline and draw three vertices. A render system walks that one-node graph each frame. There are no Items, no Scene components, and no extract step. The whole point of R0 is to prove that the renderer's skeleton — a Resource-held Gpu, a Resource-held graph, a system that runs it inside the per-frame Workload — works end to end. The triangle is hardcoded and lives in the sandbox plugin. The deliverable is a window with a triangle in it.

**R1 — A textured quad.** This milestone extends the Pass layer with a UV vertex format and a texture bind group, and introduces the first asset: a `Texture` loaded from a file. A quad's vertex and index buffers become a Resource. The Gpu layer is not touched. The render graph keeps exactly one node. The deliverable is a window showing an image on a quad.

**R2 — The `Sprite` component and the staged pipeline.** This is where the renderer becomes ECS-shaped and the first game-facing API appears. The Scene layer gains the `Sprite` component. The Items layer gains the `Extracted<Sprite>` working data. The Extract, Prepare, Queue, and Record systems become real — for one entity. The lesson of R2 is that the staged pipeline from the concept document is now genuinely running: a `Sprite` spawned in game code is extracted, prepared, queued, and recorded into a draw. The deliverable is a sprite placed by game code, using the built-in sprite material.

**R3 — Batched sprites and the atlas.** This milestone extends the Items layer with sprite batches grouped by atlas page, extends the Scene layer with the atlas registry, and extends the Pass layer with an instanced draw of many quads per atlas. The render graph's shape does not change — it is still one main pass. The deliverable is thousands of sprites drawn in a few draw calls, with no batch concept visible in game code.

**R3 is an audit checkpoint.** Before proceeding, the layering is tested deliberately: it must be possible to add a second sprite-like component — `AnimatedSprite` is the natural candidate — by touching only the Scene layer and adding one Items-layer batcher, with nothing in the Pass or Gpu layers changing. If that is not possible, the layer boundaries are wrong, and this is the moment to correct them, while the renderer is still small.

## Phase two — the 2D-to-3D contract (R4–R6)

This phase builds the parts of the renderer where the decisions from section six of the concept document are made concrete. By the end of it, the renderer is committed to a shape that can later become 3D.

**R4 — The `Camera2d` View, and the `Painter`.** This milestone makes the camera a View — an entity carrying a projection and a target — rather than a global matrix. A camera uniform is introduced, and the sprite pipeline reads the view from a bind group. The render graph is now executed once per active View. Alongside the camera, the `Painter` immediate-mode API arrives: with a camera in place, its world-space and `relative-to` coordinate modes become meaningful, and a screen-space `Painter` can in fact be pulled earlier if a debug need appears. R4 opens the first of the doors the concept document insists must be kept open. The deliverable is a pannable, zoomable view of a scene, plus immediate-mode debug drawing.

**R5 — Depth and Y-sorting.** This milestone extends the Pass layer with a depth attachment as a graph resource, and extends the Items layer so that batch keys carry a depth value and items sort by it. Two phases appear — an opaque phase sorted front-to-back and a transparent phase sorted back-to-front — and the `YSort` marker component lets depth be derived from world Y. R5 opens the second 3D-readiness door: a real depth buffer, present even in pure 2D. The deliverable is a scene where a worker correctly hides behind a building in front of it.

**R6 — The `Tilemap`.** This milestone extends the Scene layer with the `Tilemap` component, the Items layer with tilemap-chunk items, and the Pass layer with a tilemap pipeline that draws one quad per chunk. It touches neither the Gpu layer, nor the graph topology, nor the atlas system, nor the sprite path — the tilemap is a sibling of sprites, not a rewrite of them. The deliverable is a tiled terrain map under the sprites, with correct depth between the two.

## Phase three — the mature 2D pipeline (R7–R10)

**R7 — Custom materials.** This milestone builds the custom-material system: the `Material2d` derive, the `#[fragment("...")]` shader link, the WGSL prelude that supplies `VertexOutput` and `sample_sprite`, the `app.add_material::<M>()` type registration, and the pipeline cache that compiles a material's shader. A material is a plain component, so attaching one is just spawning the struct on an entity. The four shape-and-shader cases from the API document — a colored quad, a colored quad with a shader, a textured sprite with a shader, a custom-shaded mesh — all work at the end of R7. The deliverable is a sprite visibly running a custom WGSL effect.

**R8 — The `egui` UI overlay.** This milestone extends the Pass layer with a UI pass node that reads the color target and writes it back, and integrates `egui-wgpu` as a self-contained pass. UI draw lists are an Items source that does not come from the ECS, which the graph handles without caring. Nothing below the Pass layer is touched; `egui` knows nothing of sprites and sprites know nothing of `egui`. The deliverable is the game's HUD drawn over the world.

**R9 — Visualization overlays and effect passes.** This milestone is where the Spark-specific rendering lives. The `Painter` is used heavily for the dynamic overlays — power-flow arrows, demand heatmaps — recomputed each frame. The effect-marker mechanism arrives: a marker component such as `Outline` or `Highlight` is consumed by its own render-graph pass, so effects compose freely with each other and with an entity's material. The engine provides the marker-drives-a-pass mechanism; the specific effect components are defined game-side as they are needed. The deliverable is a selectable building with an outline and a highlight, and a working power-flow overlay.

**R10 — A post-processing pass.** This milestone extends the Pass layer, and only the Pass layer, with an offscreen HDR target: the main pass writes into it, and a post pass reads it and writes the swapchain. It introduces no new Scene or Items concept. R10 exists to prove that the render graph is genuinely a graph and not a hard-coded sequence. At the end of R10 the mature 2D pipeline is complete.

## Phase four — beyond 2D (R11–R12)

This phase is taken up only when the game genuinely calls for it. The work below is reachable precisely because the doors in phase two were kept open; if the game stays 2D forever, this phase is simply never built, and nothing is lost.

**R11 — 2.5D and isometric.** This milestone extends the Scene layer with a camera projection that can be orthographic, isometric, or — later — perspective, and extends the Items layer so the sort key accounts for a Y-based depth ordering. It touches neither the Pass topology nor the Gpu layer. The deliverable is the same world rendered in an isometric projection.

**R12 — 3D meshes.** This milestone extends the Scene layer with `Mesh3d`, `Transform3d`, `Material3d`, and `Light3d`, extends the Items layer with a 3D opaque phase, and extends the Pass layer with a 3D mesh pass. The render graph gains a node; the sprite path keeps working unchanged. The deliverable is a 3D mesh and a sprite drawn correctly in the same frame.

## The horizon — GPU-driven rendering

Beyond R12 lies GPU-driven rendering: building the draw-indirect buffer on the GPU, a compute culling pass, bindless texture arrays. This is the same direction large `wgpu`-based engines have taken, and it is the natural endpoint of the architecture. It is deliberately *not* a scheduled milestone. It is taken up only if and when profiling shows the renderer to be CPU-bound at a scale — many tens of thousands of entities — that Spark may never reach. The architecture leaves the door open; the roadmap does not walk through it on a schedule.

## Checkpoints and the thresholds that would change the plan

The R3 audit, described above, is the one mandatory checkpoint: if a second sprite-like component cannot be added by touching only the upper layers, the boundaries are re-cut before continuing.

Beyond that, a few measured thresholds would change the plan. If at R3 the renderer spends more than a few milliseconds of CPU per frame on a few thousand sprites, the retained item-caching approach — only re-extracting entities whose components changed — is brought forward to R4 rather than left as a later optimization. If at R10 intermediate render targets cause visible GPU memory pressure, transient-resource aliasing is introduced into the render graph at that point, since that is exactly the situation the technique exists for. If the game's day-and-night cycle ever calls for shadow casters or screen-space lighting, that is the moment the render graph stops being a hand-ordered list and becomes a true dependency-sorted graph. And if, at R9, a non-trivial new visualization cannot be authored in roughly a day, the boundary between the Items and Pass layers is leaking and is re-cut before more overlays are built.

## Sequencing with the project plan

The renderer track sits on top of the engine track in [`PLAN.md`](./PLAN.md). The window and the ECS come first; `R0` begins once the ECS can host a plugin. From that point the renderer milestones `R0`–`R12` proceed in order, each one additive over the last, each one ending in a screenshot — until the renderer is whatever picture Spark the game needs, and no more than that.
