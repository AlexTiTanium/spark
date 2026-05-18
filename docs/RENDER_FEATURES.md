# Spark Renderer Feature Catalog

**Audience:** the solo developer of Spark — a 2D top-down tile-based power-grid + city-builder, written in Rust 2024, on wgpu 29.x with WGSL, with a frozen 4-layer renderer (Gpu / Pass / Items / Scene), a single-World ECS (Extract → Prepare → Queue → Sort → Record → Present), and a frozen game-facing API (RETAINED components + IMMEDIATE `Painter`, materials as plain components, meshes/textures as `Handle<T>` assets, effects as marker components).

## TL;DR

- **Build the 2D-shipping core (R0–R10) and stop:** batched sprites + array textures, tilemap, Camera2d/Painter, depth+Y-sort, custom Material2d, MSDF text, a small post-processing chain (HDR→tonemap, bloom, vignette, optional CRT), and one strong 2D light pass. Everything else in 2D is gravy.
- **For "ambitious 2D" pick exactly one lighting story** — pragmatic (Light2d + normal-mapped sprites + screen-space soft shadows, à la `bevy_light_2d`) or research-tier (a Radiance Cascades pass in WGSL, the same algorithm Alexander Sannikov shipped in Path of Exile 2). Don't build both.
- **Most "modern renderer" features are either wrong genre, wrong API, or wrong scale for Spark.** WebGPU/wgpu 29 still has no bindless, no mesh shaders in core (only experimental native), and only experimental inline ray queries — so Nanite-class virtual geometry, ReSTIR-style hardware RT GI (à la Bevy Solari), and Unreal-style GPU-driven rendering are explicitly out of scope. A 2D city-builder also does not need them: at ≤ a few thousand draw items per frame, *the CPU is the bottleneck only if you make it one*, and a competent batched sprite renderer + a tilemap drawn as one fullscreen-quad-per-chunk will not stress a modern GPU at any sane resolution.

---

## How to read this catalog

Each entry has: **What it is**, **Bucket + verdict**, **Layers/components/passes it touches**, **Code sketch** in Spark's API style. Buckets:

1. **MUST BUILD** — on the R0–R12 roadmap, needed to ship.
2. **SHOULD/CAN BUILD** — feasible, valuable extensions.
3. **FAR FUTURE** — possible on this architecture but large effort, only if Spark turns 3D.
4. **NEVER** — out of scope; entry explains the technology and the honest reason it's out.
5. **OPTIMIZATIONS** — separate section with an honest "needed vs premature" verdict for each.

A naming convention used below: `commands.spawn((...))`, `#[derive(Material2d)] #[fragment("...")]`, `Painter`, `Transform2d`/`Transform3d`, `Handle<Mesh>` / `Handle<Texture>`, marker components for effects, `Res<T>` / `ResMut<T>` / `Query<...>` for systems.

---

# 1. MUST BUILD

These are R0–R10 essentials. Without them Spark cannot render the game.

## 1.1 Foundations (R0–R2)

### Hardcoded triangle / clear (R0)
**What:** the smoke test: open a window, configure the surface, draw one colored triangle.
**Bucket:** MUST. Validates Gpu layer + surface acquire + render pass record.
**Layers:** Gpu, Pass (a single named pass `"main"` with one WGSL).
**Why straightforward:** no Scene-layer types yet; the `Painter` and Sprite paths don't exist.

### Textured quad (R1)
**What:** load an image, upload to a `wgpu::Texture`, sample in a fragment shader, draw a quad.
**Bucket:** MUST.
**Layers:** Gpu (texture creation, sampler, bind group), Pass (textured quad pipeline + WGSL).
**Why straightforward:** the bind group layout for `(texture, sampler, uniforms)` becomes the template for every Material2d.

### Sprite component + staged pipeline (R2)
**What:** the canonical 6-stage Workload (Extract→Prepare→Queue→Sort→Record→Present) lit up for the first time with one component type.
**Bucket:** MUST.
**Layers:** Scene (`Sprite`, `Transform2d`), Items (`ExtractedSprite`, `SortKey`), Pass (sprite pipeline).
**Code:**
```rust
#[derive(Component)]
pub struct Sprite {
    pub texture: Handle<Texture>,
    pub rect: Option<Rect>,    // sub-region of the texture (atlas)
    pub anchor: Vec2,          // pivot in [0,1]
    pub color: Color,          // tint
}

commands.spawn((
    Sprite { texture: assets.load("tiles/grass.png"), ..default() },
    Transform2d::from_xy(64.0, 32.0),
));
```
Extract walks `Query<(&Sprite, &Transform2d, &GlobalVisibility)>`, pushes into the Items working set, Queue assigns to the opaque or transparent phase, Sort produces a stable order, Record emits draws.

## 1.2 Batched 2D rendering (R3–R4)

### Batched sprites + atlas (R3)
**What:** instead of one draw call per sprite, pack many sprites' per-instance data (transform, UV rect, color) into a single instanced draw against a unit quad. Group consecutive sprites that share the same texture/atlas/pipeline into one draw.
**Bucket:** MUST. This is the line between "tech demo" and "city builder".
**Layers:** Items (batcher), Pass (instanced sprite pipeline with a per-instance buffer).
**Why straightforward:** sprites are state-light; the only batch break is texture/pipeline/scissor. The `emmyleaf/kelp-2d` crate (a "2D wgpu-based sprite renderer") demonstrates the "all atlases on a texture array, one draw call per target" pattern: *"Textures are allocated on an array of atlases, which avoids rebinding resources to the graphics pipeline. This means that all drawing to a given target can be done in a single draw call."* Spark can adopt the same shape.
**Note on bindless:** the natural "bindless" version (one big descriptor array indexed per-instance) is **not possible in WebGPU/wgpu 29 core** — there is no stable bindless. wgpu has partial native-only `binding_array` support behind feature flags with severe limitations (per the wgpu Bindless Tracking Issue #3637 and gpuweb/proposals/bindless.md: "No feature combination allows Metal to use STORAGE_RESOURCE_BINDING_ARRAY", uniform-buffer arrays unsupported on most backends, etc.). The portable answer is a **texture array** (`texture_2d_array`) of fixed-size atlas pages. Spark should plan around texture-array atlases, not bindless.

### Texture atlases + array textures (R3 cont.)
**What:** pack many sprites into one large texture (or one slice of a `texture_2d_array`) so consecutive draws don't break batching. GPUs batch only within the same bound texture; an atlas is the prerequisite that *enables* the batching above.
**Bucket:** MUST.
**Layers:** Scene (an `AtlasBuilder` resource), Gpu (the texture allocation), Items (UV-rect lookup at extract time).
**Implementation:** a `guillotière`-style dynamic packer (used by `kelp-2d`) + an `Assets<Atlas>` resource. For tiles: each tile = one slot in a `2048×2048` page; for the array-texture variant, each unique tilesheet/sprite-sheet is one layer.

### Camera2d-as-View + Painter (R4)
**What:** Camera2d is just an entity with `Transform2d` + `Camera2d { zoom, viewport, target }`. It becomes a **View** in the render-graph sense (target, view-projection, clear color). The `Painter` is a system-param for the IMMEDIATE mode — it pushes the same draw items the RETAINED path produces, but per-frame from a system.
**Bucket:** MUST.
**Layers:** Scene (`Camera2d`, `Painter`), Items (per-View buckets).
**Code:**
```rust
commands.spawn((Camera2d::default(), Transform2d::IDENTITY));

fn debug_overlay(mut painter: Painter, q: Query<&Building>) {
    for b in &q {
        painter.world().rect(b.aabb, Color::CYAN.with_alpha(0.3));
    }
    painter.screen().text((10.0, 10.0), "FPS: 60", &FONT);
}
```
The `world() / screen() / relative_to(transform)` API maps each call to a different transform stack at Extract time.

## 1.3 Depth, sort, tilemap (R5–R6)

### Depth buffer + Y-sort (R5)
**What:** a depth attachment lets near-opaque objects early-Z reject overdraw; *for transparent sprites* you still need painter's-algorithm sorting back-to-front. In 2D, "Y-sort" means sort transparent sprites by `world.y + z_offset` so a character behind a fence renders before the fence. The reasoning is articulated in the classic LearnOpenGL Blending chapter: *"draw all of your opaque objects first … then sort all the transparent objects based on distance from the viewer and finally draw all the transparent objects in sorted order"* — and per the Castle Game Engine docs, transparent passes should be *"rendered with Z-testing but without Z-writing"*.
**Bucket:** MUST. The city-builder absolutely needs this (buildings, characters, smoke).
**Layers:** Items (the Sort stage), Pass (the opaque pass uses depth read+write; the transparent pass uses depth read only).
**ASCII:**
```
   pass: opaque        pass: transparent (sorted back→front)
   depth W=on R=on     depth W=off R=on
   ───────────────►    ───────────────►
   tiles, building     smoke, glass, fog
   bases (opaque)      windows, ui-world
```

### Tilemap rendering (R6)
**What:** render a big grid of tiles cheaply. The right trick for a city-builder is **chunked GPU tilemaps**: the world is divided into 32×32 (or 64×64) chunks; each chunk has a tile-index buffer; the GPU rasterizes one quad per visible chunk and samples the atlas via the index buffer in the fragment shader. Paavo Huhtala's "How I render large 3D tilemaps with a single draw call at 3000 FPS" (blog.paavo.me) demonstrates the limit case: *"about 200 to 300 microseconds on average to produce each frame when rendering a 512×512×512 world at 3440×1440 screen resolution, using an RTX 2080 Super … 3000 to 5000 FPS"*. Spark needs only the 2D version of this idea.
**Bucket:** MUST.
**Layers:** Scene (`Tilemap`, `TilemapChunk` components), Pass (a `tilemap` pipeline), Items (one draw per visible chunk).
**Why this is the right answer for Spark:** at 60 fps with a 1000×1000 world, a per-tile draw is dead on arrival; one quad per chunk + a fragment-shader lookup into a tile-ID texture turns the workload into "rasterize a few hundred quads", which is *trivial* for any GPU and stays trivial as the world grows.
**Code:**
```rust
#[derive(Component)]
pub struct Tilemap {
    pub atlas: Handle<Texture>,    // 16x16 grid of 32x32 tiles → 512x512 atlas
    pub tile_size: UVec2,          // (32, 32)
    pub chunk_size: UVec2,         // (32, 32) tiles per chunk
}

#[derive(Component)]
pub struct TilemapChunk {
    pub chunk_coord: IVec2,
    pub tile_ids: Handle<TileBuffer>, // GPU buffer of u16 tile IDs
}
```

## 1.4 Materials, UI, basic effects (R7–R10)

### Custom Material2d (R7)
**What:** the user-extensible shading hook. A struct deriving `Material2d`, attached as a component, fragments declared in WGSL with a single declarative attribute; the renderer hashes the data for dedup but the material is *not* an asset.
**Bucket:** MUST.
**Layers:** Scene (any `T: Material2d` is a Component), Pass (per-material pipeline cache, keyed by `TypeId<T>`).
**Code (frozen API style):**
```rust
#[derive(Material2d)]
#[fragment("shaders/water.wgsl")]
pub struct WaterMaterial {
    pub tint: Color,
    pub flow: Vec2,
    pub noise: Handle<Texture>,
}

commands.spawn((
    Mesh2d(quad_mesh.clone()),
    WaterMaterial { tint: AQUA, flow: vec2(0.1, 0.0), noise: noise_tex },
    Transform2d::from_xy(0.0, 0.0),
));
```
WGSL side:
```wgsl
// shaders/water.wgsl
@group(2) @binding(0) var<uniform> mat: WaterMaterial;
@group(2) @binding(1) var noise_tex: texture_2d<f32>;
@group(2) @binding(2) var noise_smp: sampler;

@fragment fn fs(in: VertexOut) -> @location(0) vec4<f32> {
    let uv = in.uv + mat.flow * globals.time;
    let n = textureSample(noise_tex, noise_smp, uv).r;
    return vec4(mat.tint.rgb * (0.8 + 0.2 * n), 1.0);
}
```
The `#[derive(Material2d)]` macro generates AsBindGroup glue, hashing for dedup, and a `SpecializedPipeline` key including the WGSL path.

### egui UI overlay (R8)
**What:** an immediate-mode debug/dev UI overlay drawn after the world.
**Bucket:** MUST (for dev) — you will live in this UI.
**Layers:** Pass (`ui` pass, depth disabled, runs last), Items (egui translates to its own draws).
**Implementation:** `egui-wgpu` integration — it asks for a queue/device/format, you give it the final color target and a textured-quad pipeline. Zero wgpu types leak into Scene.

### Visualization overlays + effect passes (R9)
**What:** the architectural pattern "one material per entity, any number of effect markers". An effect (outline, highlight, scanline-mark) is a **marker component** that a dedicated render-graph pass picks up.
**Bucket:** MUST. Power-grid visualization is *the* core gameplay overlay (wire connectivity, voltage, range, demand) — this is a hard requirement.
**Layers:** Scene (markers), Pass (one effect pass per marker type).
**Code:**
```rust
#[derive(Component)] pub struct Outline { pub color: Color, pub thickness_px: f32 }
#[derive(Component)] pub struct PowerRange { pub radius: f32, pub color: Color }
#[derive(Component)] pub struct WireHighlight;

// One building with two effects at once — totally fine:
commands.spawn((
    Sprite { texture: substation_tex, ..default() },
    Transform2d::from_xy(x, y),
    Outline { color: YELLOW, thickness_px: 2.0 },
    PowerRange { radius: 256.0, color: YELLOW.with_alpha(0.15) },
));
```
The `outline` pass reads the alpha of the sprite render target, samples 8 neighbors in screen space, and writes a ring; the `power_range` pass draws a screen-space disc per tagged entity. Crucially, *the material on the sprite does not change* — effects compose by stacking passes, not by exploding the shader permutation space.

### Post-processing pass (R10)
**What:** a thin chain operating on the final color buffer: HDR (render into Rgba16Float) → bloom (downsample/blur/upsample threshold pass) → tonemap (ACES / AgX) → color grading (LUT lookup) → vignette → optional CRT/scanline filter → final present.
**Bucket:** MUST (at least HDR + tonemap + bloom). Vignette/CRT optional.
**Layers:** Pass (each step is a fullscreen-triangle pass), Items (none — purely Pass-level).
**Code (config Resource pattern):**
```rust
#[derive(Resource, Default)]
pub struct PostProcessing {
    pub bloom: Option<Bloom>,                 // { intensity, threshold }
    pub tonemap: Tonemap,                     // Aces | AgX | Reinhard | None
    pub color_grade: Option<Handle<Texture>>, // a 32x32x32 LUT in a 2D strip
    pub vignette: Option<Vignette>,
    pub crt: Option<Crt>,
}
```
**Note on HDR-as-prerequisite:** bloom/tonemap only make sense if the color buffer is `Rgba16Float`, and emissive sprite materials write values > 1.0. Pick this up at R10, not before.

---

# 2. SHOULD / CAN BUILD

Feasible, valuable, realistic extensions. Several are on the R0–R12 roadmap (R11 isometric, R12 3D) but the renderer can choose to take them gradually.

## 2.1 Text and UI

### SDF / MSDF text rendering
**What:** instead of bitmap fonts, store each glyph as a (multi-channel) signed distance field in an atlas. The fragment shader recovers crisp edges at any zoom by thresholding the median of three channels (MSDF preserves corners that single-channel SDF rounds). The canonical sources are Viktor Chlumský's `msdfgen` and `msdf-atlas-gen` tools; the canonical shader (from the msdfgen README) is:
```glsl
float median(float r, float g, float b) { return max(min(r, g), min(max(r, g), b)); }
// ...
vec3 msd = texture(msdf, texCoord).rgb;
float sd  = median(msd.r, msd.g, msd.b);
float screenPxDistance = screenPxRange() * (sd - 0.5);
float opacity = clamp(screenPxDistance + 0.5, 0.0, 1.0);
```
**Bucket:** SHOULD. A city-builder zooms aggressively (city overview ↔ single-building inspector); bitmap fonts blur, SDF/MSDF stays sharp.
**Layers:** Scene (`Text2d` component or `Painter::text`), Pass (`text` pipeline with the median-of-3 trick), Items (one quad per glyph, batched).
**Code:**
```rust
let font: Handle<MsdfFont> = assets.load("fonts/inter.msdf.json");

commands.spawn((
    Text2d {
        content: "Substation #4".into(),
        font: font.clone(),
        size_px: 14.0,
        color: WHITE,
    },
    Transform2d::from_xy(x, y),
));
```
**Don't write a font shaper.** Use `cosmic-text` for shaping/layout and `fontdue` or `swash` only if you need glyph outlines you don't have; feed the resulting glyph IDs through your MSDF atlas.

### 9-slice sprites
**What:** stretch only the middle strip of a sprite, keep corners and edges undistorted. Trivial uniform tweak.
**Bucket:** SHOULD. UI panels, tooltips, building selection rings — all want it.
**Layers:** Scene (`NineSlice` variant of Sprite), Pass (a `nineslice` pipeline or a uniform branch in the sprite shader).

## 2.2 2D lighting (the big one for this genre)

Spark is a power-grid game with a day/night cycle and lit windows. *Some* form of 2D lighting is a strong fit. Pick **one** of the three approaches; do not build all three.

### Option A — Pragmatic forward 2D lighting + normal-mapped sprites
**What:** a forward pipeline that knows about a flat set of point/spot/ambient lights, and a per-sprite *optional* normal map. The lighting WGSL evaluates each light per-pixel with cheap N·L + falloff. This is what `jgayfer/bevy_light_2d` does: *"General purpose 2D lighting for the bevy game engine. Designed to be simple to use, yet expressive enough to fit a variety of needs."* with features *"Configurable point lights · Light occlusion · Dynamic shadows · Camera specific ambient light · Single camera rendering · Web support for WebGL2 and WebGPU"*.
**Bucket:** SHOULD (this is the recommended default for Spark).
**Layers:** Scene (`PointLight2d`, `AmbientLight2d`, optional `NormalMap` attached to sprite materials), Pass (a `lighting` pass that consumes the diffuse + normal G-buffer-lite produced by R2's sprite pipeline).
**Code:**
```rust
commands.spawn((
    PointLight2d { color: WARM_WHITE, intensity: 4.0, radius: 96.0, falloff: 2.0 },
    Transform2d::from_xy(window_x, window_y),
));

#[derive(Component, Default)]
pub struct AmbientLight2d { pub color: Color, pub intensity: f32 } // resource-or-component
```
Per-sprite normal map is *optional* — for tile graphics you'll mostly skip it; for marquee buildings (the city hall, the power plant) bake one with Laigter / Sprite DLight and the lit-window glow becomes substantially more convincing.
**Why this and not the fancier options:** ships in a weekend, no WebGPU compute-shader gymnastics, and it cleanly handles the actual gameplay-driven case: "every powered building lights its windows at night, ~hundreds of them, plus a moving sun."

### Option B — Screen-space lightmap (Slembcke style)
**What:** render lights into a separate light-map texture, multiply over the diffuse target at composite. The `bevy_2d_screen_space_lightmaps` crate (goto64) packages this technique for Bevy: *"You can find more details about the technique here: https://slembcke.github.io/2D-Lighting-Overview … One [camera] for rendering normal sprites. One for rendering the light map. One for the final image… These two textures are then multiply-blended."*
**Bucket:** SHOULD as an alternative to A. Especially good if you want lots of lights and don't need normal-mapped material response per pixel.
**Layers:** Pass (an off-screen light-map pass + a composite pass).

### Option C — Radiance Cascades 2D global illumination (research-tier)
**What:** Alexander Sannikov's algorithm: build a hierarchy of probes; each cascade level halves spatial probe density but quadruples angular resolution; rays are short and merged across cascades. The result is noiseless, scene-agnostic 2D global illumination with soft shadows. Holographic Radiance Cascades (Freeman/Sannikov/Margel, arXiv:2505.02041, 2025) is even better: *"It runs at constant cost for a given scene size, taking 1.85 ms for a 512×512 pixel image and 7.67 ms for 1024×1024 on an RTX 3080 Laptop GPU."* **This is the same algorithm family used in Path of Exile 2** — per radiance.wiki, *"Radiance Cascades is an approach to real-time global illumination, created by Alexander Sannikov at Grinding Gear Games for Path of Exile 2. First presented at ExileCon 2023."* Reference implementations for Bevy already exist: `Lommix/solis_2d`, `kornelski/bevy_flatland_radiance_cascades`, `nixonyh/bevy_radiance_cascades`.
**Bucket:** SHOULD only if you want this *as a feature of the game*, and SHOULD NOT otherwise — the engineering effort is much higher than A or B.
**Layers:** Pass (5–7 compute-shader passes for cascade construction + a final composite), Items (light entities still go through Extract).
**Hardware/API:** runs fine on WebGPU; it's "just" compute + storage textures + an HDR target. No bindless or ray-query needed.
**Code:**
```rust
commands.spawn((
    LightEmitter2d { color: WARM_WHITE, intensity: 8.0 }, // doubles as occluder if alpha>0
    Sprite { texture: lamp_tex, ..default() },
    Transform2d::from_xy(x, y),
));

#[derive(Resource)]
pub struct RadianceCascades { pub levels: u8, pub base_probe_px: u32, pub rays_per_probe_l0: u32 }
```
**Honest verdict:** this is a *huge* feature with publication-quality math; do it last, do it intentionally, and only because you want Spark's screenshots to make people gasp.

### 2D hard shadows from line-segment occluders
**What:** the classical "Sight & Light" / Red Blob Games "2D Visibility" algorithm (Amit Patel): for each light, find the polygon of visible space by ray-casting at every occluder endpoint ±ε and sweeping. Per Red Blob Games: *"Our strategy will be to sweep around 360° and process all of the wall endpoints. As we go, we'll keep track of the walls that intersect the sweep line… For the area between consecutive rays, we want to find the nearest wall."* The GPU-friendly alternative the same page recommends: *"consider using a subtractive algorithm (instead of the additive one shown here), rendering the shadow of each line segment as a quad… It will increase the rendering load on the GPU but it doesn't require sorting on the CPU."*
**Bucket:** CAN. Useful for fog-of-war / line-of-sight in a power-grid game (does this transformer see the failing line?). Skip if you're going with Radiance Cascades, which subsumes it.
**Layers:** Items (extract occluder segments), Pass (a `2d_shadows` pass writing into the light-map).

### Day/night cycle
**What:** a global directional tint + ambient-color ramp animated by a time-of-day Resource.
**Bucket:** SHOULD. The power-grid mechanic *needs* a day/night cycle so lit windows mean something.
**Layers:** Scene (`Resource TimeOfDay`), feeds the ambient term in your chosen lighting Option above.

## 2.3 Camera & view

### Multiple viewports / minimap / render-to-texture
**What:** render the world (or a different camera) to an off-screen texture; sample that texture in the UI pass.
**Bucket:** SHOULD. A minimap is non-negotiable for a city-builder.
**Layers:** Scene (`Camera2d { target: RenderTarget::Image(handle) }`), Pass (the render graph already supports named texture I/O — this is exactly what it's for).
**Code:**
```rust
let minimap_tex = images.alloc(UVec2::new(256, 256), RGBA8_SRGB);
commands.spawn((
    Camera2d { target: RenderTarget::Image(minimap_tex.clone()), zoom: 0.05, ..default() },
    Transform2d::IDENTITY,
));
// then sample minimap_tex in an egui Image, or in a UI Sprite.
```

### Camera shake / smooth follow / zoom
**What:** apply temporary offsets / damped springs / interpolated zoom levels to the Camera2d's `Transform2d`.
**Bucket:** SHOULD. Pure ECS systems; no renderer work.

## 2.4 Post-processing — beyond the R10 minimum

### FXAA (fast approximate AA)
**What:** a single fullscreen pass that detects luma edges and blurs across them. Cheap, no temporal data.
**Bucket:** SHOULD. The right AA for a 2D game; MSAA is wasted on sprite quads.
**Layers:** Pass.

### TAA / SMAA
**Bucket:** CAN, but **don't**. Temporal AA needs motion vectors, history textures, and a velocity buffer — overkill for crisp pixel-art sprites and outright wrong for them (ghosting). SMAA is a fine alternative to FXAA but its quality advantage shows on 3D geometry, not on sprites. Skip both, ship FXAA.

### Chromatic aberration / depth-of-field / motion-blur / dithering
**Bucket:** CAN (all but DoF). Each is a single extra fullscreen pass behind a toggle. DoF needs a meaningful depth signal which a 2D city-builder doesn't really have — skip DoF; the others are essentially free.

### LUT color grading
**What:** sample a 32³ LUT laid out as a 1024×32 (or 8×8 grid) texture using the current pixel's color as the lookup coord.
**Bucket:** SHOULD. The cheapest way to make day/night and zone tints feel cohesive.

### CRT / scanline / retro filter
**Bucket:** CAN as a *toggleable* fun filter. Wicked Engine recently shipped one; the WGSL is ~50 lines.

## 2.5 Animation

### Sprite-sheet animation
**What:** advance a UV-rect index over time; render as a normal Sprite.
**Bucket:** SHOULD. Standard.
**Layers:** Scene (`SpriteAnimation { frames: Vec<Rect>, fps, mode }`), pure ECS system updates the sprite's `rect` each frame.

### 2D skeletal animation (Spine / DragonBones / Esoteric format)
**Bucket:** CAN. Useful if Spark has characters. For buildings/vehicles, sprite-sheet is enough.
**Layers:** Scene (`SkeletalAnimation`), an integration crate; no renderer changes (the output is still textured quads with deformed UVs).

### 2D particle systems
**What:** spawn small sprites with simple physics; pool buffers; render as instanced quads with additive blending for embers/sparks.
**Bucket:** SHOULD. Smoke from factories, sparks from substations, steam from cooling towers — every city-builder leans on particles for "alive".
**Layers:** Scene (`ParticleEmitter` component, `Particle` entities), Pass (a `particles` pipeline, often additive).
**Code:**
```rust
commands.spawn((
    ParticleEmitter {
        rate: 50.0,
        lifetime: 1.5..2.5,
        size: 4.0..8.0,
        color: Gradient::new(&[(0.0, ORANGE), (1.0, BLACK.with_alpha(0.0))]),
        velocity: Cone { dir: Vec2::Y, spread: 0.3, speed: 20.0..40.0 },
        texture: smoke_tex,
        blend: BlendMode::Premultiplied,
    },
    Transform2d::from_xy(chimney_x, chimney_y),
));
```
GPU-side: keep particle simulation **on the CPU** for the city-builder's expected counts (few thousand max). A compute-shader particle system is FAR FUTURE territory.

## 2.6 Materials & shading

### Shader hot-reload
**What:** watch the WGSL files; on change, rebuild the shader module and the affected pipelines.
**Bucket:** SHOULD. Massive iteration-speed multiplier; trivial to add — `notify` crate watches the dir, the Pass-layer cache invalidates by file path.

### Material layering / decals (2D form: building "tagging", damage overlays, soot, rust)
**What:** draw a second textured quad over the base with a `Multiply` or `Overlay` blend that follows the host's transform.
**Bucket:** CAN. For Spark, this is most likely better expressed as a *marker effect component* (Section 1.4) — "this building has a `Damage(level)` marker, the damage pass overlays a rust texture clipped to the sprite's alpha". No need for a true decal projection system.

### Shader graphs / node-based materials
**Bucket:** NEVER (see Section 4). A solo developer doesn't need a visual shader graph; WGSL written by hand and reloaded live is more legible and inspectable, which is core to Spark's philosophy.

## 2.7 2.5D / isometric (R11)

### 2.5D / isometric projection
**What:** still a 2D renderer, but the camera projection is an *oblique* or true isometric transform; sprites are pre-rendered isometric blocks; sort order uses (x+y+z) projection instead of just y.
**Bucket:** SHOULD (R11 if you go that direction).
**Layers:** Scene (`Camera2d { projection: Iso { angle, ratio } }`), Items (new sort key).
**Why it's not really 2.5D**: nothing about the renderer changes — it remains textured quads with a different transform. The complexity is gameplay-side (snapping to iso grid, mouse-picking).

### Height-mapped tilemap (multi-layer)
**What:** for a city-builder, layered terrain where buildings occlude buildings behind them. Render the tilemap in N depth slices.
**Bucket:** CAN. Layered draw + the existing depth+Y-sort handles it.

## 2.8 Optional 3D bridge (R12)

### Mesh3d + Transform3d + a forward 3D pass
**What:** the engine's "door to 3D" — a separate `Mesh3d` component, `Transform3d`, a `Camera3d` view, and a forward 3D pipeline running in parallel with the 2D pipeline in the same render graph.
**Bucket:** CAN if you want it; otherwise FAR FUTURE.
**Layers:** Scene (`Mesh3d`, `Transform3d`, `Camera3d`), Items (a 3D draw phase), Pass (a `forward_3d` pass writing into the same HDR target the 2D path uses).
**Crucial constraint:** keep this *additive*. Transform2d and Transform3d stay separate; sprite/tilemap/2D-lighting passes don't change. The 2D path doesn't become a degenerate case of the 3D path — that's how Spark stays legible.
**Code:**
```rust
commands.spawn((
    Mesh3d(building_mesh.clone()),
    StandardMaterial { base_color: GREY, ..default() },
    Transform3d::from_xyz(0.0, 0.0, 0.0),
));
commands.spawn((Camera3d::default(), Transform3d::from_xyz(0.0, 5.0, 10.0).looking_at(Vec3::ZERO)));
```

### PBR (metallic-roughness) + normal/parallax mapping + emissive
**What:** the standard real-time PBR shader (Cook-Torrance GGX specular + Lambert diffuse + Schlick Fresnel + metallic/roughness/normal/emissive textures).
**Bucket:** CAN if R12 happens. Otherwise FAR FUTURE.
**Why it's plausible:** it's just a fragment shader and a material; nothing in the architecture resists it. Bevy's standard material is the reference implementation; you can ship a reduced version one piece at a time.

### Cascaded shadow maps for directional sun
**What:** render the scene depth from the sun's POV into 3–4 nested cascades; sample per-pixel during forward pass for shadow term.
**Bucket:** FAR FUTURE — only if R12.

---

# 3. FAR FUTURE

Possible in principle; large effort, only if the project grows a lot.

## 3.1 GPU-driven rendering (multi-draw indirect + GPU culling)
**What:** put per-instance data and culling on the GPU; replace many `draw()` calls with a single `multi_draw_indirect()` whose argument buffer is produced by a compute shader (frustum + occlusion culling). Bevy 0.16 shipped this for 3D meshes; per the Bevy 0.16 release notes by pcwalton (gist.github.com/pcwalton/7562c1a9b98bb5ae33ba2e52e41a89e5): *"Bevy 0.16 only supports GPU-driven rendering for the 3D pipeline, but the techniques are equally applicable to the 2D pipeline. Future versions of Bevy could support 2D GPU-driven rendering as well."* The classic vkguide.dev write-up describes the pattern in Vulkan; wgpu exposes it via the **native-only** `Features::MULTI_DRAW_INDIRECT` and `MULTI_DRAW_INDIRECT_COUNT` (NOT core WebGPU — Chrome/Dawn has it as an extension but the browser WebGPU spec does not).
**Bucket:** FAR FUTURE.
**Why FAR FUTURE for Spark:**
- A 2D city-builder simply does not have the draw-call pressure that justifies this. After the basic R3 batched-sprite pass, the entire scene fits in a few hundred draws.
- It would require either dropping wasm/WebGPU support (lose Web target) or a runtime fork between "native fast path with MDI" and "WebGPU slow path with regular draw" — exactly the kind of complexity that violates Spark's "no black boxes" tenet.
- Implementation cost (visibility buffers, count buffers, GPU-side culling shaders) is enormous relative to gain.
**If Spark ever goes 3D-heavy:** revisit this, with native-only feature gating.

## 3.2 Virtual geometry / Nanite (meshlets + visibility buffer + two-pass HZB occlusion)
**What:** preprocess meshes into a DAG of meshlets (≤128 triangles each), GPU-cull per meshlet against a hierarchical depth buffer, rasterize into a visibility buffer, shade in a fullscreen pass. Bevy has an experimental implementation by @JMS55 using compute-shader software rasterization + visibility buffers, scheduled for de-experimentalization (and per JMS55's "Bevy's Fifth Birthday" blog, sustained work across 0.14–0.17). The key insight (from the Bevy 0.10433 discussion): *"Meshlets + visibility buffer + two pass occlusion culling + gpu-driven rendering gives you 60-70% of Nanite's benefits."*
**Bucket:** FAR FUTURE.
**Why FAR FUTURE for Spark:**
- It's *only* useful when you have hundreds of millions of triangles. Spark might have a few thousand low-poly building meshes if it ever ships 3D buildings.
- Requires either mesh shaders (wgpu 28+ has `EXPERIMENTAL_MESH_SHADER`; not in WebGPU core) or compute-shader software raster (large effort).
- The Bevy implementation alone is a multi-year effort by a senior graphics programmer.

## 3.3 Software ray-traced GI (Tiny Glade style)
**What:** trace rays in compute shaders against a low-resolution proxy geometry / BVH; one ray per 4×4 pixel block; encode result in spherical harmonics; denoise; bilinearly upsample. Per the Tiny Glade technical reporting (hardforum.com summary): *"Tiny Glade's diffuse global illumination uses software ray tracing, so it scales down to ultra low-end and older GPUs … the game traces against lower complexity proxy geometry and shoots rays at sub-native resolution, with one ray for every 4x4 pixel grid … the lighting is encoded into a spherical harmonic to smooth things out and is simply denoised as a post process."*
**Bucket:** FAR FUTURE if Spark goes 3D. (For 2D, prefer Radiance Cascades.)
**Hardware/API:** runs on any GPU with compute (no ray-query hardware needed). The cleverness is in the BVH-against-proxy + screen-space-first-then-world-space hybrid.

## 3.4 Hardware ray-traced lighting (Bevy Solari class)
**What:** use a hardware BVH (via inline ray queries) to do ReSTIR DI + ReSTIR GI + a world-space irradiance cache, denoised with DLSS Ray Reconstruction. Bevy 0.17 shipped `bevy_solari` as experimental; per JMS55's "Realtime Raytracing in Bevy 0.18 (Solari)" blog (jms55.github.io), *"The major feature of Solari 0.18 was added support for specular materials. We added a separate specular GI pass that does 0-3 bounce pathtracing by sampling the GGX lobe of the BRDF via bounded VNDF sampling. We also fixed a major energy loss bug in light tile packing, reduced ReSTIR resampling bias, and made the world cache more reactive and much more performant on large scenes like Bistro."*
**Bucket:** FAR FUTURE (essentially never for a 2D city-builder).
**wgpu/WebGPU state (2025–2026):** wgpu exposes `Features::EXPERIMENTAL_RAY_QUERY` for inline ray queries with experimental acceleration-structure support on Vulkan and Metal (PR #8071), but: (a) it is experimental and subject to change, (b) WebGPU has no ray-tracing in the spec, (c) custom material support requires raytracing-pipeline support in wgpu (not yet — Solari's author lists it as blocking custom materials), (d) you need a denoiser and DLSS-RR is NVIDIA-only.
**Why this is honestly out of scope:** even Bevy's Solari is an experimental feature targeting AAA-class developers, with the JMS55 quote: *"if Bevy ever wants to attract AAA game developers, we need these kinds of systems"*. Spark is a 2D city-builder; it cannot justify this complexity.

## 3.5 Voxel GI / Light Propagation Volumes / DDGI probe GI
**Bucket:** FAR FUTURE (3D only). Implementable in WebGPU compute; the question is whether any 3D Spark scene ever justifies it. Almost certainly not.

## 3.6 SSAO / GTAO / HBAO (screen-space ambient occlusion)
**What:** approximate ambient occlusion from the depth+normal buffer in a fullscreen compute/fragment pass.
**Bucket:** FAR FUTURE (only if 3D). For 2D, the equivalent is baked into the sprites.

## 3.7 Screen-space reflections (SSR)
**Bucket:** FAR FUTURE (3D only).

## 3.8 Volumetrics / god rays / fog
**Bucket:** FAR FUTURE (3D). In 2D, a "god rays" effect is just a radial blur in a post pass — that's SHOULD/CAN territory.

## 3.9 Atmospheric scattering / procedural sky
**Bucket:** FAR FUTURE (3D). Bevy 0.16 shipped procedural atmospheric scattering with a raymarching mode added in 0.17 — beautiful, totally irrelevant to a 2D top-down city-builder.

## 3.10 GPU skinning + morph targets
**Bucket:** FAR FUTURE (3D). Needed only if Spark ever shows skinned 3D characters.

## 3.11 Upscaling (FSR / DLSS / XeSS / TSR / MetalFX)
**What:** render at lower resolution, upscale to display resolution with temporal+ML reconstruction.
**Bucket:** FAR FUTURE / NEVER for Spark. A 2D top-down game at native resolution costs almost nothing per pixel — upscaling exists to recover headroom you don't need. DLSS is NVIDIA-only; FSR is open source but its quality showcase is on 3D content with motion vectors, which Spark doesn't produce.

---

# 4. NEVER

Each entry: what the tech is, and the honest reason Spark won't build it.

### 4.1 True bindless textures
**What:** GPUs let shaders index into giant arrays of texture descriptors by an integer ID. This is how GPU-driven rendering, virtual texturing, and Nanite-class shading get away with one draw call for the whole scene. Bevy 0.16's GPU-driven path uses bindless for materials.
**Why NEVER on Spark:** WebGPU **does not have bindless** in the core spec as of May 2026. The gpuweb/proposals/bindless.md proposal acknowledges this directly: *"In the current (non-bindless) WebGPU binding model, shaders have access to a small set of resources that are in the GPUBindGroups currently bound at the time draw* or dispatch* is called."* wgpu has partial native-only `binding_array` support behind feature flags with severe limitations (per the wgpu Bindless Tracking Issue #3637, including unresolved items like "No feature combination allows Metal to use STORAGE_RESOURCE_BINDING_ARRAY"). Building Spark on a feature that *only* works native-Vulkan and is unstable would break the cross-platform promise and would mean rewriting the renderer when the spec lands. **Texture arrays + atlas pages are the right answer until WebGPU bindless ships.**

### 4.2 Hardware ray tracing via WebGPU
**What:** dedicated GPU units (RT cores) traverse a BVH and report hits.
**Why NEVER:** WebGPU has no ray-tracing API. wgpu's `EXPERIMENTAL_RAY_QUERY` is native-only, experimental, lacks pipeline ray tracing (so no shader execution reordering, no custom hit shaders), and is on Vulkan/Metal only as of v28+. The wgpu ray_tracing.md docs explicitly say: *"wgpu supports an experimental version of ray tracing which is subject to change … may have major bugs … expected to be subject to breaking changes."* Even if you took this dependency, the use case (real-time path-traced GI on a 2D city-builder) doesn't exist. **No clean API in Spark's model:** you'd need a `RaytracingMesh3d`-style component bypassing the entire 2D pipeline, a BLAS/TLAS asset type, and a denoiser; that's a separate engine.

### 4.3 Mesh shaders / task shaders
**What:** a replacement for the vertex pipeline where the GPU itself decides what to draw — workgroups generate meshlets, cull, LOD, and emit primitives. The basis of Unreal's Nanite. Per the wgpu mesh_shading.md spec: *"Mesh shaders are most effective in scenes with many polygons … Scenes that are not bottlenecked by geometry (perhaps instead by fragment processing or post processing) will not see much benefit from using them."*
**Why NEVER on Spark:** WebGPU does not have mesh shaders. wgpu has `EXPERIMENTAL_MESH_SHADER` (v28) fully on Vulkan, passthrough on Metal/DX12 — native-only, experimental, and irrelevant for sprites. **There is no sensible API for sprite quads in mesh-shader form** that adds anything over instanced quads.

### 4.4 Nanite-class virtualized geometry
**What:** see 3.2.
**Why NEVER:** wrong genre (no high-poly 3D), wrong team size (Bevy's JMS55 has been working on Bevy's implementation since 2024 across 4 releases and it's still experimental, with the JMS55 quote: *"Continuing virtual geometry and Solari is a given"* — meaning years more work), and wrong API (best path uses mesh shaders, which WebGPU lacks).

### 4.5 Pipeline ray tracing (hit/miss/anyhit shader stages, shader execution reordering)
**Why NEVER:** wgpu only supports inline ray queries; per wgpu's ray_tracing.md, *"Ray tracing pipelines are currently in development."* Even when it lands it won't be in WebGPU. Without it you can't do clean custom-material RT shading. Spark doesn't need it anyway.

### 4.6 Visual shader graphs / node-based materials
**What:** a UI where artists wire up nodes that compile to a shader (Unreal Material Editor, Unity Shader Graph, Godot Visual Shader).
**Why NEVER:** Spark is a solo-developer Rust learning project. The audience is one person writing WGSL. A shader graph is a multi-engineer-year project building an IDE inside your engine, and it contradicts Spark's "legible, inspectable, no black boxes" tenet. **No clean API in Spark's model**: materials are *plain Rust structs* deriving `Material2d` with a `#[fragment("path.wgsl")]` — adding a graph would mean shipping a compiler, a node UI, a serialization format, and a runtime that emits WGSL strings. Just write the WGSL.

### 4.7 Voxel cone tracing GI / SVOGI / Lumen-class GI
**Why NEVER:** 3D-only feature, AAA-scale engineering effort. Crytek's SVOGI, Unreal's Lumen, and NVIDIA's VXGI are all multi-person-year projects.

### 4.8 Forward+ / clustered shading on the 3D path
**What:** divide screen into tiles or view frustum into 3D clusters; per-cluster light list; shade with O(lights-per-cluster) instead of O(total-lights).
**Why NEVER (unless Spark turns 3D-heavy):** this is the right answer for "thousands of dynamic lights in a 3D scene". A 2D top-down power-grid game with hundreds of lights doesn't need it; a simple forward pass with a flat light list works at 60 fps.

### 4.9 Skinned mesh GPU compute / morph targets
**Why NEVER (unless 3D characters):** wrong genre. Buildings don't skin.

### 4.10 Hardware-accelerated upscaling (DLSS / FSR-Native / XeSS via DirectSR)
**Why NEVER:** wrong API surface (DLSS is NVIDIA's NGX SDK, not exposed through WebGPU; FSR is shippable in WGSL but solves a problem Spark doesn't have), wrong genre. A 2D city-builder is CPU-bound long before it's pixel-bound.

### 4.11 Async compute (independent compute-queue parallelism)
**What:** submit compute work to a separate hardware queue running in parallel with graphics. Common on Vulkan/DX12.
**Why NEVER:** WebGPU has a single queue. wgpu reflects this. Not exposed; not coming.

### 4.12 64-bit atomics / int64 in shaders
**Why NEVER:** experimental in wgpu only on some backends; WGSL spec does not require them. Spark has no need (Nanite-class visibility-buffer rasterizers use them; Spark doesn't).

### 4.13 Vulkan device-generated commands / DX12 work graphs
**What:** the next step beyond multi-draw-indirect — entire command lists generated on the GPU.
**Why NEVER:** native-only, vendor-divergent, not in WebGPU, not in wgpu. The Bevy 0.16 release notes explicitly say *"We're watching new API features, such as Vulkan device generated commands and Direct3D 12 work graphs, with interest"* — but watching, not implementing. Spark should ignore.

### 4.14 Multi-threaded command recording across many threads
**What:** record multiple `wgpu::CommandEncoder`s in parallel on different threads.
**Why NEVER (in this form):** wgpu does allow it, but Spark's frame is dominated by Extract/Prepare/Queue/Sort, not command recording — and Spark's deliberate single-World ECS optimizes for legibility, not multi-threaded record concurrency. If profiling later shows Record as a bottleneck (it won't), revisit. Single-threaded record is the right default.

---

# 5. OPTIMIZATIONS — honest need/no-need verdicts

For each optimization: what it does, and **does Spark actually need it?**

### 5.1 Sprite/quad batching (CPU-side)
**What:** consolidate consecutive sprites with the same texture/pipeline into one instanced draw.
**Verdict:** **NEEDED, day 1.** This is R3. Without it, a city of 5000 visible tiles is 5000 draw calls and the renderer is hopeless. With it, the same city is ~10 draws.

### 5.2 Texture atlasing
**Verdict:** **NEEDED.** It's the *prerequisite* for batching: GPUs only batch within the same bound texture. R3.

### 5.3 Array textures (`texture_2d_array`)
**Verdict:** **NEEDED at scale.** Lets you have several atlas "pages" reachable in one bind group; in the fragment shader, the per-instance buffer contributes a layer index. Cheap; portable on WebGPU; the right "poor-man's bindless".

### 5.4 Instancing (instance buffer with per-quad transform/UV/color)
**Verdict:** **NEEDED.** This is *how* batching is implemented. Free.

### 5.5 Frustum culling (CPU, per sprite/chunk)
**Verdict:** **NEEDED for the tilemap and per-entity sprites.** But: keep it CPU-side and per-chunk for tiles, per-AABB for entities. A KDTree/grid is overkill; a chunk-AABB-vs-camera-AABB test is one branch per chunk.

### 5.6 Occlusion culling (GPU, hi-Z, two-pass)
**Verdict:** **NOT NEEDED. Premature.** A 2D top-down view has almost no occlusion (buildings overlap by a few pixels, not a lot). Even in 3D, the Bevy 0.16 release notes caution: *"occlusion culling won't be faster on all scenes. Small scenes, or those using simpler non-PBR rendering are particularly likely to be slower with occlusion culling turned on."* The cost of an occlusion pipeline (HZB construction + a culling compute pass + a second-pass refinement) dwarfs the savings for 2D.

### 5.7 LOD (level of detail)
**Verdict:** **NOT NEEDED in 2D.** Sprites are pre-rasterized at fixed sizes; you're already at "the right LOD" by definition. For tilemaps you might keep a *miplevel* of the atlas (see 5.16) for far zoom, which is mipmapping, not LOD.

### 5.8 Dirty-rect / damage-region rendering
**What:** only re-render the screen regions that changed since last frame.
**Verdict:** **NOT NEEDED for Spark. Actively harmful.** Modern GPUs are insanely fast at "redraw the whole 2D scene"; dirty-rect bookkeeping is a CPU-and-correctness burden. This made sense in 1995 (DOS/Win16 GDI), not 2026. Keep it filed under "if you ever build a 2D editor app".

### 5.9 Multi-draw indirect (`multi_draw_indirect`)
**Verdict:** **NOT NEEDED.** Per Dawn's `multi_draw_indirect.md`: *"wgpu::FeatureName::MultiDrawIndirect feature must be enabled to use the commands. Most desktop GPUs support this feature."* — but it's an extension feature, not core WebGPU. Spark's draw counts after batching are O(hundreds); MDI's payoff starts at O(tens of thousands).

### 5.10 GPU-side culling (compute shader emits the indirect-draw buffer)
**Verdict:** **NOT NEEDED.** Couples with 5.9; same reasoning.

### 5.11 Pipeline cache (persistent, between program runs)
**What:** save compiled pipeline binaries to disk so the next launch is fast. wgpu has `PipelineCache` — per its docs: *"In most GPU drivers, shader code must be converted into a machine code which can be executed on the GPU. Generating this machine code can require a lot of computation. Pipeline caches allow this computation to be reused between executions of the program … This resource currently only works on the following backends [Vulkan/Android]."* Mostly relevant on Android/Vulkan; desktop drivers manage their own caches.
**Verdict:** **CAN ADD LATE.** Useful if Spark ever gets a long pipeline-compile time at startup. Not urgent.

### 5.12 Pipeline pre-compilation / specialization-key precomputation (to avoid shader stutter)
**What:** the Unreal/DX12 industry-wide problem: don't compile pipelines mid-frame; compile them all up front based on known specialization keys.
**Verdict:** **MILD NEED.** Build a clean `PipelineCache` keyed by `(MaterialTypeId, VariantKey)` in your Pass layer, populate at startup or at level-load. WGSL compiles via Naga are fast; shader stutter as the DX12/UE5 community knows it is *not really a wgpu problem at Spark's scale*. But ensuring you never create a pipeline mid-frame is hygiene worth enforcing.

### 5.13 Bindgroup caching
**What:** dedup `BindGroup` creation by hashing `(layout, resources)`.
**Verdict:** **NEEDED at modest scale.** Recreating a bindgroup per sprite would kill you; the existing Pass layer cache is the right place. Hash by `(layout_id, [resource_ids])`.

### 5.14 Buffer pooling / suballocation (one big vertex/instance buffer, suballocate)
**Verdict:** **NEEDED for the per-frame instance buffer.** Don't `create_buffer` per frame. A ring buffer of N MB suballocated per phase is standard. For everything else (mesh assets, textures): no, the asset system already manages these as long-lived resources.

### 5.15 Mipmapping
**What:** precompute downsampled versions of textures so the GPU samples the right level per pixel; avoids shimmering and improves cache locality.
**Verdict:** **NEEDED for tilemap atlases at zoom-out.** Generated once at upload time via a simple compute shader or a downsample-blit chain. For pixel-art sprites where you want crisp pixels at any zoom, *don't* mip — use nearest sampling.

### 5.16 Texture compression (BCn / ASTC / ETC2)
**Verdict:** **NICE TO HAVE.** wgpu supports BC, ASTC, ETC2 via feature flags. For a 2D city-builder shipping a few hundred MB of atlases, BC7 cuts VRAM significantly and is a free win. Defer until you have textures big enough to care.

### 5.17 Texture streaming
**What:** load high-resolution texture data only when the camera gets close.
**Verdict:** **NOT NEEDED.** Top-down 2D city-builders don't have streaming-scale data.

### 5.18 Render-graph resource aliasing / transient memory
**What:** detect that texture A (used in pass 1) and texture C (used in pass 3) have non-overlapping lifetimes and back them with the same GPU memory. The Frostbite frame-graph paper and the AMD GPUOpen RPS SDK both describe how *"if Pass A and Pass B use similar textures but don't overlap, the same texture can be used for both"* (Tony Adriansen's Vulkan render-graph blog).
**Verdict:** **NOT NEEDED for Spark.** Aliasing matters when you have a 20-pass deferred renderer with 8K G-buffers. With 5–10 passes and small intermediate targets, you save kilobytes. Skip.

### 5.19 Render-graph pass culling
**What:** if pass X's output is consumed by nothing, skip it.
**Verdict:** **NICE TO HAVE, trivial.** Once the render graph is real (post-R8 or so), a topological sort + reverse reachability from the swap-chain output cleanly drops unused passes. This is also a great debug tool ("why isn't this overlay showing up?").

### 5.20 Double / triple buffering of the swapchain
**Verdict:** **HANDLED BY WGPU.** Configure `SurfaceConfiguration::desired_maximum_frame_latency`. Don't reinvent.

### 5.21 GPU timestamp profiling
**What:** insert timestamp queries between passes; readback measures pass duration.
**Verdict:** **NEEDED for any serious optimization.** Use `Features::TIMESTAMP_QUERY` + integrate with Tracy via Wumpf's **`wgpu-profiler`** crate, whose README enumerates the integrations: *"Tracy integration (behind tracy feature flag) · Puffin integration (behind puffin feature flag) · chrome trace flamegraph json export"*. Bevy 0.17 wired exactly this into its Tracy integration: PR #18490 ("Tracy GPU support by JMS55") states *"Build on top of the existing render diagnostics recording to also upload gpu timestamps to tracy."*

### 5.22 RenderDoc frame capture support
**Verdict:** **FREE WIN.** RenderDoc just works with wgpu on Vulkan/DX12. Per the RenderDoc FAQ: *"Currently RenderDoc supports Vulkan 1.4, D3D11 (up to D3D11.4), D3D12, OpenGL 3.2+, and OpenGL ES 2.0 - 3.2."* For macOS, use Xcode's Metal frame capture. The Bevy profiling docs explicitly note *"while RenderDoc is a great debugging tool, it is not a profiler, and should not be used for this purpose"* — so use it for capture and inspection, use Tracy for timing. Cost to Spark: label your resources (`label: Some("sprite_pipeline")`) and you're done. The `pramberg/bevy_renderdoc` crate is a useful reference for the Rust integration shape.

### 5.23 Overdraw / mip / wireframe debug views
**Verdict:** **NEEDED later.** Add as togglable post-passes after R9: count fragment-shader invocations per pixel, render with a heatmap. Catches "your transparent particles are killing the GPU" in one screenshot.

### 5.24 Render-graph visualizer
**Verdict:** **NICE TO HAVE.** Once the graph has > 8 passes, an `egui` window listing nodes + edges saves you from staring at code. Bevy has one; you can do something simpler.

### 5.25 Async asset upload (load texture on a worker thread, `queue.write_texture` after)
**Verdict:** **NEEDED if you ever stall on load.** A naive `image::open` on the main thread will freeze. Use `tokio::task::spawn_blocking` or `rayon` + a transfer queue (which wgpu doesn't expose as a separate queue, but `queue.write_texture` from a worker thread works because `Queue: Send + Sync`).

### 5.26 Upscaling (FSR/DLSS/XeSS/TSR/MetalFX)
**Verdict:** **NOT NEEDED.** See 3.11 / 4.10.

### 5.27 MSAA
**What:** multi-sample anti-aliasing — the GPU rasterizer takes multiple samples per pixel.
**Verdict:** **NOT NEEDED.** Wrong AA for a 2D sprite renderer; FXAA in post is cheaper and looks better on sprite edges.

### 5.28 Async compute / multiple queues
**Verdict:** **NOT POSSIBLE on WebGPU.** Single queue. See 4.11.

---

# Appendix A — Roadmap mapped to milestones

| R-tag | Feature | Bucket | Notes |
|---|---|---|---|
| R0 | Hardcoded triangle | MUST | Gpu+Pass smoke test |
| R1 | Textured quad | MUST | Bind-group template |
| R2 | Sprite component, staged pipeline | MUST | First Workload run-through |
| R3 | Batched sprites + atlas | MUST | The line into "real renderer" |
| R4 | Camera2d-as-View + Painter | MUST | RETAINED + IMMEDIATE both work |
| R5 | Depth + Y-sort | MUST | Opaque/transparent split |
| R6 | Tilemap (chunked GPU) | MUST | One quad per chunk |
| R7 | Custom Material2d | MUST | Frozen macro API |
| R8 | egui UI overlay | MUST | Dev velocity unlock |
| R9 | Visualization overlays + effect passes | MUST | Power-grid is the whole game |
| R10 | Post-processing (HDR/tonemap/bloom/vignette + LUT) | MUST | Stop at minimum chain |
| R11 | 2.5D / isometric | SHOULD | Optional path |
| R12 | 3D meshes (Mesh3d, Camera3d, forward pass) | CAN | Additive door, not a rewrite |
| post-R12 | Day/night + 2D lighting (pick A or C) | SHOULD | Genre fit |
| post-R12 | SDF/MSDF text | SHOULD | Zoom-stable text |
| post-R12 | Particles | SHOULD | "Alive" cities |
| post-R12 | Shader hot-reload | SHOULD | Iteration speed |
| post-R12 | wgpu-profiler + Tracy + RenderDoc labels | SHOULD | Hygiene |
| horizon | GPU-driven 2D rendering | FAR FUTURE | Not before draw-call pressure exists |
| horizon | Radiance Cascades | FAR FUTURE / SHOULD | Only if you want the look |
| never | Bindless, RT GI, mesh shaders, Nanite | NEVER | API + genre + team-size |

# Appendix B — Honest closing opinion

Spark's biggest risk is the wrong kind of ambition. A 2D city-builder *does not need 60% of what modern renderers ship*. The renderer is a means to put pixels on screen so the simulation can be played; chasing Nanite, ReSTIR, voxel-GI, or even GPU-driven 2D rendering is engineering theater for this genre. **Build R0–R10, ship one strong lighting story, add Tracy + RenderDoc hygiene, then go back to the simulation code where the actual game lives.** The 3D door (R12) should stay locked unless you have a concrete reason to open it.
