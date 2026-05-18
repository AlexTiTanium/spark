# Spark Renderer — Architecture Concept

This document records the *concept* behind Spark's renderer: the single organizing philosophy, the layered model, and the principle of growth. It is the "why" and the "shape" — not the game-facing API (that is [`RENDER_API.md`](./RENDER_API.md)) and not the ordered build plan (that is [`RENDER_ROADMAP.md`](./RENDER_ROADMAP.md)). It sits alongside [`PLAN.md`](./PLAN.md), [`ECS_DESIGN.md`](./ECS_DESIGN.md), [`GAME_DESIGN.md`](./GAME_DESIGN.md), and [`UI_DESIGN.md`](./UI_DESIGN.md).

The renderer is built on `wgpu` (targeting the current `29.x` stable line) with WGSL shaders, and it lives entirely inside Spark's custom ECS. Every piece of long-lived state is a Resource or an Entity; there are no global statics.

## 1. The core idea

The renderer is a single pipeline that **compiles a retained scene, expressed as ordinary ECS data, into one frame of GPU commands.** The game never tells the renderer to draw anything. It describes the world — a `Sprite` here, a `Tilemap` there, a `Camera2d` watching them — by spawning entities and mutating components. The renderer observes that data each frame and produces the picture. The relationship is declarative and one-directional: the game writes, the renderer reads.

Thinking of the renderer as a *compiler* is the most useful mental handle. A compiler takes a high-level description and lowers it, in well-defined stages, into something a machine executes. Spark's renderer does exactly that: the high-level description is the ECS scene, the stages are Extract, Prepare, Queue, Sort, Record, and the machine code is a `wgpu` command buffer. Because it is a compiler and not a command interface, the same input always produces the same output, and every stage can be inspected — which matches the engine's standing rule that there are no black boxes.

## 2. The four layers

The renderer is split into four layers, stacked from the GPU upward to the game. Each layer owns a distinct concern, and the dependency between them is strictly one-directional: a layer may read the layer below it, but never the reverse.

```
+-------------------------------------------------------------+
|  LAYER 4 — SCENE     game-facing components and the camera   |
|    What the game spawns: Sprite, Tilemap, Camera2d, ...      |
|    Knows nothing about wgpu.                                 |
+-------------------------------------------------------------+
|  LAYER 3 — ITEMS     extracted, sorted, batched draw items   |
|    The renderer's working set for the current frame.         |
|    Knows nothing about specific GPU device state.            |
+-------------------------------------------------------------+
|  LAYER 2 — PASS      the render graph: passes and pipelines  |
|    Named passes with declared inputs and outputs.            |
|    Knows wgpu types and WGSL.                                |
+-------------------------------------------------------------+
|  LAYER 1 — GPU       the thin wgpu wrapper                   |
|    Device, queue, surface; Spark's own resource handles.     |
|    The only place wgpu types are spoken aloud.               |
+-------------------------------------------------------------+
                            |
                        wgpu 29.x
```

The **Gpu** layer is a thin, value-oriented wrapper over `wgpu`. It owns the device, queue, and surface, and it issues Spark's own resource handles (`BufferId`, `TextureId`, and so on). It is deliberately *not* a virtual GPU abstraction — `wgpu` is already the cross-platform abstraction, and wrapping it in a second one would be dead weight. The wrapper exists only so that the layers above can hot-reload shaders, label resources, and pool buffers without ever naming a raw `wgpu` type.

The **Pass** layer is the render graph. It owns the set of named passes that make up a frame, each declaring what it reads and what it writes, plus the caches for pipelines, bind groups, and textures. This is the only layer that speaks `wgpu` and WGSL out loud.

The **Items** layer is the renderer's per-frame working set. It takes the game's scene description and turns it into draw items — extracted, sorted into phases, and batched. It knows nothing about specific GPU device state; it deals in "what to draw and in what order," not "how to record it."

The **Scene** layer is everything the game touches. It is the components a game spawns and the camera that observes them. It knows nothing about `wgpu`, nothing about passes, nothing about batching. Its full surface is described in [`RENDER_API.md`](./RENDER_API.md).

The one-directional read rule is the load-bearing constraint of the whole design. A `Sprite` component holds an image handle, a tint, and a depth value — never a `wgpu::Texture`. The moment a `wgpu` type leaks into the Scene layer, the separation is gone and undoing the leak becomes a multi-month refactor. The rule is cheap to keep and ruinously expensive to recover.

### How the layers map onto the ECS

| Renderer concept | ECS shape |
|---|---|
| `Gpu`, the render graph, pipeline and atlas caches, the per-frame item set | Resources (one of each) |
| A sprite, tile chunk, camera, light, custom-shaded shape | Entity plus components |
| An extracted draw item for one entity this frame | Entity in the renderer's working set, tagged with an `Extracted<T>` component |
| A render pass (the main sprite pass, the UI pass, an effect pass) | A pass-node value held inside the render-graph Resource |
| `extract_sprites`, `batch_sprites`, `record_pass`, and so on | Ordinary function-systems with parameter-declared access |
| The renderer's per-frame pipeline as a whole | A named Workload whose phases run in order, with command flushes between them |

## 3. The frame pipeline — and one deliberate divergence from Bevy

Every frame, the renderer runs the same six-stage recipe, in order: **Extract, Prepare, Queue, Sort, Record, Present.** Extract reads the Scene layer and produces Items. Prepare uploads per-frame GPU data. Queue assigns items to passes. Sort orders items within each pass by depth or material. Record walks the render graph and writes the `wgpu` command buffer. Present hands the finished frame to the surface.

This staged shape is borrowed from Bungie's Destiny renderer and from Bevy, which adopted the same idea. Spark keeps the staging because it cleanly separates *what to draw* (Extract produces it) from *how to draw it* (Record consumes it). But Spark makes one deliberate and important departure: **there is only one `World`.**

Bevy runs a separate render sub-application with its own parallel `World`, and Extract is the synchronization point that copies data between the two. That design exists so that the simulation of frame N+1 can run in parallel with the rendering of frame N. It is genuine engineering, and it carries genuine cost — the Bevy team has spent years on the ergonomics of keeping two worlds in sync, and the model is heavy for small scenes. Spark is a 2D top-down simulation; it will be bound by simulation work, not by rendering. It does not need the second world, so it does not pay for it. Extract is simply a Workload phase that copies Scene-layer data into renderer-side Resources within the single `World`. If pipelined rendering is ever genuinely needed, the Workload boundary at the end of Extract is exactly the seam where a double buffer could be introduced later — additively, without disturbing the layers.

## 4. The render graph — keep the interface, defer the machinery

The render graph is the Pass layer's organizing structure: a set of pass nodes, each declaring its input and output resources, executed in dependency order. The pattern comes from Frostbite's FrameGraph and is now standard in large engines, where a frame may contain hundreds of passes and the graph's automatic scheduling — barrier insertion, transient-memory aliasing, pass culling — earns its keep.

Spark is not in that regime and will not be for years. A 2D city builder has roughly five passes. So the concept here is precise: **adopt the render graph's *interface* from the start, and defer its *machinery* until the pass count demands it.** The interface is the part that matters early — passes that declare what they read and write, so that a new pass (a shadow pass, a post-effect, an outline effect) can be dropped in without rewriting the others. The machinery — memory aliasing, automatic barrier reordering, async-compute scheduling — is overkill for five passes and an easy source of bugs. Concretely, the render graph begins life as a small ordered list of pass nodes whose execution order is set by hand. It grows into a true dependency-sorted graph only once there are enough passes that hand-ordering becomes error-prone. The vocabulary is present from day one; the algorithm arrives only when it pays for itself.

## 5. Growth by extension, never by rewrite

The single most important property of this concept is that the renderer grows **additively.** Every milestone in [`RENDER_ROADMAP.md`](./RENDER_ROADMAP.md) adds a component type, a system, or a pass node. No milestone changes the meaning of something that already exists. The four layers, the six-stage pipeline, and the `wgpu` wrapper are written once, early, and then never substantially rewritten. A textured quad extends the sprite path; a tilemap is a sibling of sprites, not a replacement; 3D meshes add a pass node while sprites keep working untouched.

This is the direct application of the project's guiding rule — build the engine the game needs, not a general-purpose engine — to the renderer. It is also the property that justifies designing the four layers up front even though the game is small: a small, layered renderer costs almost nothing extra to build, and it keeps the door to 2.5D and 3D open. A small, monolithic renderer is cheaper today and slams that door shut.

## 6. The 2D-to-3D bridge

Near-term work is entirely 2D. But a handful of decisions made early decide whether Spark can later bend into isometric, 2.5D, and full 3D without a rewrite. These are the doors to keep open.

The first is that **the camera is a View, not a global matrix.** From the moment a camera exists, it is an entity carrying a projection, a render target, and a subgraph to execute — not a hard-coded `view * projection`. This is what makes a minimap, a render-to-texture, or a later perspective camera a matter of spawning another entity rather than rewriting the renderer.

The second is that **vertex formats belong to pipelines, not to the renderer.** The sprite pipeline uses a 2D vertex layout; a future mesh pipeline will use a 3D one. They do not share a single global vertex type, which would waste memory in 2D and constrain 3D.

The third is that **a depth buffer exists from the moment depth sorting is introduced, even in pure 2D.** A real depth attachment plus a per-item depth value is the simplest correct way to layer a top-down scene, and it is the same mechanism a 3D renderer uses. A renderer with no depth buffer is stuck in painter's-algorithm-only and cannot generalize.

The fourth is that **materials are described by a typed binding layout, not by a closed enum.** A sprite's material and a future 3D material are the same kind of thing — a shader plus typed parameters — so the material system must be open to new material types from the start, never a fixed `enum` of known cases.

The fifth is that **coordinate handedness, the up-axis, and the depth range are chosen once and never re-read.** Spark uses +Y up, a right-handed basis, and a 0-to-1 depth range — `wgpu`'s natural mode — fixed on the first day. Changing this mid-project is the single most expensive thing in a 2D-to-3D transition.

The doors that slam shut, and must therefore be avoided, are: batching sprites into one mega-buffer that cannot survive depth sorting; sampling directly from the swapchain (which breaks the day a post-effect is added); hard-coding a single bind-group layout; and baking the projection into a shader at compile time.

## 7. Lessons that shaped this concept

This concept was not invented in a vacuum; it is a synthesis of what other renderers got right and wrong.

**Bevy** is the closest large-scale reference and the most instructive. Its staged Extract-Prepare-Queue-Render pipeline is sound and is adopted here. Its dual-`World` render sub-app is the part Spark deliberately does not copy, because the cost of keeping two worlds synchronized is real and the benefit (pipelined rendering) is one Spark does not need. Bevy's own community openly identifies the dual-world model as heavy for small scenes and its material API as boilerplate-prone — both warnings Spark heeds.

**Tiny Glade** (by Pounce Light) is the strongest real-world data point. The team started on Bevy for everything, then replaced Bevy's renderer with a custom one while keeping Bevy's ECS and app framework, because their rendering needs outgrew what Bevy's renderer was shaped to give. The lesson is direct and reassuring for Spark: the ECS and scheduler are the durable foundation, and the renderer is the part worth owning yourself. Spark's plan — a custom renderer on a custom ECS — is exactly that conclusion.

**rend3** is the cautionary tale. It was a clean `wgpu`-based renderer published as a general-purpose library, and it was archived when its single maintainer ran out of time and passion. The lesson is to build one renderer for one game, not a redistributable library — which is, again, Spark's stated philosophy.

**rafx** is the closest existing match to Spark's destination. Its layering — a low-level API wrapper, a framework with the render graph and extract jobs, and a renderer plugin layer — is essentially Spark's Gpu / Pass / Items+Scene split. Its framing of the render graph as a scheduler of view-and-phase jobs directly informed the "renderer as compiler" idea here.

**Our Machinery** formalized the notion that components can contribute nodes to the render graph — the per-component rendering hook. Spark borrows that idea but expresses it with ordinary Rust trait dispatch and ordinary ECS systems, not a runtime plugin system, whose integration cost was part of why Our Machinery struggled.

**sokol_gfx** sets the tone for the Gpu layer: explicit pipeline objects, frame-transient uniform data, no hidden ownership, and a thin wrapper rather than a deep abstraction.

The **frame-graph literature** (Frostbite's FrameGraph talk and the engines that followed) supplies the render-graph interface. The well-known critiques of render graphs — that they are boilerplate-heavy and unintuitive at small pass counts — are the reason Spark adopts the interface but hand-orders passes until the count justifies a true dependency graph.

## 8. Trade-offs, and what we deliberately defer

A few things are expensive to change later and must be gotten right early: the single-`World` design with renderer state held in Resources and Entities; the four-layer separation and its one-directional read rule; the camera-as-View model; the presence of a depth buffer once sorting exists; the fixed choice of handedness, up-axis, and depth range; and an atlas system shaped from the start as a layered texture array, since that is the same shape that scales to bindless later.

Many things are deliberately deferred under a strict YAGNI discipline. Automatic barrier inference is unnecessary because `wgpu` already tracks resource state. Transient-resource aliasing waits until there is real GPU memory pressure. Pipelined parallel rendering is not needed by a simulation-bound game. A second graphics backend is pointless because `wgpu` is already the abstraction. Shader hot-reload, a serialized scene format, physically based shading, and GPU-driven indirect rendering are all genuine future possibilities but are not on the critical path and are scheduled — if at all — only after the mature 2D pipeline exists.

The honest risks of this concept are worth naming. The render graph is mild overkill in the earliest milestones, mitigated by the fact that it starts as roughly sixty lines of hand-ordered list. The staged pipeline adds synchronization points, which are invisible at the scale of a 2D simulation but could be collapsed for a fast path if they ever were not. And the combination of a custom ECS, a custom renderer, and the Rust learning curve is a real source of slow progress — mitigated by the roadmap's rule that every milestone must end in a screenshot of the game using it.

## 9. Non-goals

The renderer concept explicitly does not aim to be a general-purpose or redistributable rendering library, a multi-backend hardware abstraction layer, a physically based 3D renderer in its first years, or a platform for runtime-loaded rendering plugins. It aims to be one well-layered renderer that grows, by extension only, from a single triangle to whatever picture Spark the game eventually needs.

## Status

This concept is accepted as the target. The game-facing API that realizes it is specified in [`RENDER_API.md`](./RENDER_API.md), and the ordered, additive plan to build it is in [`RENDER_ROADMAP.md`](./RENDER_ROADMAP.md). The exact ECS scheduling mechanism that drives the renderer's per-frame Workload is an ECS-layer concern and is settled in [`ECS_DESIGN.md`](./ECS_DESIGN.md) when the ECS is built.
