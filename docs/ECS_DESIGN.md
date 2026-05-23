# Spark ECS — Architecture & Build Plan

This is the design and step-by-step build plan for the `lib/ecs/` crate. We're rolling it from scratch as a learning exercise.

## Design philosophy

**Bevy-style API** for ergonomics, **Shipyard-inspired workloads** for explicit grouping and ordering, **sparse-set storage** for implementation simplicity, **traceability built in** so the editor can inspect everything live, **parallel-execution-ready** but single-threaded first.

The non-negotiable rule: **all memory management is ECS-based**. The window handle, the wgpu device, the input state, the asset cache, the power network — every piece of long-lived state lives in the World as either a Resource (singleton) or an Entity (many). Nothing in global statics, nothing in side allocations.

## Goals & non-goals

**Goals:**
- Function-parameter system extraction (Bevy-style): `fn sys(time: Res<Time>, q: Query<(&mut A, &B)>)`.
- Named, first-class workloads with explicit ordering between them.
- Full system traceability: every system call recorded with timing, access set, command/event counts.
- Editor-friendly reflection: list entities, components, resources, system graph, frame timings — all readable at runtime.
- Sparse-set storage for simplicity. Archetype migration possible later behind the same API.
- **Parallel system execution is a committed M4 deliverable**, not a stretch goal: lockless via per-system access sets + `Send + Sync` component/resource bound. Phase 1 ships a sequential executor, but the `Access` model and DAG/batch structure are built from day one as the safety proof for the M4 switchover.
- Default to safe Rust. `unsafe` is allowed when paired with a documented `# Safety` contract and an *enforced* check (runtime assertion, structural invariant, or type-level proof) — not reviewer trust — and confined to one `unsafe fn` per concern with `SAFETY:` comments at every call site. Example: roadmap issue 1b's `DenseMut::get` for multi-mut query joins, paired with `assert_no_self_conflict`.

**Non-goals (for v1):**
- Archetype storage. Stretch refactor after the API stabilises.
- Networking, save serialization, or scripting — separate concerns, layered on top later.

## Architecture overview

```
                              App
                               │
                       ┌───────┴───────┐
                     World          Scheduler
                       │                │
              ┌────────┼────────┐       │
          Entities  Components  Resources
                       │              Stages (Startup, First, PreUpdate,
                       │                          FixedUpdate, Update,
                       │                          PostUpdate, Render, Last)
                       │                │
                       │              Each Stage contains Workloads
                       │                │
                       │              Each Workload contains Systems
                       │                │
                     Events ─── consumed by Systems via EventReader/Writer
                       │
                   Commands ──── deferred mutations, flushed between Workloads
```

Per frame, the App calls the Scheduler. The Scheduler runs each Schedule in order, each Workload in dependency order, each System extracts its params from the World, mutations queue into Commands, command queues flush between Workloads, events drain at end of frame.

## Core types — schema reference

### Entity

Generational ID. Pure data. `Copy`.

```rust
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Entity {
    index: u32,
    generation: u32,
}
```

When an entity is despawned, its index goes onto a free list; on reuse, generation bumps. Old `Entity` handles to recycled slots fail `is_alive` cleanly.

### Component

Any `'static + Send + Sync` type marked with the derive:

```rust
#[derive(Component, Debug, Clone, Copy)]
pub struct Position(pub Vec2);

#[derive(Component, Debug, Clone)]
pub struct Sprite {
    pub texture: TextureHandle,
    pub size: Vec2,
    pub tint: Color,
}

#[derive(Component)]
pub struct PlayerControlled;        // marker (zero-sized)

#[derive(Component)]
pub struct Operational;             // marker
```

The `#[derive(Component)]` macro:
- registers the type at startup in a `ComponentRegistry` (name, `TypeId`, `Debug` formatter, optional serde hooks)
- emits zero runtime cost beyond a one-shot inventory entry
- exposes the type to the editor for inspection

### Resource

Singletons. Exactly one per type per world. Anything "engine-global" lives here.

```rust
#[derive(Resource)]
pub struct Time {
    pub delta: f32,
    pub fixed_delta: f32,
    pub elapsed: f32,
    pub frame: u64,
}

#[derive(Resource)]
pub struct Window {
    pub handle: winit::window::Window,
    pub width: u32,
    pub height: u32,
}

#[derive(Resource)]
pub struct RenderContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
}

#[derive(Resource, Default)]
pub struct InputState {
    pub keys_held: BitSet,
    pub keys_just_pressed: BitSet,
    pub mouse_pos: Vec2,
    pub mouse_buttons: u8,
}

#[derive(Resource, Default)]
pub struct PowerNetwork {
    pub supply: f32,
    pub demand: f32,
    pub ratio: f32,
}
```

The `#[derive(Resource)]` macro registers the type for editor inspection just like components.

### Event

Typed messages between systems. Double-buffered across frames so readers don't miss them.

```rust
#[derive(Event, Debug, Clone)]
pub struct TileClicked {
    pub tile: IVec2,
    pub button: MouseButton,
}

#[derive(Event)]
pub struct ConstructionCompleted(pub Entity);

#[derive(Event)]
pub struct CityTierUp {
    pub city: Entity,
    pub new_tier: u32,
}
```

### Query

A query is a declarative description of which entities a system reads or writes.

```rust
Query<&Position>                                            // read
Query<&mut Position>                                        // write
Query<(&mut Position, &Velocity)>                           // tuple
Query<&Worker, Without<CurrentJob>>                         // with filter
Query<&Plant, (With<Operational>, Without<UnderMaintenance>)>  // multiple filters
Query<(&Position, Option<&Velocity>)>                       // optional component
Query<(Entity, &Position)>                                  // entity ID alongside
```

Filters available on day 1: `With<T>`, `Without<T>`, tuple of filters. Deferred: `Or<(F1, F2)>` (post-M4); `Changed<T>`, `Added<T>` (Phase 2 — depends on the change-tick storage slot).

### System

A regular Rust function. The signature declares what it needs from the world:

```rust
fn movement(
    time: Res<Time>,
    mut q: Query<(&mut Position, &Velocity)>,
) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0 * time.delta;
    }
}
```

**Parameter rules:**

1. Order doesn't matter. Each parameter is looked up by type. `fn(a: Res<X>, b: Res<Y>)` and `fn(b: Res<Y>, a: Res<X>)` are equivalent.
2. You declare only what you need. Zero parameters is valid (e.g. startup logging). One. Twenty. Any subset.
3. Conflicting parameters within one system panic at registration (e.g. two `&mut` queries over the same component type without disjoint filters).
4. Conflicting parameters *between* systems in the same workload cause those systems to run sequentially, not in parallel — handled automatically by the scheduler.

**Available parameter types:**

| Param | Purpose |
|-------|---------|
| `Res<T>` | Read a resource |
| `ResMut<T>` | Write a resource |
| `Query<D>` | Read entities (data shape `D`) |
| `Query<D, F>` | Same with filter `F` |
| `Commands` | Defer entity spawn/despawn/insert/remove |
| `EventReader<E>` | Read events from this frame and last |
| `EventWriter<E>` | Send events |
| `Local<T>` | Per-system local state, persists between calls |
| `Entities` | Direct access to the entity allocator (rare) |
| `&World` / `&mut World` | Escape hatch — system runs alone, no parallelism |

### Commands — deferred mutations

Systems never mutate world structure directly. They queue commands; the scheduler flushes them between workloads.

```rust
fn place_plan(
    mut events: EventReader<TileClicked>,
    economy: Res<Economy>,
    mut cmd: Commands,
) {
    for click in events.read() {
        if economy.capital >= 100.0 {
            cmd.spawn((
                Plan { tile: click.tile, kind: BuildingKind::WaterWheel },
                Position(click.tile.as_vec2()),
                ConstructionProgress { current: 0.0, required: 30.0 },
            ));
            cmd.update_resource::<Economy>(|e| e.capital -= 100.0);
        }
    }
}

fn finish_construction(
    mut cmd: Commands,
    completed: Query<Entity, With<ConstructionDone>>,
) {
    for e in &completed {
        cmd.entity(e)
           .remove::<ConstructionProgress>()
           .remove::<ConstructionDone>()
           .insert(Operational);
    }
}
```

Commands available:
- `cmd.spawn((A, B, C))` — spawn an entity with a bundle (tuple) of components
- `cmd.despawn(entity)` — remove the entity and all its components
- `cmd.entity(e).insert(comp).remove::<T>()` — incremental edits to an existing entity
- `cmd.insert_resource(r)` / `cmd.update_resource::<T>(|t| ...)` / `cmd.remove_resource::<T>()`
- `cmd.send_event(e)` — equivalent to `EventWriter<E>::write(e)`

Always deferred. Single-threaded. Deterministic.

### Workload — first-class system group (Shipyard-inspired)

A workload is a named bundle of systems that belong together. Systems inside a workload can run in parallel when access allows. Workloads have explicit ordering between each other within a stage.

Work is registered two ways, by intent — two separate mechanisms sharing only the `Stage` they sit in. **Sequential** systems go on the `Application` (`app.add_system(stage, fn)`): they run in the calling thread, in registration order, with no batching or conflict-checking between them. A **parallel-capable** group is a workload (`app.add_workload(label, stage, |w| { … })`, which forwards to a per-`Stage` `Schedule`): the scheduler batches its systems by access disjointness. `Schedule` is a workloads-only container — it has no `add_system`, no anonymous workload. Within a stage, `run_stage` runs the sequential systems first, then the stage's workloads, then one command flush.

```rust
// One enum per subsystem; each variant is a workload label. The derive
// reads the variant names at compile time, so it generates both the
// identity (`TypeId` + variant index) and the `&'static str` name — which
// is why it works on an enum, not just a unit struct.
pub trait WorkloadLabel: 'static {
    fn id(&self) -> WorkloadId;     // TypeId + variant — generated by the derive
    fn name(&self) -> &'static str; // the variant name — generated by the derive
}

#[derive(WorkloadLabel)]
pub enum Workload {
    Input,
    Simulation,
    PowerGrid,
    CityTick,
    Construction,
    Rendering,
}

app.add_workload(Workload::Input, Stage::PreUpdate, |w| {
    let poll  = w.add_system(poll_window_events);          // add_system → SystemRef (Copy handle)
    let state = w.add_system(update_input_state).after(poll);
    w.add_system(update_mouse_world_pos).after(state);
});

app.add_workload(Workload::PowerGrid, Stage::FixedUpdate, |w| {
    // These run in parallel — disjoint access, no order declared.
    let supply = w.add_system(collect_supply);
    let demand = w.add_system(compute_demand);

    // Ordered against the handles above — a partial order, not a linear chain.
    let distribute = w.add_system(distribute_power).after(supply).after(demand);
    w.add_system(emit_blackout_events).after(distribute);
})
.after(Workload::Simulation);   // workload ordering: same verb, label arg, on the chained return
```

Workload ordering uses **one `.after`/`.before` verb pair, reused at both levels** — only the argument type changes:
- **Systems** order against the `SystemRef` handle returned by `add_system`: `.after(handle)` / `.before(handle)`. Calls accumulate (`.after(a).after(b)` = "after both"). Handles, not `fn` items, because the same function added twice must stay distinguishable.
- **Workloads** order against a `WorkloadLabel` on the `add_workload(...)` **return**: `.after(label)` / `.before(label)`. No handle is needed — every workload already carries a label, and labels are `pub` enum variants, so a plugin orders against another plugin's workload with no shared state. (`add_workload` therefore returns an *ordering builder*, not `&mut App`; it does not fluent-chain with `init_resource` / `add_event`.)
- **An undeclared order between two conflicting systems — or two conflicting workloads — is a registration error** (decision (a)): overlapping write access with no `.after`/`.before` between them is rejected, not silently ordered. Assert, per pair, that either order is fine with `.any_order_with(handle | label)` — a property of the code, which the scheduler trusts but cannot verify. Because `Commands` declares zero access, this fires only on real component/resource clashes.
- **Labels resolve lazily, at schedule-build time** — you may `.after(Workload::X)` before `X` is registered. Eager resolution would reintroduce "register A before B", the implicit ordering decision B2 abolishes.
- Commands flush at workload boundaries (all queued commands apply after a workload completes, before the next starts) — this is what makes a workload the atomic unit; a system needing a *different* upstream belongs in its own workload.
- Editor visualizes workloads as a labelled DAG.

> **Why handles + `.after`/`.before`, not `.chain()`.** An earlier draft ordered systems with `.after(some_fn)` and `.after_all_prior()`. Both were dropped: `fn`-item identity is fragile (duplicates, closures, generics), and `.after_all_prior()` is registration-order-coupled — the exact thing B2 forbids. A linear `.chain()` was weighed and rejected too, because real dependencies form a *partial* order (a diamond: `files → {meshes ∥ textures} → upload`) that a chain can't express without falsely serialising the independent branches. Handle/label `.after`/`.before` expresses the DAG directly. See #34 for the full decision trail.

Why workloads on top of stages? Stages define the broad frame structure; workloads let modules group their related systems with a meaningful name. A plugin can add a self-contained workload — defining its *own* `WorkloadLabel` enum, with no central list to edit — without worrying about exact system order in some giant frame-wide list.

### Stage — the frame shape

```rust
/// The fixed per-frame phases, in execution order. A closed, exhaustive
/// enum: order is intrinsic (declaration order), `match` is exhaustive,
/// and a typo is a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stage {
    Startup,        // once, before main loop begins
    First,          // very first thing each frame
    PreUpdate,      // input poll, time tick
    FixedUpdate,    // runs N times per frame based on accumulator (60 Hz)
    Update,         // main game logic at display rate
    PostUpdate,     // cleanup
    Render,         // build draw lists, submit GPU work
    Last,           // very last thing each frame
}
```

Per-frame order: `First → PreUpdate → (FixedUpdate × N) → Update → PostUpdate → Render → Last`.

Inside a stage, workloads run in the order their explicit `.after`/`.before` constraints (by label) dictate. Within a workload, systems run in the order their explicit `.after`/`.before` constraints (by handle) dictate; an undeclared order between two conflicting systems is a registration error, not an implicit choice.

Commands flush between every workload. Events double-buffer between `Last` (frame N) and `First` (frame N+1).

**Why a closed enum, and why `Stage` not `Schedule`.** There is exactly one frame timeline, so there is one shared set of phases. An enum makes that set typo-proof and exhaustively `match`-able, and encodes the order in its variant declaration order — no runtime ordering structure needed. The name `Stage` keeps `Schedule` free for a possible future runnable system-graph container, and avoids a four-way clash with the `Scheduler` / `StageData` machinery.

**How a subsystem extends it.** Not by adding a `Stage` variant — variants of different enums have no defined order, and the frame is a single timeline. A subsystem orders *its own* work with **workloads** (it defines its own `WorkloadLabel` enum, see above) that live inside the shared stages. The rare case of a genuinely new *global* phase is a deferred, **non-breaking** upgrade: widen `add_system` / `add_workload` to take `impl StageLabel`, add `impl StageLabel for Stage {}`, and every existing `Stage::Update` call-site still compiles. `StageLabel` would then carry identity exactly like `WorkloadLabel` — a trait with `#[derive(StageLabel)]`.

**Where `Stage` lives.** In `spark-core`, not `spark-ecs` — the concrete frame phases belong to the *app/frame layer*, the same split Bevy draws between `bevy_app` (which owns the `Update` / `Startup` / `FixedUpdate` labels) and `bevy_ecs` (which owns the generic schedule machinery). `spark-core` already owns `Application` and `run_stage`, so the enum sits beside its only consumer. Because `spark-ecs` sits *below* `spark-core` in the dependency graph it can't name `Stage` directly; the future `spark-ecs` scheduler will accept it through the `StageLabel` trait above (`impl StageLabel for Stage`), which keeps the graph cycle-free. Shipped as `spark-core::Stage` in #32, with the call-site migration riding along.

### Plugin

A plugin registers everything a module owns: components, resources, events, systems, workloads.

```rust
pub struct PowerGridPlugin;

impl Plugin for PowerGridPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PowerNetwork>()
           .add_event::<BlackoutStarted>()
           .add_event::<BlackoutEnded>();

        // `add_workload` returns an ordering builder, so it is its own statement.
        app.add_workload(Workload::PowerGrid, Stage::FixedUpdate, |w| {
            let supply = w.add_system(collect_supply);
            let demand = w.add_system(compute_demand);
            let distribute = w.add_system(distribute_power).after(supply).after(demand);
            w.add_system(emit_blackout_events).after(distribute);
        })
        .after(Workload::Simulation);
    }
}
```

### App

```rust
fn main() {
    App::new()
        // Engine plugins
        .add_plugin(CorePlugin)
        .add_plugin(WindowPlugin { title: "Spark", size: (1280, 720) })
        .add_plugin(InputPlugin)
        .add_plugin(RenderPlugin)
        .add_plugin(AssetsPlugin)
        // Game plugins
        .add_plugin(WorldPlugin)
        .add_plugin(PowerGridPlugin)
        .add_plugin(CityPlugin)
        .add_plugin(WorkerPlugin)
        .add_plugin(PlansPlugin)
        .add_plugin(EconomyPlugin)
        .add_plugin(UiPlugin)
        // Run the main loop
        .run();
}
```

`App::run()` consumes the app, hands control to the window plugin's event loop, which calls `app.frame()` per redraw. Each `frame()` runs the stages in order.

## Derive macros

Four derive/attribute macros, all living in a nested proc-macro crate `spark-ecs-macros` at `lib/ecs/macros/`. The crate is nested (not a top-level `lib/*` sibling) so it stays organizationally owned by `spark-ecs`; consumers depend on and import only `spark_ecs`, which re-exports the derives:

| Macro | What it does |
|-------|--------------|
| `#[derive(Component)]` | Registers the type in `ComponentRegistry` for introspection |
| `#[derive(Resource)]` | Same for resources |
| `#[derive(Event)]` | Same for events; also implements `Event` marker trait |
| `#[derive(WorkloadLabel)]` | Matches over an enum's variants to generate per-variant label identity + name |
| `#[derive(Trace)]` | Opt-in: emits `ChangeEvent` on every mutation of this component |
| `#[system]` (attribute) | Captures `file:line` + module path metadata for system introspection (optional convenience) |

The derives are not required for the type to work — manual registration via `app.register_component::<T>()` is always available. But the derives are the ergonomic default and what every example uses.

## Traceability layer

Traceability is built into every system, workload, and command flow by default.

### System metadata (registration-time)

```rust
pub struct SystemMeta {
    pub name: &'static str,            // function name
    pub module: &'static str,          // module path
    pub file: &'static str,            // source file
    pub line: u32,                     // source line
    pub reads: &'static [TypeId],      // declared read set
    pub writes: &'static [TypeId],     // declared write set
    pub workload: WorkloadId,
    pub stage: Stage,
}
```

Populated automatically by `IntoSystem` impls (using `std::any::type_name`, `module_path!()`, `file!()`, `line!()`). The `#[system]` attribute macro can override or enrich this.

### Runtime FrameTrace resource

```rust
#[derive(Resource, Default)]
pub struct FrameTrace {
    pub frame: u64,
    pub workloads: Vec<WorkloadTrace>,
}

pub struct WorkloadTrace {
    pub label: WorkloadId,
    pub stage: Stage,
    pub started_at: Instant,
    pub duration: Duration,
    pub systems: Vec<SystemTrace>,
    pub commands_flushed: usize,
}

pub struct SystemTrace {
    pub system: SystemId,
    pub started_at: Instant,
    pub duration: Duration,
    pub entities_read: usize,
    pub entities_mutated: usize,
    pub commands_queued: usize,
    pub events_sent: HashMap<EventTypeId, usize>,
}
```

`FrameTrace` is overwritten each frame. The editor reads it. `tracing` crate spans wrap every system call, giving free integration with `tracing-subscriber`, `tracy`, flamegraphs, etc.

### Component change log (opt-in)

```rust
#[derive(Component, Trace)]   // <-- opt-in
pub struct PowerNetwork { ... }

// Editor reads from a ChangeLog resource:
let log = world.resource::<ChangeLog>();
for ev in log.read::<PowerNetwork>(last_frame) {
    println!("PowerNetwork mutated by system {:?} at frame {}", ev.system, ev.frame);
}
```

`#[derive(Trace)]` wraps mutable access in a smart pointer that records who wrote and when. Off by default; on for components you want to debug.

### Command log

Every `Commands::spawn`, `despawn`, `insert`, `remove` records a row in a frame-scoped `CommandLog` resource. The editor's history panel reads it directly.

## Editor introspection — reflection APIs

The editor (built as another plugin running in-process) gets these world-level introspection methods:

```rust
// All entities currently alive
for entity in world.entities().iter() { ... }

// All components on a single entity (name + debug string)
for inspect in world.inspect_entity(entity) {
    println!("{}: {}", inspect.component_name, inspect.debug);
}

// All resources
for inspect in world.inspect_resources() {
    println!("{}: {}", inspect.resource_name, inspect.debug);
}

// System graph for the current frame
let graph = world.resource::<FrameTrace>().system_graph();

// Live timings
for sys in &world.resource::<FrameTrace>().workloads[0].systems {
    println!("{:?}: {:?}", sys.system, sys.duration);
}
```

Zero-cost when the editor isn't attached — registration-time metadata only; the `FrameTrace` resource is updated regardless because tracing is cheap, but no one reads it. The editor is a separate plugin (`EditorPlugin`) that adds an egui overlay reading these APIs.

## Parallel execution model (committed for M4)

Each system declares its access set via the `SystemParam::access()` method. The scheduler within a workload:

1. Builds a DAG from the declared `.after`/`.before` edges. An access conflict (read-write or write-write on the same `TypeId`) keeps two systems out of the same batch, but it does **not** silently add an ordering edge — two conflicting systems with no declared order are a registration error (decision (a)) unless acknowledged with `.any_order_with`.
2. Topologically batches: systems with no remaining dependencies and no current-batch conflicts run in parallel via Rayon.
3. Commands flush between workloads (single-threaded, deterministic).

```text
Workload::Simulation:
  worker_ai        reads Worker, writes JobBoard
  plant_operation  reads Plant, writes PowerNetwork
  construction     writes ConstructionProgress, reads Worker

Conflict edges:
  worker_ai ↔ construction   (both touch Worker — read vs write)
  plant_operation: no conflicts → runs in parallel with one of them

Batches:
  Batch 1: [plant_operation, worker_ai]   (parallel)
  Batch 2: [construction]                  (sequential after worker_ai)
```

Determinism is preserved: parallel systems target disjoint data, so iteration order within a batch doesn't affect output.

Phase 1 runs everything sequentially within a workload to keep the scheduler simple while we're learning. The parallel executor is the M4 deliverable (Stage 19) and is committed, not optional — Spark's simulation requires it. The `Access` model and DAG/batch structure are built from Phase 1 so M4 is a `RefCell → UnsafeCell` swap behind an already-correct scheduler.

## Internal implementation sketch

Key traits the ECS hangs on. Real implementations are deferred to the build plan; this is to communicate the shape.

### `SystemParam` — the heart of function-parameter extraction

```rust
pub trait SystemParam {
    type State: 'static + Send + Sync;   // cached lookup state between calls
    type Item<'w>;                       // what the function actually receives

    fn init(world: &mut World) -> Self::State;
    fn access() -> Access;               // read/write set for this param
    fn get<'w>(state: &'w mut Self::State, world: &'w UnsafeWorldCell) -> Self::Item<'w>;
}

// Implemented for:
impl<T: Resource> SystemParam for Res<T> { ... }
impl<T: Resource> SystemParam for ResMut<T> { ... }
impl<D: QueryData, F: QueryFilter> SystemParam for Query<'_, '_, D, F> { ... }
impl SystemParam for Commands<'_, '_> { ... }
impl<E: Event> SystemParam for EventReader<'_, '_, E> { ... }
impl<E: Event> SystemParam for EventWriter<'_, E> { ... }
impl<T: Default + 'static> SystemParam for Local<'_, T> { ... }

// Tuples up to arity 16
impl<P1: SystemParam, P2: SystemParam> SystemParam for (P1, P2) { ... }
// ... via macro
```

### `IntoSystem` — turn a function into a `Box<dyn System>`

```rust
pub trait IntoSystem<Params> {
    fn into_system(self) -> Box<dyn System>;
}

// Generated for each arity via macro:
impl<F, P1, P2> IntoSystem<(P1, P2)> for F
where
    P1: SystemParam,
    P2: SystemParam,
    F: FnMut(P1::Item<'_>, P2::Item<'_>) + 'static
       + for<'w> FnMut(P1::Item<'w>, P2::Item<'w>),
{
    fn into_system(mut self) -> Box<dyn System> {
        Box::new(FunctionSystem {
            func: self,
            state: None,
            meta: SystemMeta {
                name: type_name_of::<F>(),
                reads: union(P1::access().reads, P2::access().reads),
                writes: union(P1::access().writes, P2::access().writes),
                // ...
            },
        })
    }
}
```

### `ComponentStorage<T>` — sparse set

```rust
pub struct ComponentStorage<T> {
    sparse: Vec<Option<u32>>,    // sparse[entity.index] -> dense index
    dense: Vec<T>,                // packed component data
    entity_index: Vec<Entity>,    // entity_index[dense_idx] -> Entity
}
```

O(1) insert/remove/get. Linear iteration over `dense`. Cache-friendly enough for Spark's scale. Archetype refactor is stretch.

### `World`

```rust
pub struct World {
    entities: EntityAllocator,
    components: HashMap<TypeId, Box<dyn AnyStorage>>,
    resources: HashMap<TypeId, Box<dyn AnyResource>>,
    events: HashMap<TypeId, Box<dyn AnyEvents>>,
    change_log: ChangeLog,
}
```

Heterogeneous storages stored as trait objects, downcast via `TypeId`. `RefCell` inside each storage for runtime borrow checking until we move to parallel execution.

### `Scheduler`

```rust
pub struct Scheduler {
    stages: HashMap<Stage, StageData>,
}

pub struct StageData {
    workloads: Vec<WorkloadData>,
    workload_order: Vec<WorkloadId>,   // topo-sorted by .after/.before (by label)
}

pub struct WorkloadData {
    label: WorkloadId,
    systems: Vec<Box<dyn System>>,
    system_order: Vec<SystemId>,        // topo-sorted within workload
}
```

Phase 1: `Scheduler::run(world)` walks stages → workloads → systems sequentially. M4 (Stage 19): builds parallel batches within a workload via Rayon, using the per-system access set as the disjointness proof.

## Crate structure

No `mod.rs` files — modern Rust convention uses `foo.rs` as the module file
next to a `foo/` directory of submodules.

```
lib/
└── ecs/
    ├── Cargo.toml
    ├── src/
    │   ├── lib.rs              # re-exports (incl. spark_ecs_macros::{Component, Resource, …})
    │   ├── entity.rs           # Entity, EntityAllocator
    │   ├── storage.rs          # ComponentStorage<T>, AnyStorage
    │   ├── world.rs            # World
    │   ├── resource.rs         # Resource access, AnyResource
    │   ├── query.rs            # parent — Query<'_, D, F> + re-exports
    │   ├── query/
    │   │   ├── data.rs         # QueryData trait, tuple impls
    │   │   ├── filter.rs       # With, Without (Or deferred)
    │   │   └── iter.rs         # Query iteration
    │   ├── system.rs           # parent — System trait + re-exports
    │   ├── system/
    │   │   ├── param.rs        # SystemParam trait, all impls
    │   │   └── function.rs     # IntoSystem, FunctionSystem
    │   ├── stage.rs            # StageData + StageLabel trait (the concrete Stage enum lives in spark-core)
    │   ├── workload.rs         # Workload labels, WorkloadData, builder
    │   ├── scheduler.rs        # Scheduler — runs stages
    │   ├── commands.rs         # Commands, CommandQueue
    │   ├── events.rs           # Events<T>, EventReader, EventWriter
    │   ├── plugin.rs           # Plugin trait, App
    │   ├── trace.rs            # FrameTrace, SystemTrace, ChangeLog
    │   ├── reflect.rs          # Reflection APIs for the editor
    │   └── access.rs           # Access set, conflict detection
    ├── tests/
    │   ├── entity.rs
    │   ├── storage.rs
    │   ├── query.rs
    │   ├── system.rs
    │   ├── workload.rs
    │   ├── commands.rs
    │   └── events.rs
    └── macros/                 # nested proc-macro crate (spark-ecs-macros)
        ├── Cargo.toml          # proc-macro = true; deps: syn, quote, proc-macro2
        └── src/
            ├── lib.rs
            ├── component.rs    # #[derive(Component)]
            ├── resource.rs     # #[derive(Resource)]
            ├── event.rs        # #[derive(Event)]
            ├── workload_label.rs # #[derive(WorkloadLabel)]
            ├── trace.rs        # #[derive(Trace)]
            └── system.rs       # #[system] attribute
```

The nested `macros/` crate is a single mandatory `members = ["lib/ecs/macros", …]` entry in the workspace `Cargo.toml`. From every consumer's perspective the ECS is one cohesive package — they depend only on `spark-ecs`, which re-exports the derives.

## Cargo.toml

```toml
# lib/ecs/Cargo.toml
[package]
name = "spark-ecs"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
# spark-ecs is the deepest engine crate — no spark-core dep, no engine deps.
# The proc-macro crate is nested at `macros/` and is the only dependency
# besides workspace stdlib-adjacent helpers added as we land features.
spark-ecs-macros = { path = "macros" }
tracing.workspace = true

# No external ECS dependency — everything written from scratch.
```

```toml
# lib/ecs/macros/Cargo.toml — nested proc-macro crate
[package]
name = "spark-ecs-macros"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[lib]
proc-macro = true

# Add these to [workspace.dependencies] in the root Cargo.toml as exact
# pinned versions (project convention — never caret ranges). Look up the
# latest stable at implementation time.
[dependencies]
syn = { workspace = true, features = ["full"] }
quote.workspace = true
proc-macro2.workspace = true
```

## Build plan — phases and stages

Each stage ends with passing tests. We don't move on until green.

### Phase 1 — Core ECS (stages 1–14)

This is the minimum to ship Spark.

**Stage 1 — Entity + EntityAllocator** (½ day)
Generational IDs, allocate/destroy/recycle with generation bump, `is_alive`.
Test: 1M create/destroy cycles preserve invariants.

**Stage 2 — `ComponentStorage<T>` (sparse set)** (1 day)
Generic over `T: 'static + Send + Sync`. `insert`, `remove`, `get`, `get_mut`, `iter`, `iter_mut`, `len`, `contains`.
Test: insert 10k, swap-remove half randomly, iter remaining, verify count.

**Stage 3 — `World` with heterogeneous storages** (1–2 days)
`HashMap<TypeId, Box<dyn AnyStorage>>`, `AnyStorage` trait, `spawn`, `despawn` cascade through all storages, `insert::<T>`, `remove::<T>`, `get::<T>`, `get_mut::<T>`.
Test: two component types, despawn cascades correctly.

**Stage 4 — Resources** (½ day)
`world.insert_resource(r)`, `resource::<T>()`, `resource_mut::<T>()`, `remove_resource::<T>()`.
Test: insert/read/update/remove cycle.

**Stage 5 — Queries (no filters)** (2–3 days)
`Query<&T>`, `Query<&mut T>`, `Query<(&A, &B)>`, `Query<(&mut A, &B)>` for arity 1–4. Driver-storage selection for joins (pick smallest).
Test: movement system across 1k entities.

**Stage 6 — Query filters** (1 day)
`With<T>`, `Without<T>`, tuple-of-filters, `Option<&T>` in data. `Or<(F1, F2)>` is **deferred to post-M4** — `With`/`Without` cover the day-1 needs, and `Or` requires extra `Access`-model handling that is easier to add once the parallel scheduler is in place.
Test: filter combinations yield correct sets.

**Stage 7 — Derive macros: `Component`, `Resource`, `Event`** (1–2 days)
Nested `lib/ecs/macros/` proc-macro crate (`spark-ecs-macros`). `#[derive(Component)]` registers in `ComponentRegistry`, captures name/debug fn. Same for the others. `spark-ecs` re-exports the derives so consumers depend only on `spark-ecs`.
Test: derived types show up in registry; can be enumerated.

**Stage 8 — `SystemParam` for `Res<T>` and `ResMut<T>`** (1–2 days)
Trait definition, impls for `Res`/`ResMut`. Access set computation.
Test: standalone `Res<T>` extraction works against a world.

**Stage 9 — `SystemParam` for `Query<D, F>`** (2–3 days)
Implement the trait for queries. Cache lookup state across calls.
Test: extract a query from a world via the param trait.

**Stage 10 — `IntoSystem` + tuple macro** (2–3 days)
`IntoSystem` trait, tuple impls for arity 0–8 via macro. `FunctionSystem` wrapper. `Box<dyn System>` boxing.
Test: write a regular function with two params, register it, invoke via the system box.

**Stage 11 — `Stage` enum + sequential scheduler** (1 day)
`Stage::Startup`, `First`, `PreUpdate`, `FixedUpdate`, `Update`, `PostUpdate`, `Render`, `Last`. Scheduler walks them in order, runs systems sequentially within each. The **`Stage` enum + call-site migration shipped with #32**: the enum lives in `spark-core` (replacing the M1–M3 `pub mod stages { pub const STARTUP: &str = "startup"; … }` stand-in), and `add_system(stages::FOO, …)` call-sites moved to `add_system(Stage::Foo, …)` — the method keeps its name (a plural `add_systems` was deferred then; resolved 2026-05-22/05-23 — the plural *unordered* form lives on the workload builder as `w.add_systems((..))`, while `App::add_system` stays the singular **sequential** registrar — see #41). The sequential scheduler itself is still pending (scheduler epic children 2–6). See roadmap item 3.
Test: three systems in three different stages run in correct order.

**Stage 12 — `Workload` + builder** (2 days)
`WorkloadLabel` trait + `#[derive(WorkloadLabel)]` (matches over an enum's variants to generate per-variant identity + name). Workload builder closure: `app.add_workload(label, stage, |w| { ... })`. Ordering is one verb pair at both levels: `w.add_system(sys)` returns a handle directly (no `.id()`) — then `.after(handle)` / `.before(handle)` for systems; `app.add_workload(...).after(label)` / `.before(label)` for workloads (lazy label resolution at build). An undeclared conflict is a registration error (decision (a)); `.any_order_with(handle | label)` is the escape hatch. No `.chain()` / `.after_all_prior()`. Topo-sort within *and* across workloads.
Test: ordering respected (incl. a diamond); undeclared conflict errors and `.any_order_with` silences it; cycle detected with the pinned message; a forward-referenced label resolves at build. See #34.

**Stage 13 — `Commands` + `CommandQueue`** (2 days)
`Commands::spawn`, `despawn`, `entity(e).insert().remove()`, `insert_resource`, `update_resource`. Queue flush between workloads.
Test: spawn inside a system → entity visible to next workload.

**Stage 14 — `Events<T>` + readers/writers + `Plugin`/`App` + `FixedUpdate`** (2 days)
`Events<T>` resource (double-buffered ring). `EventReader<T>` (cursor per system), `EventWriter<T>`. `Plugin` trait. `App::add_plugin`, `add_system`, `add_workload`, `init_resource`, `add_event`, `run`. `FixedUpdate` accumulator.
Test: end-to-end app with 1 plugin, 1 workload, 1 event round-trip; 100 ms simulated time = 6 fixed updates at 60 Hz.

**After Phase 1: the ECS is feature-complete for shipping Spark.**

### Phase 2 — Traceability & editor (stages 15–18)

**Stage 15 — `FrameTrace` resource + `tracing` spans** (1 day)
Wrap every system call in a `tracing::info_span!`. Record `SystemTrace` per system per workload. Update `FrameTrace` resource each frame.
Test: read `FrameTrace` after one frame; counts and durations populated.

**Stage 16 — `#[derive(Trace)]` + `ChangeLog`** (2–3 days)
Derive macro wraps `&mut T` access in a tracking smart pointer. `ChangeLog` resource holds events per `TypeId`. Frame-scoped retention.
Test: mutate a `#[derive(Trace)]` component; verify event appears in log with correct system attribution.

**Stage 17 — Reflection APIs** (1–2 days)
`world.entities()`, `world.inspect_entity(e)`, `world.inspect_resources()`. Component/resource registries expose `name`, debug-fmt fn, optional serde.
Test: walk all entities and dump their components via registry; assert structure.

**Stage 18 — `CommandLog` resource** (1 day)
Per-frame log of every command applied. Editor history panel reads from it.
Test: spawn/insert/remove inside a system; verify entries in log.

**After Phase 2: editor can introspect everything live.** The actual `EditorPlugin` (egui overlay) lives in `lib/editor/`, not in `lib/ecs/`, but consumes the APIs above.

### Phase 3 — Performance & polish (stages 19+)

Stage 19 (parallel execution) is **committed for M4**, not stretch. The remaining stages in this phase are optional.

**Stage 19 — Parallel system execution** *(M4 — committed, not stretch)*
Track access sets per system. Within a workload, batch non-conflicting systems and run via Rayon. Determinism preserved by disjoint-data invariant. The `Component`/`Resource` `Send + Sync + 'static` bound is the safety proof; `Access` declarations are the disjointness proof; conflicts are caught at registration time.
Estimated: 3–5 days.

**Stage 20 — `Local<T>` per-system state**
Per-system state that persists between calls. Cached inside `FunctionSystem`.
Estimated: ½ day.

**Stage 21 — Run conditions**
`.run_if(condition_fn)` on systems and workloads. Useful for "only run during gameplay, not in menu".
Estimated: 1 day.

**Stage 22 — `Changed<T>` / `Added<T>` query filters**
Builds on `#[derive(Trace)]` infrastructure. Lets systems iterate only modified entities.
Estimated: 2 days.

**Stage 23 — `#[derive(Bundle)]` for named bundles**
Sugar over tuples: `#[derive(Bundle)] struct WorkerBundle { pos: Position, vel: Velocity, ... }` then `cmd.spawn(worker_bundle)`.
Estimated: 1 day.

**Stage 24 — Archetype storage refactor**
Replace sparse-set internals with archetype tables. Public API unchanged. Benchmark improvement.
Estimated: 1–2 weeks.

## Worked example 1 — orange-style demo (movable sprite + camera)

Smallest end-to-end demo of the whole pipeline: window opens, sprite shows, WASD moves it, camera follows.

### Components

```rust
// lib/core/src/types.rs
#[derive(Component, Debug, Clone, Copy)]
pub struct Position(pub Vec2);

#[derive(Component, Debug, Clone, Copy)]
pub struct Velocity(pub Vec2);

#[derive(Component, Debug, Clone)]
pub struct Sprite {
    pub texture: TextureHandle,
    pub size: Vec2,
    pub tint: Color,
}

// game/src/main.rs
#[derive(Component)] pub struct PlayerControlled;
#[derive(Component)] pub struct CameraTarget;
```

### Resource

```rust
#[derive(Resource, Default)]
pub struct Camera {
    pub position: Vec2,
    pub zoom: f32,
}
```

### Systems

```rust
pub fn spawn_player(mut cmd: Commands, assets: Res<AssetServer>) {
    let texture = assets.load_texture("player.png");
    cmd.spawn((
        Position(Vec2::ZERO),
        Velocity(Vec2::ZERO),
        Sprite { texture, size: vec2(32.0, 32.0), tint: Color::WHITE },
        PlayerControlled,
        CameraTarget,
    ));
}

pub fn read_player_input(
    input: Res<InputState>,
    mut q: Query<&mut Velocity, With<PlayerControlled>>,
) {
    let mut dir = Vec2::ZERO;
    if input.held(Key::W) { dir.y += 1.0; }
    if input.held(Key::S) { dir.y -= 1.0; }
    if input.held(Key::A) { dir.x -= 1.0; }
    if input.held(Key::D) { dir.x += 1.0; }
    for mut vel in q.iter_mut() {
        vel.0 = dir.normalize_or_zero() * 200.0;
    }
}

pub fn integrate_motion(
    time: Res<Time>,
    mut q: Query<(&mut Position, &Velocity)>,
) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0 * time.delta;
    }
}

pub fn camera_follow(
    mut camera: ResMut<Camera>,
    targets: Query<&Position, With<CameraTarget>>,
) {
    if let Some(target) = targets.iter().next() {
        camera.position = camera.position.lerp(target.0, 0.1);
    }
}
```

### Plugin

```rust
pub struct DemoPlugin;

impl Plugin for DemoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Camera>()
           .add_system(Stage::Startup, spawn_player);

        app.add_workload(Workload::PlayerInput, Stage::PreUpdate, |w| {
            w.add_system(read_player_input);
        })
        .after(Workload::Input);

        app.add_workload(Workload::PlayerMotion, Stage::Update, |w| {
            let motion = w.add_system(integrate_motion);
            w.add_system(camera_follow).after(motion);
        });
    }
}
```

### main.rs

```rust
fn main() {
    App::new()
        .add_plugin(CorePlugin)
        .add_plugin(WindowPlugin { title: "Spark Demo", size: (1280, 720) })
        .add_plugin(InputPlugin)
        .add_plugin(RenderPlugin)
        .add_plugin(AssetsPlugin)
        .add_plugin(DemoPlugin)
        .run();
}
```

A complete, runnable game showing the full ECS round-trip: input → motion → camera → render. ~80 lines of game-side code.

## Worked example 2 — first Spark loop (plant → city)

Building on the same primitives.

### Components + Resources

```rust
#[derive(Component, Debug)]
pub struct Plant {
    pub kind: PlantKind,
    pub output_mw: f32,
}

#[derive(Component)] pub struct Operational;
#[derive(Component)] pub struct UnderMaintenance;

#[derive(Component, Debug)]
pub struct City {
    pub population: u32,
    pub demand_mw: f32,
    pub supply_mw: f32,
}

#[derive(Resource, Default)]
pub struct PowerNetwork {
    pub supply: f32,
    pub demand: f32,
    pub ratio: f32,
}

#[derive(Event)]
pub struct CityTierUp {
    pub city: Entity,
    pub new_tier: u32,
}
```

### Systems

```rust
pub fn collect_supply(
    plants: Query<&Plant, With<Operational>>,
    mut grid: ResMut<PowerNetwork>,
) {
    grid.supply = plants.iter().map(|p| p.output_mw).sum();
}

pub fn compute_demand(
    mut cities: Query<&mut City>,
    mut grid: ResMut<PowerNetwork>,
) {
    let mut total = 0.0;
    for mut city in cities.iter_mut() {
        city.demand_mw = city.population as f32 * 0.001;
        total += city.demand_mw;
    }
    grid.demand = total;
    grid.ratio = if total > 0.0 { (grid.supply / total).min(1.0) } else { 1.0 };
}

pub fn distribute_power(
    grid: Res<PowerNetwork>,
    mut cities: Query<&mut City>,
) {
    for mut city in cities.iter_mut() {
        city.supply_mw = city.demand_mw * grid.ratio;
    }
}

pub fn city_growth(
    time: Res<Time>,
    mut cities: Query<(Entity, &mut City)>,
    mut events: EventWriter<CityTierUp>,
) {
    for (entity, mut city) in cities.iter_mut() {
        let met = if city.demand_mw > 0.0 { city.supply_mw / city.demand_mw } else { 1.0 };
        if met > 0.95 {
            city.population += (time.fixed_delta * 2.0) as u32;
            if city.population >= 1000 {
                events.write(CityTierUp { city: entity, new_tier: 2 });
            }
        } else if met < 0.5 {
            city.population = city.population.saturating_sub((time.fixed_delta * 1.0) as u32);
        }
    }
}
```

### Plugin

```rust
pub struct PowerCityPlugin;

impl Plugin for PowerCityPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PowerNetwork>()
           .add_event::<CityTierUp>();

        app.add_workload(Workload::PowerGrid, Stage::FixedUpdate, |w| {
            let supply = w.add_system(collect_supply);
            let demand = w.add_system(compute_demand).after(supply);
            w.add_system(distribute_power).after(demand);
        });

        app.add_workload(Workload::CityTick, Stage::FixedUpdate, |w| {
            w.add_system(city_growth);
        })
        .after(Workload::PowerGrid);
    }
}
```

Fully working Spark minimum-loop. Each system is small and focused; access sets are declared by the parameter types; the M4 parallel executor will batch non-conflicting systems automatically.

## Open questions

Parked for future decisions:

- **Bundle derive** — do we want `#[derive(Bundle)]` for named bundles like `WorkerBundle { pos, vel, sprite }`? Tuple-spawn `cmd.spawn((Position, Velocity, Sprite))` works without it. Convenient if many systems spawn the same shape.
- **Run conditions** (`.run_if(...)`) — phase 3, decide when we hit game states (main menu vs gameplay).
- **System sets** (Bevy's `SystemSet`) — we have workloads, which is similar but heavier. Decide if a lighter "label set" abstraction also has value.
- **Hot-reload** — if `EditorPlugin` becomes capable, do we want hot-reload of systems via `libloading`? Likely out of scope for v1.
- **Save/load** — separate concern; design once core ECS is stable. Will piggy-back on the reflection APIs from stage 17.
