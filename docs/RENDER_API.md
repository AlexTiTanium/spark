# Spark Renderer — Public API (Target Design)

This document specifies the **game-facing API** of Spark's renderer: the surface that game code actually touches. The architectural reasoning behind it is in [`RENDER_CONCEPT.md`](./RENDER_CONCEPT.md); the ordered plan that builds it is in [`RENDER_ROADMAP.md`](./RENDER_ROADMAP.md).

This is a *target* design. Not all of it exists at the first milestone — the roadmap says precisely when each piece lands. But every piece here is the agreed destination, and the roadmap reaches it by extension only, never by rewriting what came before.

The renderer is built on `wgpu` 29.x. The game-facing API never exposes a `wgpu` type. The single place where shading language touches game code is a short WGSL fragment file, described in the materials section below.

## 1. The governing principle

The game never issues a draw call. It **describes the world as ordinary ECS data** — it spawns an entity carrying a `Sprite`, it mutates a `Transform2d`, it despawns the entity when the building is demolished — and the renderer observes that data each frame and produces the picture. There is no `draw_sprite()` function and there never will be.

This gives the game exactly two ways to put something on screen.

The **retained** mode is for content that lives across frames: the world. You spawn an entity with render components once, and forget it. The renderer keeps drawing it until you despawn it. Sprites, tilemaps, meshes, cameras, and lights are all retained.

The **immediate** mode is for content that is recomputed every frame: debug visuals, the power-flow overlay, a selection highlight that follows the cursor. This is the `Painter` — a single system parameter whose drawing lasts exactly one frame and is reissued the next. There are no entities to manage and no lifetimes to track.

Everything below is one or the other. If a thing persists, it is an entity. If a thing is recomputed each frame, it is a `Painter` call.

## 2. How the API maps onto the ECS — the separation of concerns

The renderer's game-facing surface is small, and each part has a definite ECS shape. Keeping these categories distinct is what makes the API legible.

| Category | What it is | Examples |
|---|---|---|
| **Components you spawn** | The "what to draw." Attached to an entity to make it visible. | `Sprite`, `AnimatedSprite`, `Tilemap`, `Mesh2d`-handle, `Mesh3d`-handle, `Camera2d`, `Camera3d`, `Light3d` |
| **Components that modify** | The "where / how / whether." Attached alongside the above. | `Transform2d`, `Transform3d`, `Visibility` / `Hidden`, `YSort`, `RenderTarget`, a material component |
| **System parameters** | The verbs of a frame. Asked for in a system's signature. | `Painter` (immediate drawing), `EguiCtx` (immediate UI) |
| **Resources** | One-of-a-kind state the game reads or configures. | `Assets` (load handles), `ClearColor`, `RenderSettings` |
| **Assets, behind handles** | Large, shared data. Loaded once, referenced by `Handle<T>`. | `Image`, `SpriteSheet`, `Tileset`, `Mesh2d`, `Mesh3d` |

The dividing line between an asset and a component is **size**. Large data that is almost always shared — a mesh of thousands of vertices, a multi-megabyte texture — is an asset, loaded once and referenced everywhere by a cheap `Handle<T>`. Small data that often varies per entity is a plain component, copied freely onto each entity. This rule is what makes a `Sprite` hold a `Handle<Image>` (the texture is large) while a custom material is itself a bare component (a material is a few floats and a handle or two). The rule is applied consistently throughout the API below, and the reasoning is spelled out where it matters.

## 3. Transforms

A renderable entity carries a transform that says where it is. There are two transform types, kept deliberately separate.

```rust
// 2D transform — for sprites, tilemaps, 2D meshes, 2D cameras.
// `z` is the depth / draw-order key; it is NOT a third spatial axis.
pub struct Transform2d {
    pub translation: Vec2,
    pub z: f32,            // larger z = closer to the viewer
    pub rotation: f32,     // radians
    pub scale: Vec2,
}

// 3D transform — for meshes, 3D cameras, lights. Arrives only at the 3D milestone.
pub struct Transform3d {
    pub translation: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}
```

They are separate types rather than one unified 3D transform because a 2D game should not carry a dead Z axis on every sprite, and because each type then says exactly what it means. `Transform3d` is a sibling added later, not a replacement — it slots in without touching any 2D code.

## 4. Sprites — the retained workhorse

A sprite is an entity. You spawn it once.

```rust
fn place_coal_plant(mut commands: Commands, assets: Res<Assets>) {
    // assets.load returns a handle immediately; the image streams in behind it.
    let img: Handle<Image> = assets.load("textures/coal_plant.png");

    commands.spawn((
        Sprite::from_image(img),              // what to draw
        Transform2d::from_xy(320.0, 180.0),   // where to draw it
    ));
}
```

The `Sprite` component holds only game-level concepts and not a single `wgpu` type:

```rust
pub struct Sprite {
    pub image: Handle<Image>,
    pub tint: Color,                 // multiplied into the pixels; WHITE = unchanged
    pub anchor: Anchor,              // Center, BottomCenter, ...
    pub flip_x: bool,
    pub flip_y: bool,
    pub custom_size: Option<Vec2>,   // None = the image's natural size
}
```

Constructors keep struct literals short: `Sprite::from_image(h)`, and a builder style such as `.with_tint(...)` or `.anchored(Anchor::BottomCenter)` for the rest. A sprite drawn from one frame of a sprite sheet uses `Sprite::from_sheet(sheet_handle, frame_index)`. A solid colored quad — a sprite with no texture — uses `Sprite::solid(color, size)`; this is the colored-quad primitive, and treating it as a degenerate sprite means there is one component to learn instead of two.

Moving a sprite is mutating its `Transform2d` in an ordinary system; the renderer follows. Removing it is `commands.entity(e).despawn()`. Hiding it without destroying it is `commands.entity(e).insert(Hidden)`.

Animation is the proof that the design grows by extension. A sprite sheet loads as a `Handle<SpriteSheet>`, and `AnimatedSprite` is simply another component added alongside `Sprite`, touching nothing in the existing sprite path:

```rust
commands.spawn((
    Sprite::from_sheet(worker_sheet, 0),
    AnimatedSprite::looping(&[0, 1, 2, 3], 8.0),   // frames, frames-per-second
    Transform2d::from_xy(x, y),
));
```

### Batching is invisible

There is no batch API, and that absence is a feature. Grouping sprites by atlas, building instance buffers, and collapsing them into a few draw calls all happen inside the renderer's Items and Pass layers. Drawing ten thousand sprites is simply spawning ten thousand `Sprite` entities:

```rust
fn scatter_trees(mut commands: Commands, assets: Res<Assets>) {
    let tree = assets.load("textures/tree.png");
    for pos in forest_positions() {
        commands.spawn((Sprite::from_image(tree), Transform2d::from_xy(pos.x, pos.y)));
    }
}
```

The word "batch" never appears in game code. The renderer's job is to make that fast.

## 5. Cameras

A camera is an entity with components, not a global matrix and not a singleton resource. This is the architectural decision that keeps render-to-texture and multiple viewports cheap.

```rust
commands.spawn((
    Camera2d {
        clear_color: Some(Color::hex("#0d0d14")),
        ..default()
    },
    Transform2d::from_xy(0.0, 0.0),
));
```

Panning and zooming are mutations of the camera's `Transform2d` in an ordinary system — there is no separate camera API. Because a camera is an entity, a minimap is just a second camera that draws into a texture instead of the screen:

```rust
commands.spawn((
    Camera2d { order: 1, ..default() },   // `order` decides which camera draws on top
    Transform2d::default(),
    RenderTarget::image(minimap_texture), // draw into a texture, not the swapchain
));
```

A 3D camera is `Camera3d` with a `Transform3d` — the same pattern, added at the 3D milestone.

## 6. Tilemaps

A tilemap is not a million tile entities — that would crush both the ECS and the developer. It is a single entity whose component holds the grid of tile IDs.

```rust
let map = commands.spawn((
    Tilemap::new(tileset, UVec2::new(64, 64), /* tile size */ 16.0),
    Transform2d::default(),
)).id();
```

Painting the map is mutating that component through an ordinary `Query<&mut Tilemap>`; the renderer detects the change and re-uploads only the touched chunks.

```rust
fn carve_river(mut maps: Query<&mut Tilemap>) {
    let mut map = maps.single_mut();
    map.fill(IRect::new(0, 0, 64, 4), TileId::WATER);   // fill a band
    map.set(IVec2::new(10, 12), TileId::COAL_DEPOSIT);  // set one cell
}
```

The game thinks in "cell (x, y) holds tile T." Chunking and GPU re-upload are invisible.

## 7. Draw order

Coarse draw order is the `z` field of `Transform2d`: a larger `z` draws closer to the viewer. For a top-down game the more important tool is Y-sorting — a worker standing higher on screen should be hidden behind a building in front of it. That is a single marker component:

```rust
commands.spawn((
    Sprite::from_image(worker),
    Transform2d::from_xy(x, y),
    YSort,   // depth is now derived automatically from world Y
));
```

`Visibility` and the `Hidden` marker control whether an entity draws at all, without despawning it.

## 8. The Painter — immediate-mode drawing

The `Painter` is a system parameter for everything recomputed each frame: debug primitives, the power-flow overlay, a hovering cursor highlight. Whatever you draw with it lives one frame and is reissued the next.

It draws in one of three coordinate spaces.

```rust
fn draw_stuff(mut painter: Painter, q: Query<&Transform2d, With<Building>>) {
    // DEFAULT — screen coordinates, in pixels from the window corner.
    // Camera-independent; ideal for HUD-level debugging.
    painter.rect(Rect::from_min_size(vec2(8.0, 8.0), vec2(20.0, 20.0)), Color::RED);

    // .world() — world coordinates; the drawing moves with the camera.
    painter.world().line(vec2(0.0, 0.0), vec2(100.0, 50.0), Color::CYAN);

    // .relative_to(transform) — (0,0) is that transform's origin.
    // Draw "around an entity" without knowing its absolute position.
    for tf in &q {
        painter.relative_to(tf).circle(Vec2::ZERO, 12.0, Color::YELLOW);
    }
}
```

`.world()` and `.relative_to(&tf)` return a `Painter` scoped to that coordinate space; the rest of the API — `line`, `triangle`, `rect`, `circle`, `polyline`, `arrow`, `text` — is the same in all three. A custom-shaded immediate primitive is possible via `painter.with_material(handle)`, described in the materials section.

## 9. UI

UI is not built from components. `egui` is immediate-mode, so the UI API is to write ordinary `egui` code inside a system that asks for an `EguiCtx` parameter.

```rust
fn hud(mut ui: EguiCtx, economy: Res<Economy>, grid: Res<PowerGrid>) {
    egui::TopBottomPanel::top("hud").show(ui.ctx(), |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("Capital: {}", economy.capital));
            ui.label(format!("Power: {:.0} / {:.0} MW", grid.supply, grid.demand));
        });
    });
}
```

There are no UI components and no bridging layer. The renderer runs the `egui` pass at the end of the frame; the game just draws UI in ordinary systems. This matches the decision recorded in [`UI_DESIGN.md`](./UI_DESIGN.md).

## 10. Materials and shaders

A **material** is a WGSL fragment shader together with its typed parameters. Everything else — the vertex shader, the camera binding, the sprite's own texture, the `VertexOutput` struct — the engine supplies for free. A custom shader is therefore usually about ten lines: you write only the fragment function.

Ordinary sprites, colored quads, and tilemaps use a built-in material, and the game never thinks about materials at all. A custom material is defined only when custom WGSL is wanted.

### A material is a plain component

A custom material is a Rust struct that derives `Material2d`. **That derive also makes the struct a `Component`**, and the struct is attached directly to an entity. There is no asset registry step, no handle, and no wrapper component — you just put the material struct on the entity, exactly as you put any other component there.

The two halves of a material live in two places. The WGSL fragment shader lives under `assets/shaders/`, because it is an asset and can be hot-reloaded. The Rust struct lives next to game code. The `#[fragment("...")]` attribute, with a path relative to the assets root, is what links them.

```
spark-game/
  assets/shaders/
    water.wgsl          <- WGSL: the fragment shader for WaterMaterial
  src/game/materials/
    water.rs            <- Rust: struct WaterMaterial + #[derive(Material2d)]
```

The Rust side:

```rust
// src/game/materials/water.rs
#[derive(Material2d)]                  // this derive also implies Component
#[fragment("shaders/water.wgsl")]      // path is relative to the assets root
pub struct WaterMaterial {
    // All #[uniform] fields are packed by the engine into ONE uniform buffer
    // at @group(1) @binding(0), in declaration order.
    #[uniform] pub tint: Color,
    // Each #[texture] field takes the next TWO bindings: the texture and its
    // sampler. Here, ripples becomes @binding(1) and @binding(2).
    #[texture] pub ripples: Handle<Image>,
}
```

The binding rule is one sentence: `#[uniform]` fields collapse into a single buffer at binding 0, and each `#[texture]` field takes the next two bindings. The WGSL must declare matching bindings; group 0 is the engine's view and globals, group 1 is the material.

The paired WGSL is short, because the engine already provides the vertex shader and the view:

```wgsl
// assets/shaders/water.wgsl

// The engine prelude: VertexOutput (what the vertex shader produced) and
// sample_sprite (access to the sprite's own texture). The engine writes these.
#import spark::sprite::{ VertexOutput, sample_sprite }
#import spark::globals::globals          // engine-wide values, e.g. globals.time

// Group 1 is the material. Order and types MUST match WaterMaterial's fields.
struct Material {
    tint: vec4<f32>,
}
@group(1) @binding(0) var<uniform> m: Material;
@group(1) @binding(1) var ripples: texture_2d<f32>;
@group(1) @binding(2) var ripples_smp: sampler;

// The game writes only this function.
@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let wobble = textureSample(ripples, ripples_smp, in.uv + globals.time * 0.05).rg;
    let base = sample_sprite(in.uv + (wobble - 0.5) * 0.04);
    return base * m.tint;
}
```

Genuinely global values, such as time, come from the engine's `@group(0)` globals and are *not* material fields. A material holds only what genuinely varies per material. The vertex shader can also be overridden with a `#[vertex("...")]` attribute, but in 2D that is rarely needed.

### Using a material — three steps, not four

```rust
// 1. Register the material TYPE — once, when the game plugin is built.
//    This tells the renderer about the shader so it can build the pipeline.
impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_material::<WaterMaterial>();
    }
}

// 2 + 3. Create and attach the material in ONE step: it is just a component.
fn spawn_pond(mut commands: Commands, assets: Res<Assets>) {
    commands.spawn((
        Sprite::from_image(assets.load("textures/pond.png")),
        WaterMaterial {                       // the material IS the component
            tint: Color::WHITE,
            ripples: assets.load("textures/ripples.png"),
        },
        Transform2d::from_xy(0.0, 0.0),
    ));
}
```

There is no separate "create the material asset, get a handle, attach the handle" dance — the material struct goes straight onto the entity. Updating a material is an ordinary query, exactly like mutating any other component:

```rust
fn pulse_selected(time: Res<Time>, mut q: Query<&mut GlowMaterial, With<Selected>>) {
    for mut glow in &mut q {
        glow.intensity = 1.0 + (time.elapsed_secs() * 4.0).sin() * 0.5;
    }
}
```

Because a material is a component, the renderer deduplicates identical materials internally by hashing their contents, so two ponds with the same `WaterMaterial` still share one GPU bind group. At Spark's scale — a handful of distinct material types — this hashing is negligible. The one situation it would not be is tens of thousands of entities with a custom material; if profiling ever shows that, an explicit shared form (`Handle<M>` accepted as a material) can be added later as an opt-in escape hatch, without changing the common-case API above.

### The four shape-and-shader cases

A plain colored quad needs no material at all — the built-in material handles it:

```rust
commands.spawn((
    Sprite::solid(Color::hex("#ff5500"), Vec2::splat(48.0)),
    Transform2d::from_xy(100.0, 100.0),
));
```

A colored quad with a custom shader is the same `Sprite::solid` plus a material component:

```rust
let_ = (); // GlowMaterial { color, intensity } defined like WaterMaterial above
commands.spawn((
    Sprite::solid(Color::WHITE, Vec2::splat(48.0)),     // the shape: a white quad
    GlowMaterial { color: Color::CYAN, intensity: 1.5 },// the look: a glow shader
    Transform2d::from_xy(200.0, 100.0),
));
```

A textured sprite with a custom shader is the same, with the sprite carrying an image; the shader reaches the sprite's own texture through `sample_sprite` in the prelude, while the material's own `#[texture]` fields are additional textures:

```rust
commands.spawn((
    Sprite::from_image(pond_tex),
    WaterMaterial { tint: Color::WHITE, ripples: ripples_tex },
    Transform2d::default(),
));
```

A custom-shaded triangle has two forms. A persistent one is a retained mesh — note that the mesh is an asset (large, shared) referenced by a handle, while the material stays a bare component (small):

```rust
let tri = meshes.add(Mesh2d::triangle(a, b, c));   // mesh: asset -> handle
commands.spawn((
    tri,                                            // the mesh handle is a component
    GlowMaterial { color: Color::CYAN, intensity: 1.5 },
    Transform2d::default(),
));
```

A per-frame dynamic one uses the `Painter`: `painter.with_material(glow).triangle(a, b, c, Color::WHITE)`. One material type — `GlowMaterial` here — works unchanged on a quad, a sprite, a retained mesh, and a `Painter` primitive, because a material is not bound to what it is drawn on.

## 11. One material per entity — and effects as a separate thing

The GPU draws a shape with exactly one pipeline, which means exactly one fragment shader, which means **exactly one material per entity.** This is not a Spark choice; it is how the hardware works. A renderable entity therefore carries at most one material component.

Layered visual effects — a selection outline and a highlight glow on the same building — are not "more materials." A material is the *surface* of an object; an outline is an *effect layered around* it. Effects are expressed as ordinary marker components, consumed not by the main draw but by their own render-graph passes:

```rust
// An effect is a plain marker component, defined game-side when first needed.
#[derive(Component)] pub struct Outline   { pub color: Color, pub width: f32 }
#[derive(Component)] pub struct Highlight { pub color: Color, pub intensity: f32 }

fn on_select(mut commands: Commands, just_picked: Query<Entity, Added<Selected>>) {
    for e in &just_picked {
        // Attach BOTH effects. They do not conflict, because they are not
        // materials competing for one draw — each is its own render-graph pass.
        commands.entity(e).insert((
            Outline   { color: Color::WHITE, width: 2.0 },
            Highlight { color: Color::CYAN,  intensity: 0.4 },
        ));
    }
}
```

Effects compose freely with each other and with the entity's own material, because each effect is a separate pass: the surface material runs in the main pass, the outline in an outline pass, the highlight in a highlight pass. The resulting taxonomy is clean — **an entity has zero or one material component, and any number of effect-marker components.** The engine provides the *mechanism* (a marker component drives a render-graph pass); specific effects such as `Outline` are defined in the game, when the game needs them.

## 12. Meshes and 3D

3D is not a new API. It is the same API with `3d` in the type name. Spawning a 3D mesh is structurally identical to spawning a sprite:

```rust
// 2D sprite
commands.spawn((Sprite::from_image(coal_plant), Transform2d::from_xy(320.0, 180.0)));

// 3D mesh — the same shape
commands.spawn((
    Mesh3d::from(cube_handle),
    Material3d::from(steel_handle),
    Transform3d::from_xyz(3.0, 0.0, 1.0),
));
```

Cameras and lights are likewise just entities:

```rust
commands.spawn((Camera3d::default(), Transform3d::looking_at(Vec3::ZERO)));
commands.spawn((Light3d::directional(Color::WHITE), Transform3d::default()));
```

`Mesh2d` covers custom 2D geometry — a filled service-area polygon, a power-line ribbon — for cases a quad cannot express. The pattern learned once with `Sprite` is the pattern for everything: an entity, a render component, a transform, observed by a camera.

### Meshes are assets — and why that is not the material situation

A mesh is reached through a `Handle`, never spawned as a bare component the way a material is. This looks like an inconsistency next to the material rule of section 10, but it is the asset-versus-component rule of section 2 — decided by size — applied correctly. A material is a few dozen bytes, so it is copied freely onto each entity as a component. A mesh is geometry: a triangle is around a hundred bytes, but a service-area polygon is kilobytes and a detailed ribbon is tens of kilobytes. Vertex data that large belongs in the asset arena, referenced by a cheap handle, not copied into every entity's component storage. A mesh is the same *kind* of thing as an `Image` — large, GPU-resident, lifetime-managed — so `Handle<Mesh2d>` is consistent with `Handle<Image>`, not an exception to it.

A mesh reaches an entity by one of two routes, and which one depends on where the geometry comes from. An **authored** mesh — a file on disk — loads like any other asset, in a single line:

```rust
// Authored mesh: a file. Loads like any asset; no separate registration step.
let gear: Handle<Mesh2d> = assets.load("meshes/gear.mesh");
```

A **procedural** mesh — geometry built in code at runtime, which is the common case for a 2D game whose shapes come from gameplay — is constructed and then registered, which yields its handle:

```rust
// Procedural mesh: built in code, then registered. The handle that comes back
// is the identity of a GPU vertex buffer — it is load-bearing, not ceremony.
let area: Handle<Mesh2d> = meshes.add(Mesh2d::polygon(&service_area_points));
```

The `meshes.add(...)` step is not the avoidable ceremony that the old material-as-asset design carried. For a procedural mesh, registration is the genuine act of handing freshly built vertex data to the GPU and receiving a stable identity for it. A throwaway shape that changes every frame should not be a retained mesh at all — that is what the immediate-mode `Painter` is for.

## 13. The whole game-facing API on one page

The entire surface that game code imports from the renderer:

```rust
use spark::prelude::*;

// --- Spawned into the world (retained) ---------------------------
//   render components — "what to draw"
Sprite          AnimatedSprite      Tilemap
Mesh2d-handle   Mesh3d-handle
Camera2d        Camera3d            Light3d
//   modifier components — "where / how / whether"
Transform2d     Transform3d
YSort           Hidden / Visibility RenderTarget
//   material components — "the surface look" (zero or one per entity)
//   plus game-defined effect markers (Outline, Highlight, ...) — any number
//   assets, behind handles
Handle<Image>   Handle<SpriteSheet> Handle<Tileset>
Handle<Mesh2d>  Handle<Mesh3d>

// --- System parameters (the verbs of a frame) --------------------
Painter         // immediate-mode drawing for this frame
EguiCtx         // immediate-mode UI for this frame
Res<Assets>     // load asset handles

// --- Config resources (set once, forget) -------------------------
ClearColor      RenderSettings       // vsync, msaa
```

That is the whole thing: roughly a dozen components, two immediate-mode system parameters, asset loading, and two config resources. No `wgpu` type, no pipeline, no pass, no encoder, no batch. A picture of any complexity is assembled by combining these primitives; the renderer's Items, Pass, and Gpu layers turn the combination into a frame.

## 14. Deliberately deferred

A few API affordances are intentionally not in the target above, and would be added only if a concrete need appears. An explicit shared-material form via `Handle<M>` is deferred until profiling shows the per-component material hashing is a real cost. Pipeline specialization — multiple shader variants of one material — is deferred until a material genuinely needs four or more variants. A serialized scene format is deferred until there is a level editor. None of these would change the API in sections 3 through 13; each is a strictly additive extension, in keeping with the concept's promise to grow by extension only.
