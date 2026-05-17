# spark-ecs

The Spark engine's custom Entity-Component-System (ECS). The deepest
crate in the workspace: pure stdlib, no engine dependencies — every
other engine crate, including `spark-core`, sits on top.

> **Today vs tomorrow.** This README documents the **whole** ECS, not
> just what's implemented at M1. Sections marked **today** describe
> the M1 surface that compiles and runs right now; sections marked
> **future** describe the architecture being built toward M3 and M4.
> Future-architecture code blocks use `rust,ignore` because the types
> they reference (`Entity`, `Component`, `Query`, `Commands`, …) don't
> exist yet — they're the spec for what's coming.
>
> The full build plan and design rationale lives in
> [`docs/ECS_DESIGN.md`](../../docs/ECS_DESIGN.md). This README is the
> friendly tour; that doc is the engineering reference.

## Why an ECS?

Suppose you're writing a power-grid simulator. Some objects are power
plants that produce energy. Some are cities that consume energy. Some
are workers walking around to repair things. Some are plans the player
has placed but nothing has been built there yet.

In **object-oriented** code, you'd reach for inheritance:

```text
            GameObject
              │
   ┌──────────┼──────────┐
   ▼          ▼          ▼
 Plant       City      Worker
   │
   ▼
WaterWheel, CoalPlant, …
```

That works for a while. Then someone wants a *plant under
construction* — it has a `ConstructionProgress`, isn't producing
energy yet, but should still appear on the map. Where does that
component go? On `Plant`? But then you have to remember to check
`is_built` everywhere. On a new `PlantUnderConstruction`? Then the
construction system has two cases. On a wrapper? Now you've got
delegation hell.

In an **ECS**, you stop asking "what is this thing" and start asking
"what does this thing have". A "plant under construction" is just an
entity that has:

```text
    Position, BuildingKind, ConstructionProgress
```

Once construction finishes, a system removes
`ConstructionProgress` and adds `Operational`. The entity didn't
"become" a different class — its component set changed.

That's the ECS bet: every system reads the components it cares about
and ignores the rest. Adding a new feature usually means adding a new
component and a new system, never editing existing systems. The
update graph stays flat and explicit.

> **What about "Components in Unity"?** That's the same idea applied
> half-way: each `GameObject` is a heap-allocated container holding a
> list of components. ECS goes further — entities are *just an ID*,
> components live in tightly-packed arrays keyed by that ID, and
> systems iterate the arrays directly. Same ergonomics, much better
> data layout for cache and parallelism.

## The three vocabulary words

```text
   ┌──────────────────────────────────────────────────────────────┐
   │                                                              │
   │   Entity        an ID — `(index, generation)`                │
   │     ↓                                                        │
   │   Component     a piece of data attached to an Entity        │
   │     ↑                                                        │
   │   System        a function that reads / writes Components    │
   │                                                              │
   └──────────────────────────────────────────────────────────────┘
```

**Entity** — a 64-bit ID (`index: u32, generation: u32`). Nothing
else. The whole "object" exists only as the set of components keyed
by this ID across many storages.

**Component** — a plain Rust struct (`Position`, `Velocity`, `Plant`,
`PlayerControlled`). Marker components (zero-sized like
`PlayerControlled`) are fine. Components live in *per-type storages*
inside the [`World`].

**System** — a Rust function. Its parameters describe what it reads
and writes; the engine wires up the access for you:

```rust,ignore
fn integrate(time: Res<Time>, mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0 * time.delta;
    }
}
```

This declarative shape — *the function signature is the interface* —
is the trick that makes ECS code stay readable as the game grows.

## Today's surface (M1)

What ships **today** is the resource layer plus the system-parameter
machinery that future entity/query work plugs into.

### `World`: where state lives

The [`World`] is a type-erased container that owns one value per
type. Today it only stores *resources* (singletons, one per type);
component storage and the entity allocator land in M3.

```rust
use spark_ecs::World;

struct GameTime { dt: f32, elapsed: f32 }
struct Score(u32);

let mut world = World::new();
world.add_resource(GameTime { dt: 0.016, elapsed: 0.0 });
world.add_resource(Score(0));

assert_eq!(world.resource::<Score>().0, 0);
world.resource_mut::<Score>().0 = 42;
assert_eq!(world.resource::<Score>().0, 42);
```

Resources can be any `T: 'static` for now. When the
`#[derive(Resource)]` macro lands in M3 it'll tighten the bound to
`Send + Sync + 'static`, which the M4 parallel scheduler needs.

### `Res<T>` / `ResMut<T>`: resource accessors

A system asks for resources by parameter type:

```rust
use spark_ecs::{IntoSystem, Res, ResMut, World};

struct GameTime { dt: f32 }
struct Score(u32);

fn tick_score(time: Res<GameTime>, mut score: ResMut<Score>) {
    // Borrow-check is at runtime via RefCell — two `ResMut`s over the
    // *same* T would panic; two `ResMut`s over *different* Ts (like
    // here) coexist fine.
    score.0 += (time.dt * 1000.0) as u32;
}

let mut world = World::new();
world.add_resource(GameTime { dt: 0.016 });
world.add_resource(Score(0));

let mut system = IntoSystem::into_system(tick_score);
system(&world);
system(&world);
// 0.016 * 1000 = 16, truncated to 16. Called twice → 32.
assert_eq!(world.resource::<Score>().0, 32);
```

`Res<T>` derefs to `&T`, `ResMut<T>` to `&mut T`.

### `IntoSystem`: turning a fn into a runnable system

[`IntoSystem`] is what `spark_core::Application::add_system` calls
under the hood. It takes a function whose parameters are all
[`SystemParam`] (today: `Res<T>` and `ResMut<T>`, for arities 0..=4)
and wraps it as a uniform `Box<dyn FnMut(&World)>` the engine can
store next to systems of other shapes.

You usually don't call `IntoSystem::into_system` yourself — `spark-core`
does it when you write `app.add_system(stage, my_fn)`. This is the
*Bevy-style function-parameter pattern*: the function's signature
*is* the spec of what the engine needs to inject. Read more in
[`spark-core`'s README](../core/README.md).

## Tomorrow's architecture (M3 onwards)

Here's how the rest of the ECS will be built. Every code block below
is `rust,ignore` because the types don't exist yet — read these as
"here's how the API will look when M3 lands". The
[`docs/ECS_DESIGN.md`](../../docs/ECS_DESIGN.md) doc is the engineering
spec; this section is the friendly tour with memory diagrams.

### Entities: the generational ID

An entity is just two `u32`s — an index and a generation:

```rust,ignore
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Entity {
    index: u32,
    generation: u32,
}
```

Why two numbers? Because spawning and despawning recycle slots. If
slot 12 is despawned and later reused for a different entity, any
*old* `Entity` handle that still points at index 12 must be detectable
as stale — that's what the generation number does.

```text
EntityAllocator state after spawning 6 entities and despawning #3:

   generation:  [0, 0, 0, 1, 0, 0]
                       ↑
              bumped when slot 3 was destroyed

   free_list:   [3]
                 ↑
             ready for reuse

Spawning a 7th entity pops `3` off the free list and returns
`Entity { index: 3, generation: 1 }`. The old handle
`Entity { index: 3, generation: 0 }` is now stale and
`world.entities().is_alive(old)` returns false.
```

Costs:

| Operation | Cost |
|-|-|
| Spawn (no free slot) | O(1) — push onto `generation` |
| Spawn (free slot available) | O(1) — pop free list, return existing index |
| Despawn | O(1) — bump generation, push onto free list |
| `is_alive(entity)` | O(1) — compare against `generation[entity.index]` |

### Components: the sparse-set storage

Each component type gets its own storage — a *sparse set*:

```text
ComponentStorage<Position>:

  sparse:        [None,  Some(0),  None,  Some(1),  None,  Some(2)]
                    ↑       ↑        ↑       ↑        ↑       ↑
                  e0       e1       e2      e3       e4      e5
                                                              (e2, e4 lack Position)

  dense:         [Position(p1),  Position(p3),  Position(p5)]
                       ↑              ↑              ↑
                  index 0         index 1         index 2

  entity_index:  [e1, e3, e5]            ◀── reverse lookup for swap-remove
```

To read `Position` for entity `e3`:

```text
  sparse[e3.index] = Some(1)
  dense[1]         = Position(p3)         ◀── O(1) lookup
```

To remove `Position` from entity `e1`:

```text
  swap dense[0] with dense[2], pop the tail:
    dense:        [Position(p5), Position(p3)]
    entity_index: [e5, e3]
  patch sparse[e5.index] = Some(0)
  set   sparse[e1.index] = None
```

That's the **swap-remove** trick: removal is O(1) and keeps `dense`
densely packed for fast iteration.

| Operation | Cost |
|-|-|
| `insert::<T>(entity, value)` | O(1) — push onto `dense`, set `sparse[i]` |
| `remove::<T>(entity)` | O(1) — swap-remove from `dense`, patch sparse |
| `get::<T>(entity)` | O(1) — `dense[sparse[entity.index]]` |
| Iterate all `T`s | O(n) over the *dense* array — cache-friendly |

> **Why sparse sets, not archetypes?** Bevy uses archetype tables
> (one row per entity, one column per component type, all entities
> with the *same* component set share a table). Faster iteration in
> exchange for slower component add/remove (entities migrate between
> tables). For Spark's MVP — thousands of entities, lots of
> component churn from construction/repair/operational state — sparse
> sets are simpler, fast enough, and easier to reason about. Stage 24
> of the build plan keeps the door open for an archetype refactor
> behind the same public API.

### Spawning and despawning: Commands

Systems never mutate the world's *structure* directly — no calling
`world.spawn(...)` inside a system. Instead, systems queue
**commands** which the scheduler applies at the end of the workload.
This keeps iteration stable (a system iterating `Query<&Plant>` can't
have new plants pop into existence mid-loop) and keeps determinism
intact for parallel execution.

```rust,ignore
use spark_ecs::{Commands, Query, Res, EventReader, Entity};

fn place_plant_plan(
    mut events: EventReader<TileClicked>,
    economy: Res<Economy>,
    mut cmd: Commands,
) {
    for click in events.read() {
        if economy.capital >= 100.0 {
            // Spawn a tuple of components — that's an "entity bundle".
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
    for entity in &completed {
        cmd.entity(entity)
           .remove::<ConstructionProgress>()
           .remove::<ConstructionDone>()
           .insert(Operational);
    }
}
```

Commands available:

| Command | Effect |
|-|-|
| `cmd.spawn((A, B, C))` | Spawn a new entity with the tuple as a bundle |
| `cmd.despawn(entity)` | Remove the entity and all its components |
| `cmd.entity(e).insert(c)` | Add component `c` to existing entity |
| `cmd.entity(e).remove::<T>()` | Remove component `T` from entity |
| `cmd.insert_resource(r)` | Add or replace a resource |
| `cmd.update_resource::<T>(\|t\| …)` | Mutate a resource in-place |
| `cmd.send_event(e)` | Equivalent to `EventWriter<E>::write(e)` |

Commands flush **between workloads**, never inside one. So if
workload A spawns an entity, workload B sees it; but two systems
inside A both see the *old* state until A finishes.

### Queries: finding entities

A query is a declarative spec of which entities a system wants. The
**data shape** says which components to read or write, the **filter**
says how to narrow the set.

#### Data shapes

```rust,ignore
Query<&Position>                            // immutable single
Query<&mut Position>                        // mutable single
Query<(&Position, &Velocity)>               // tuple: read both
Query<(&mut Position, &Velocity)>           // tuple: write one, read one
Query<(Entity, &Position)>                  // include the Entity ID
Query<(&Position, Option<&Velocity>)>       // Velocity may be absent
```

Iterating returns tuples in the same shape:

```rust,ignore
fn integrate(time: Res<Time>, mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0 * time.delta;
    }
}
```

#### Filters

Add a second type parameter to narrow further. Filters don't *fetch*
the component, they only *gate* the iteration:

```rust,ignore
// Operational plants only (Operational is a marker — zero-sized).
Query<&Plant, With<Operational>>

// Workers that don't currently have a job.
Query<&Worker, Without<CurrentJob>>

// Multiple filters as a tuple = AND.
Query<&Plant, (With<Operational>, Without<UnderMaintenance>)>

// Or<(F1, F2)> = either filter matches.
// (Deferred — ships after the initial With/Without pair lands.)
Query<&Plant, Or<(With<Operational>, With<UnderMaintenance>)>>
```

#### All variants at a glance

```rust,ignore
// — DATA SHAPES —
Query<&T>                                    // read
Query<&mut T>                                // write
Query<(&A, &B)>                              // read tuple
Query<(&mut A, &B)>                          // mixed
Query<(&A, &B, &C)>                          // arity grows freely
Query<Entity>                                // just the ID, no component
Query<(Entity, &Position, &mut Velocity)>    // ID + multiple components
Query<&Position, ()>                         // explicit empty filter

// — FILTERS (initial M3 set) —
With<T>            // entity must have T (but don't fetch T)
Without<T>         // entity must NOT have T
(F1, F2, F3)       // AND of filters

// — OPTIONAL DATA —
Option<&T>         // fetch T if present, give None otherwise

// — DEFERRED (follow-ups after the initial Query lands) —
Or<(F1, F2)>       // OR of filters — comes after With/Without
Changed<T>         // only entities whose T was mutated this frame
Added<T>           // only entities that gained T this frame

// — NOT SUPPORTED — explicitly refuses to compile —
// Query<(&mut A, &mut B)>  // two mutable joins; needs aliasing reasoning
//                          // that belongs to M4 parallel-executor work.
```

### Iteration and cost

The driver-storage trick is what keeps queries fast. To resolve
`Query<(&A, &B)>` the engine:

1. Picks the **smallest** of `A`'s storage and `B`'s storage as the
   *driver*.
2. Iterates the driver's `dense` array.
3. For each entity, looks up the other component via O(1) sparse-set
   access. Skips if absent.

So a `Query<(&Plant, &CityName)>` where there are 50 plants and 200
cities-with-names runs in 50 iterations, not 50 × 200.

| Query | Iteration cost |
|-|-|
| `Query<&T>` | O(n) over T's dense array; n = entities with T |
| `Query<(&A, &B)>` | O(min(\|A\|, \|B\|)) — driver picks smaller |
| `Query<&A, With<B>>` | O(\|A\|) + one sparse lookup per item |
| `Query<&A, Without<B>>` | O(\|A\|) + one sparse lookup per item |
| `Query<Option<&T>>` | O(over the rest of the query) — Option doesn't gate |

Filters are essentially free: `With<T>`/`Without<T>` are a single
sparse lookup per candidate entity, no component fetch.

> **`(&mut A, &mut B)` is not allowed.** Composing two mutable joins
> would require aliasing reasoning the engine refuses to do in safe
> code — driving one storage's `iter_mut` while also `iter_mut`-ing
> another that may overlap can't be proven disjoint without unsafe.
> The trait bounds make it a compile error rather than a runtime
> hazard. Use `(&mut A, &B)` instead, swap if needed, and revisit
> when M4's parallel executor brings proven-disjoint access.

#### Combining queries, resources, commands, events

A single system can mix every parameter type. This is where ECS
ergonomics shine — read the function signature, you know everything
it touches:

```rust,ignore
fn city_growth(
    time: Res<Time>,                                  // resource read
    mut grid: ResMut<PowerNetwork>,                   // resource write
    mut cities: Query<(Entity, &mut City)>,           // entities + write
    plants: Query<&Plant, With<Operational>>,         // entities + filter
    mut events: EventWriter<CityTierUp>,              // event send
    mut cmd: Commands,                                // structural change
) {
    grid.supply = plants.iter().map(|p| p.output_mw).sum();

    for (id, mut city) in cities.iter_mut() {
        // ... update city.population based on grid.ratio ...
        if city.population >= 1000 {
            events.write(CityTierUp { city: id, new_tier: 2 });
        }
        if city.population == 0 {
            cmd.despawn(id);                          // queued, applied later
        }
    }
}
```

The compiler reads the parameter types, the engine wires up the
borrows. No `world.get_thing()` calls; no manual locking.

### Events: messages between systems

Events are how systems talk without depending on each other. A system
emits a `CityTierUp` event; any number of systems may read it. The
emitter and reader never reference each other directly.

```rust,ignore
#[derive(Event)]
pub struct CityTierUp { pub city: Entity, pub new_tier: u32 }

fn emit_tierups(mut writer: EventWriter<CityTierUp>, /* … */) {
    writer.write(CityTierUp { city, new_tier: 2 });
}

fn react_to_tierups(mut reader: EventReader<CityTierUp>) {
    for ev in reader.read() {
        // … play sound, animate banner, etc.
    }
}
```

Events are double-buffered: writes go into the "current" buffer;
readers see *this frame's* and *last frame's* writes. The buffers
swap at end of frame. That guarantees a reader registered after the
event was written still picks it up exactly once.

### Workloads and the schedule

Systems are grouped into **workloads** (named units of related work)
and workloads are ordered inside **schedules** (frame-shape slots).

```text
   ┌─────────────────────── one frame ─────────────────────────────┐
   │                                                                │
   │   First                                                        │
   │     │                                                          │
   │     ▼                                                          │
   │   PreUpdate                       ◀── input poll, time tick    │
   │     │                                                          │
   │     ▼                                                          │
   │   FixedUpdate × N                 ◀── 60 Hz deterministic sim │
   │     │                                                          │
   │     ▼                                                          │
   │   Update                          ◀── variable-rate logic     │
   │     │                                                          │
   │     ▼                                                          │
   │   PostUpdate                      ◀── cleanup                  │
   │     │                                                          │
   │     ▼                                                          │
   │   Render                          ◀── draw lists, GPU submit  │
   │     │                                                          │
   │     ▼                                                          │
   │   Last                                                         │
   │                                                                │
   │   Commands flush  ───── between every workload                 │
   │   Events swap     ───── between Last(N) and First(N+1)         │
   └────────────────────────────────────────────────────────────────┘
```

A workload is a named bundle. Power-grid systems go together in
`Workload::PowerGrid`; city-tick systems in `Workload::CityTick`.
Workloads can declare ordering between each other:

```rust,ignore
app.add_workload(Workload::PowerGrid, Schedule::FixedUpdate, |w| {
    w.add(collect_supply);                        // disjoint access
    w.add(compute_demand);                        // run in parallel
    w.add(distribute_power).after_all_prior();    // joins after both
    w.add(emit_blackout_events).after(distribute_power);
});

app.add_workload(Workload::CityTick, Schedule::FixedUpdate, |w| {
    w.after_workload(Workload::PowerGrid);        // sequential between workloads
    w.add(city_growth);
});
```

Within a workload, the M4 scheduler reads each system's declared
access set (from its parameter types) and runs non-conflicting
systems in parallel via Rayon. Between workloads, everything is
sequential and deterministic — commands flush, events propagate.

> **Stage-shape migration.** Today (M1) the schedule slots are
> string constants on `spark-core` (`stages::STARTUP = "startup"`,
> `stages::UPDATE = "update"`) and you call
> `app.add_system(stages::UPDATE, my_fn)`. The M3 scheduler PR
> replaces that stand-in with the canonical `Schedule` enum shown
> above (`Schedule::Startup`, `Schedule::Update`, …) and the call
> becomes `app.add_systems(Schedule::Update, my_fn)`. Same idea,
> compile-time exhaustiveness, no more stringly-typed footguns.

## Deep dive: how memory evolves

The *Components* section earlier showed what a `ComponentStorage<T>`
looks like at rest. This section walks through what happens in memory
**step by step** as you spawn entities, attach components, remove
them, despawn entities, and spawn again. Every operation here is
O(1) — but understanding *why* makes it easier to write systems that
stay fast and to read the code when something goes wrong.

### The structures we track

```text
EntityAllocator
├── generation:  Vec<u32>     ◀── one entry per slot, bumped on destroy
├── alive:       Vec<bool>    ◀── whether the slot is currently live
└── free_list:   Vec<u32>     ◀── LIFO of indices ready for reuse

ComponentStorage<T>
├── sparse:        Vec<Option<u32>>   ◀── entity.index → dense_idx
├── dense:         Vec<T>             ◀── packed component values
└── entity_index:  Vec<Entity>        ◀── dense_idx → Entity

World
├── entities:    EntityAllocator
└── components:  HashMap<TypeId, ComponentStorage<…>>
```

### T = 0 — an empty world

```text
allocator:           gen=[]  alive=[]  free=[]
storage<Position>:   not yet created  (no one has touched the TypeId)
```

### T = 1 — `world.spawn()` → E0

`allocate()` checks `free_list` — it's empty, so it pushes a brand-new
slot: `index = 0`, `generation[0] = 0`, `alive[0] = true`.

```text
allocator:  gen=[0]  alive=[T]  free=[]
E0 = Entity { index: 0, generation: 0 }
```

The storage map is still empty — no components yet.

### T = 2 — `world.insert(E0, Position { x: 0, y: 0 })`

This is the very first `Position` insert, so `World` lazily creates a
`ComponentStorage<Position>`. The storage stretches `sparse` to
`entity.index + 1` entries, writes `Some(0)` at the new entry,
pushes the value onto `dense`, and pushes `E0` onto `entity_index`.

```text
storage<Position>:
  sparse:        [Some(0)]
  dense:         [P{0, 0}]
  entity_index: [E(0, gen=0)]
                 ↑ dense_idx = 0
```

**Lookup cost:** `sparse[0]` → `Some(0)` → `dense[0]`. Two index
reads, O(1).

### T = 3, T = 4 — spawn E1 and E2, insert more components

```text
spawn E1 → index=1, generation=0       (free_list empty → push new slot)
spawn E2 → index=2, generation=0

world.insert(E1, Position{10, 0})
world.insert(E2, Position{20, 0})
world.insert(E2, Velocity{-1, 0})       ◀── new TypeId → new storage
```

State after:

```text
allocator:  gen=[0,0,0]  alive=[T,T,T]  free=[]

storage<Position>:
  sparse:        [Some(0), Some(1), Some(2)]
  dense:         [P{0,0},  P{10,0}, P{20,0}]
  entity_index: [E0,      E1,      E2]

storage<Velocity>:
  sparse:        [None, None, Some(0)]    ◀── only stretched to index=2
  dense:         [V{-1, 0}]
  entity_index: [E2]
```

Notice how `Velocity`'s `sparse` array is mostly `None` — E0 and E1
don't have a `Velocity`. That's where the **sparse** in "sparse set"
comes from: most slots are empty. The `dense` array, on the other
hand, is always packed: no gaps. That's what makes iteration
cache-friendly — `for v in &velocity_storage.dense` walks a
contiguous slab of memory.

### T = 5 — `world.remove::<Position>(E1)` — the swap-remove dance

This is the operation the sparse-set scheme exists for. Removing
`dense[1]` by shifting the tail down would be O(n). Instead, the
storage swaps the doomed element with the last one and pops:

```text
step 1:  dense_idx = sparse[1].take()    → 1,  sparse[1] = None
step 2:  last_idx  = dense.len() - 1     → 2
step 3:  dense_idx ≠ last_idx, so swap:
           swapped_entity        = entity_index[2] = E2
           sparse[E2.index = 2]  = Some(1)         ◀── redirect E2's sparse entry
step 4:  dense.swap_remove(1)
         entity_index.swap_remove(1)
```

After:

```text
storage<Position>:
  sparse:        [Some(0), None,   Some(1)]    ◀── E1 gone; E2 now points at dense[1]
  dense:         [P{0,0},          P{20,0}]
  entity_index: [E0,              E2]
```

`dense` stays packed. One swap, one pop — O(1). The trade-off:
iteration order isn't stable across removes. Systems don't depend on
that; they only need a *deterministic* iteration within a single
frame, which the engine still guarantees.

### T = 6 — `world.despawn(E2)` — type-erased cleanup

The `World` doesn't know which components are on E2 — it can't,
that's the cost of type-erasure. So `despawn` walks **every**
`Box<dyn AnyStorage>` in the `HashMap` and tells each one "remove this
entity if you have it":

```rust,ignore
fn despawn(&mut self, e: Entity) {
    for (_typeid, storage) in &mut self.components {
        storage.borrow_mut().remove_entity(e);   // no-op if absent
    }
    self.entities.destroy(e);
}
```

In `Position`'s storage, E2 sits at `dense[1]` *and* that's the last
slot, so the swap-remove degenerates to a plain pop. In `Velocity`'s
storage, E2 was the only entry — pop again.

```text
storage<Position>:
  sparse:        [Some(0), None, None]
  dense:         [P{0, 0}]
  entity_index: [E0]

storage<Velocity>:
  sparse:        [None, None, None]
  dense:         []
  entity_index: []
```

In the allocator:

```text
generation[2] += 1     (was 0, now 1)
alive[2] = false
free_list.push(2)
```

```text
allocator:  gen=[0,0,1]  alive=[T,T,F]  free=[2]
```

### T = 7 — `world.spawn()` again — the generation trick pays off

`allocate()` finds `free_list` non-empty, pops `2` (LIFO), sets
`alive[2] = true`. Crucially, it **does not** reset `generation[2]`
— that stays at `1`, the value it was bumped to during destroy.

```text
allocator:  gen=[0,0,1]  alive=[T,T,T]  free=[]
E3 = Entity { index: 2, generation: 1 }       ◀── same slot, new generation
```

Any old copy of `E2 = Entity { index: 2, generation: 0 }` still
floating around in user code is now provably stale:

```rust,ignore
world.is_alive(E2)   // E2.gen = 0, allocator.gen[2] = 1 → false ✓
world.is_alive(E3)   // E3.gen = 1, allocator.gen[2] = 1 → true  ✓
```

Without the generation field, `spawn` would return `Entity(index=2)`
again and the stale handle would silently point at the new tenant —
the classic **ABA bug**. The generation makes an entity's identity
unique forever, while still letting the underlying slot be reused.

### Cost table

| Operation | What it touches | Cost |
|-|-|-|
| `spawn()` | `allocator`: pop `free_list`, or push to `generation` + `alive` | O(1) amortised |
| `insert<T>(e, v)` | `storage<T>`: maybe grow `sparse`, push `dense` + `entity_index` | O(1) amortised |
| `get<T>(e)` | one hash on `TypeId`, two index reads | O(1) |
| `remove<T>(e)` | swap-remove `dense` + `entity_index`, patch the neighbour's `sparse` | O(1) |
| `despawn(e)` | for **every** storage in the `HashMap`: `remove_entity` | O(K), K = component types |

The detail worth knowing about `despawn`: its cost depends on how
many component **types** are registered in the `World`, *not* how
many components the doomed entity actually has. Every storage gets
its `RefCell::borrow_mut`, a `sparse[entity.index]` check, and (if
absent) an immediate return. Cheap, but not free — keep an eye on it
when component-type count grows into the hundreds.

### What actually takes memory

A realistic case: 10 000 map tiles, each with a `Position` (8 bytes)
and a `Walkable` marker component (zero-sized).

```text
allocator:
  generation:    10_000 × 4 B  =  40 000 B
  alive:         10_000 × 1 B  =  10 000 B
  free_list:     0             (no one has died yet)

storage<Position>:
  sparse:        10_000 × 4 B  =  40 000 B   (Option<u32> often packs to 4 B via niche)
  dense:         10_000 × 8 B  =  80 000 B
  entity_index:  10_000 × 8 B  =  80 000 B

storage<Walkable> (zero-sized):
  sparse:        10_000 × 4 B  =  40 000 B
  dense:         0             (ZST: Vec allocates no backing storage at all)
  entity_index:  10_000 × 8 B  =  80 000 B
```

Notice what dominates: `sparse` and `entity_index` — not the
component data itself. For **common** components (sit on most
entities: `Position`, `Visible`) that's a great deal — you pay 12 B
of overhead per slot for fast O(1) lookup. For **rare** components
(1 % of entities) most of `sparse` is wasted slots; that's where a
`HashMap`-backed storage would be a better fit. Spark sticks to one
storage scheme for M3 (sparse only) and revisits per-component
storage strategies only if profiling shows a need — premature
optimisation otherwise.

## A frame, step by step

Picture three plants, two cities, one worker, and a `PowerNetwork`
resource. Here's what happens during one frame.

**Before the frame**

```text
World state:
  Resources:
    Time { delta: 0.016, frame: 41 }
    PowerNetwork { supply: 8.0, demand: 6.0, ratio: 1.0 }
    InputState { … }

  Components:
    Position:        e1, e2, e3, e4, e5, e6   (all six)
    Plant:           e1, e2, e3                (three plants)
    Operational:     e1, e2                    (two are running)
    UnderMaintenance: e3                       (one is being repaired)
    City:            e4, e5                    (two cities)
    Worker:          e6                        (one worker)
```

**Step 1: `First` schedule** — frame bookkeeping. `Time::frame`
bumps to 42, the renderer swaps frame buffers.

**Step 2: `PreUpdate` workloads**

```text
   Workload::Input
     ↓
   poll_window_events   ◀── drain queued OS events into InputState
     ↓
   update_input_state   ◀── compute "just pressed" / "just released"
     ↓
   [Commands flush] ────  no commands queued by these systems
```

**Step 3: `FixedUpdate` workloads (run N times at 60 Hz)**

```text
   Workload::PowerGrid
     ↓
   collect_supply        ◀── Query<&Plant, With<Operational>>
                              iterates dense[Plant] = [p1, p2, p3]
                              filters With<Operational>: e1, e2 match
                              sum p1.output_mw + p2.output_mw → 8.0
                              writes ResMut<PowerNetwork>::supply = 8.0
     ↓ (parallel)
   compute_demand        ◀── Query<&mut City>
                              iterates dense[City] = [c4, c5]
                              c4.demand_mw = 200*0.001 = 0.2
                              c5.demand_mw = 800*0.001 = 0.8
                              writes ResMut<PowerNetwork>::demand = 1.0
                              writes ResMut<PowerNetwork>::ratio = 1.0
     ↓ (joins after both)
   distribute_power      ◀── Query<&mut City>
                              c4.supply_mw = 0.2 * 1.0 = 0.2
                              c5.supply_mw = 0.8 * 1.0 = 0.8
     ↓
   emit_blackout_events  ◀── grid.ratio is 1.0, nothing emitted
     ↓
   [Commands flush]      no commands queued

   Workload::CityTick (after_workload PowerGrid)
     ↓
   city_growth           ◀── Query<(Entity, &mut City)>
                              for c4: demand met → population += dt*2
                              for c5: demand met → population += dt*2
                              c5.population now 1001 →
                                EventWriter<CityTierUp>::write(…)
     ↓
   [Commands flush]      no commands queued
   [Events written]      queued for readers in remaining workloads
```

**Step 4: `Update` workloads** — variable-rate game logic.
`react_to_tierups` runs an `EventReader<CityTierUp>` over the writes
from `CityTick`. It schedules a tile-banner animation via `Commands`.

```text
   Workload::Reactions
     ↓
   react_to_tierups      ◀── reads the CityTierUp emitted above
                              cmd.spawn((TierUpAnimation { city: e5 }, …))
     ↓
   [Commands flush]      new entity e7 appears in storages
                              Position[e7], TierUpAnimation[e7]
```

**Step 5: `PostUpdate`** — cleanup. `despawn_finished_animations`
removes entities whose lifetime is up.

**Step 6: `Render`** — `build_draw_list` reads
`Query<(&Position, &Sprite)>`, writes a `RenderQueue` resource that
the render plugin submits to the GPU.

**Step 7: `Last`** — frame finalisation. The `Events<CityTierUp>`
buffer rotates: this frame's writes move to the "previous frame"
slot, the new "current" slot is empty. A reader registered next
frame still picks up the tierup once before it falls off.

End of frame. Back to Step 1 for frame 43.

## Quick reference: every query shape

| Query | What it iterates | Iteration item |
|-|-|-|
| `Query<&T>` | All entities with `T` | `&T` |
| `Query<&mut T>` | All entities with `T` | `&mut T` |
| `Query<(&A, &B)>` | Entities with both `A` and `B` | `(&A, &B)` |
| `Query<(&mut A, &B)>` | Entities with both | `(&mut A, &B)` |
| `Query<(&A, &B, &C)>` | Entities with all three | `(&A, &B, &C)` |
| `Query<Entity>` | Every alive entity | `Entity` |
| `Query<(Entity, &T)>` | Entities with `T`, including ID | `(Entity, &T)` |
| `Query<(&T, Option<&U>)>` | Entities with `T`, `U` if present | `(&T, Option<&U>)` |
| `Query<&T, With<U>>` | Entities with both `T` and `U`, only fetch `T` | `&T` |
| `Query<&T, Without<U>>` | Entities with `T` but not `U` | `&T` |
| `Query<&T, (With<U>, Without<V>)>` | AND of filters | `&T` |
| `Query<&T, Or<(With<U>, With<V>)>>` | OR of filters (*deferred*) | `&T` |

Iteration methods on every `Query`:

```rust,ignore
for x in q.iter() { … }                  // immutable
for x in q.iter_mut() { … }              // requires `mut q: Query<…>`
for x in &q { … }                        // sugar for q.iter()
for x in &mut q { … }                    // sugar for q.iter_mut()
let count = q.iter().count();
let first = q.iter().next();
let one = q.single();                    // panics if 0 or >1 entities
let maybe = q.get_single();              // Result<_, QuerySingleError>
let exact = q.get(entity)?;              // fetch one specific entity
```

## Where this crate fits

```text
        ┌──────────────────────────────────┐
        │       src/  (game binary)        │
        └────────────────┬─────────────────┘
                         │  depends on
                         ▼
        ┌──────────────────────────────────┐
        │  spark-log, spark-window,        │
        │  spark-render, …                 │
        │  (every other engine crate)      │
        └────────────────┬─────────────────┘
                         │  all depend on
                         ▼
        ┌──────────────────────────────────┐
        │           spark-core             │
        │   (Application, Plugin, stages,  │
        │    EngineError, re-exports of    │
        │    World/Res/ResMut/…)           │
        └────────────────┬─────────────────┘
                         │  depends on
                         ▼
        ┌──────────────────────────────────┐
        │          spark-ecs               │  ◀── this crate
        │       (stdlib only)              │
        └──────────────────────────────────┘
```

`spark-ecs` sits at the very bottom of the dep graph on purpose: pure
stdlib, no third-party crates, no engine crates above it. Every crate
above can pull `World`, `Res`, `ResMut`, and friends through
`spark-core`'s re-exports without circular dependency worries.

For the full milestone plan see [`docs/PLAN.md`](../../docs/PLAN.md);
for the engineering rationale behind every design choice in this
README see [`docs/ECS_DESIGN.md`](../../docs/ECS_DESIGN.md).

[`World`]: struct.World.html
[`SystemParam`]: trait.SystemParam.html
[`IntoSystem`]: trait.IntoSystem.html
