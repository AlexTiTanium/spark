# spark-ecs

The Spark engine's custom Entity-Component-System (ECS). The deepest
crate in the workspace: pure stdlib, no engine dependencies — every
other engine crate (including `spark-core`) sits on top.

> **What is an ECS?** It's a data layout. Instead of objects that own
> their behaviour (`player.update()`), you split things into:
> - **Entities** — bare identifiers, no data of their own.
> - **Components** — small structs (`Position`, `Health`, `Sprite`)
>   stored in per-type tables keyed by the entity id.
> - **Systems** — functions that read and write those tables.
>
> Adding a feature is usually adding a new component and a new system,
> not editing existing classes. The update graph stays flat.

> **Today vs tomorrow.** Code blocks tagged ` ```rust ` compile and
> run today — they're doc tests, kept honest by
> `cargo test --doc -p spark-ecs`. Code blocks tagged ` ```rust,ignore `
> show types that don't exist yet; they're the spec of what's coming,
> not what's runnable. (`Commands`, the events API — `Event` / `Events`
> / `EventReader` / `EventWriter` — and the workload API —
> `WorkloadLabel` / `Schedule` / `add_workload` — ship today.) `Query`
> exists today for `&T` / `&mut T`, every `&` / `&mut` combination
> of 2-/3-/4-/5-tuples (including multi-mut at any arity, e.g.
> `(&mut A, &mut B, &mut C)`), and the filter generic `Query<D, F>`
> (`With` / `Without` / `And` / `Or`); the remaining spec-frozen
> extensions (`Option<&T>`, `Entity`-as-data) are marked inline as ⏳
> in the *What's next* section. The forward-looking
> design is settled — see [`docs/ECS_DESIGN.md`](../../docs/ECS_DESIGN.md)
> for the full engineering reference and [`docs/PLAN.md`](../../docs/PLAN.md)
> for the milestone plan.

## Why ECS instead of OOP?

Suppose you're writing a power-grid simulator. Some objects are power
plants that produce energy. Some are cities that consume it. Some are
workers walking around to repair things. Some are plans the player
has placed but nothing has been built there yet.

In an OOP design you'd reach for inheritance:

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
component go? On `Plant`? Then you need an `is_built` check
everywhere. On a new `PlantUnderConstruction`? Then construction has
two cases. On a wrapper? Now you've got delegation hell.

In an ECS you stop asking "what is this thing" and start asking "what
does this thing have". A *plant under construction* is just an entity
with:

```text
    Position, BuildingKind, ConstructionProgress
```

When construction finishes, a system removes `ConstructionProgress`
and adds `Operational`. The entity didn't *become* a different class
— its component set changed.

> **What about Unity's `GameObject` components?** That's the same
> idea applied half-way: each `GameObject` is a heap-allocated
> container holding a list of components. A pure ECS goes further —
> entities are *just an ID*, components live in tightly-packed arrays
> keyed by that ID, and systems iterate the arrays directly. Same
> ergonomics, much better data layout for cache and parallelism.

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

- **Entity** — 64 bits: `index: u32, generation: u32`. Nothing else.
  Generations let the engine reuse freed indices without confusing
  old handles for new tenants.
- **Component** — any data struct that opts in with
  `#[derive(Component)]` (`Position`, `Velocity`, `Plant`,
  `PlayerControlled`). Marker components (zero-sized like
  `Operational`) are fine. Components live in per-type storages inside
  the `World`; the derive's `Send + Sync + 'static` bound is what lets
  the scheduler iterate them in parallel.
- **System** — a plain Rust function whose parameters describe what
  it reads and writes. Today: `Res<T>`, `ResMut<T>`, `Query<D>` (for
  `D = &T`, `&mut T`, or a tuple of those), and `Commands` for
  deferred spawn / despawn / insert. `EventReader`/`Writer` and
  `Local` land in follow-up PRs — see *What's next* further down.

```rust
// Runs today. `Time` here is a tiny stand-in for the future
// `spark-time` resource — the real one arrives with the frame-loop
// PR. Everything else (`Res`, `Query`, the tuple join, `iter_mut`)
// is real spark-ecs API.
use spark_ecs::{Component, IntoSystem, Query, Res, Resource, World};

#[derive(Resource)]
struct Time { delta: f32 }
#[derive(Component)]
struct Position(f32, f32);
#[derive(Component)]
struct Velocity(f32, f32);

fn integrate(time: Res<Time>, mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0 * time.delta;
    }
}

let mut world = World::new();
world.add_resource(Time { delta: 0.016 });
world.spawn().insert(Position(0.0, 0.0)).insert(Velocity(60.0, 0.0));

let mut sys = IntoSystem::into_system(integrate);
sys(&world);
// 60 units/sec × 0.016 sec ≈ 0.96. Extract the value into a `f32`
// before the `Query` drops — `&Position` can't outlive its query.
let x = Query::<&Position>::from_world(&world).iter().map(|p| p.0).next().unwrap();
assert!((x - 0.96).abs() < 0.001);
```

This declarative shape — *the function signature is the interface* —
is the trick that makes ECS code stay readable as the game grows.

## Plug it into the `Application`

`spark-ecs` does not register itself with the engine — there's no
`EcsPlugin`. The `World` is owned by `spark_core::Application` from
the moment you construct it. Resources, entities, and components
flow through either `Application` methods (`add_resource`,
`add_system`) or — for one-shot setup during a plugin's `build()` —
through `app.world_mut()`, which hands out a plain `&mut World`.

```rust,ignore
// `rust,ignore` because spark-ecs is the bottom of the dep graph —
// it can't reach `spark_core` from its own doctest. Try this snippet
// in any crate that depends on both `spark-core` and `spark-ecs`.
use spark_core::{Application, Plugin};
use spark_ecs::{Res, ResMut};

struct Score(u32);
struct GameTime { dt: f32 }

fn tick(time: Res<GameTime>, mut score: ResMut<Score>) {
    score.0 += (time.dt * 1000.0) as u32;
}

struct ScorePlugin;
impl Plugin for ScorePlugin {
    fn build(&self, app: &mut Application) {
        app.add_resource(GameTime { dt: 0.016 })
            .add_resource(Score(0))
            .add_system(spark_core::Stage::Startup, tick);
    }
}

Application::new().add_plugin(ScorePlugin).run().unwrap();
```

For plugins that need to *pre-populate* the world with entities —
loading a level, seeding a tile grid, instantiating a fixture for
tests — reach for `app.world_mut()`:

```rust,ignore
// Same dep-graph reason as the previous block.
use spark_core::{Application, Plugin};

struct Tile { x: i32, y: i32 }
struct Walkable;

struct LevelPlugin;
impl Plugin for LevelPlugin {
    fn build(&self, app: &mut Application) {
        let world = app.world_mut();
        for y in 0..10 {
            for x in 0..10 {
                world.spawn().insert(Tile { x, y }).insert(Walkable);
            }
        }
    }
}

Application::new().add_plugin(LevelPlugin).run().unwrap();
```

The rest of this README operates directly on `World`, which *is*
exercisable in doctests. Mentally swap any standalone `World::new()`
for `app.world_mut()` when reading inside-a-plugin code.

## `World`: where state lives

The `World` owns three things, type-erased and keyed by `TypeId`:

- **Resources** — one value per type, the canonical home for engine
  singletons (`GameTime`, `RenderContext`, `InputState`).
- **Entities** — generational handles allocated and recycled by an
  `EntityAllocator`.
- **Components** — one `ComponentStorage<T>` per component type.

Every accessor returns a `Ref` or `RefMut` guard from a `RefCell`, so
they all take `&self` — two `&mut`-style borrows over *different*
types coexist freely inside one system.

```text
World
├── entities:   EntityAllocator
├── components: HashMap<TypeId, RefCell<Box<dyn AnyStorage>>>
│               ├── TypeId::of::<Position>() → ComponentStorage<Position>
│               ├── TypeId::of::<Velocity>() → ComponentStorage<Velocity>
│               └── …
└── resources:  HashMap<TypeId, RefCell<Box<dyn Any>>>
                ├── TypeId::of::<GameTime>() → GameTime
                └── …
```

Resources and components share the `RefCell<Box<dyn _>>` shape but
live in **separate** maps — that's why `resource_mut::<T>()` and
`get_mut::<T>()` (component) can be in flight simultaneously without
fighting each other.

## Resources

A resource is one value of one type. A second `add_resource::<T>`
overwrites the first.

```rust
use spark_ecs::{Resource, World};

#[derive(Resource)]
struct GameTime { dt: f32, elapsed: f32 }
#[derive(Resource)]
struct Score(u32);

let mut world = World::new();
world.add_resource(GameTime { dt: 0.016, elapsed: 0.0 });
world.add_resource(Score(0));

assert_eq!(world.resource::<Score>().0, 0);
world.resource_mut::<Score>().0 = 42;
assert_eq!(world.resource::<Score>().0, 42);
```

Two access patterns:

| Method | When |
|-|-|
| `world.resource::<T>()` / `world.resource_mut::<T>()` | You expect `T` to be present; missing → panic with the type name. |
| `world.get_resource::<T>()` / `world.get_resource_mut::<T>()` | `T` may not be present; missing → `None`. |

Two `resource_mut` over *different* `T` coexist (disjoint cells); two
over the *same* `T` panic at runtime (the `RefCell` uniqueness
check). The M4 parallel scheduler will catch the same-type conflict
at registration time and turn it into a compile-friendlier error.

```rust
use spark_ecs::{Resource, World};

#[derive(Resource)]
struct Time { dt: f32 }
#[derive(Resource)]
struct Score(u32);

let mut world = World::new();
world.add_resource(Time { dt: 0.016 });
world.add_resource(Score(0));

let mut t = world.resource_mut::<Time>();
let mut s = world.resource_mut::<Score>();
t.dt = 0.020;
s.0 = 7;
drop(t);
drop(s);
assert_eq!(world.resource::<Score>().0, 7);
```

Inside a *system*, prefer the `Res<T>` / `ResMut<T>` parameters —
see *Systems and parameters* below.

## Entities

Spawning an entity returns a chainable [`EntityMut`] builder. Call
`.id()` at the end if you need the handle; drop the builder if you
don't.

```rust
use spark_ecs::{Component, World};

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }
#[derive(Component)]
struct PlayerControlled;

let mut world = World::new();
let player = world.spawn()
    .insert(Position { x: 0.0, y: 0.0 })
    .insert(Velocity { x: 1.0, y: 0.0 })
    .insert(PlayerControlled)
    .id();

assert!(world.is_alive(player));
assert!(world.get::<Position>(player).is_some());
```

Operations on a live entity:

| Method | Effect |
|-|-|
| `world.insert::<T>(e, v)` | Attach `v` to `e`. Returns the displaced `T` if one was already attached. |
| `world.remove::<T>(e)` | Detach and return `e`'s `T`, or `None`. |
| `world.get::<T>(e)` | Borrow `e`'s `T` immutably, or `None` if absent. |
| `world.get_mut::<T>(e)` | Borrow `e`'s `T` mutably, or `None` if absent. |
| `world.despawn(e)` | Wipe `e` from every storage and free the slot. Returns `true` on a live handle. |
| `world.is_alive(e)` | `true` iff this exact handle (index *and* generation) is still live. |

**Dead-entity policy.** `insert`, `remove`, `get`, `get_mut` all
silently return `None` for stale or never-allocated handles — no
panics. `despawn` returns `false` for the same case. The single
explicit liveness check is `is_alive`.

```rust
use spark_ecs::{Component, World};

#[derive(Component)]
struct Tag;

let mut world = World::new();
let e = world.spawn().insert(Tag).id();
assert!(world.despawn(e));
assert!(!world.is_alive(e));

// Stale handle: every component-op returns None, despawn returns false.
assert!(world.get::<Tag>(e).is_none());
assert!(world.insert(e, Tag).is_none());
assert!(world.remove::<Tag>(e).is_none());
assert!(!world.despawn(e));
```

### Generational handles: why two `u32`s?

A naive entity id is just `u32`. Allocate `Entity(0)`; destroy it;
allocate again → you get `Entity(0)` back. Any code that kept the old
`Entity(0)` is now silently pointing at a *different* entity. That's
the classic **ABA problem**.

The fix: every handle carries a *generation* number alongside its
slot index. The allocator bumps `generation[index]` every time the
slot is reused. A stale handle's generation no longer matches what's
in the allocator, so `is_alive` returns `false`. Slots are reused
(bounded memory); identities never repeat in practice.

```text
EntityAllocator after spawn × 6, destroy slot 3:

   generation:  [0, 0, 0, 1, 0, 0]
                          ↑ bumped from 0 → 1 on destroy
   alive:       [T, T, T, F, T, T]
                          ↑ cleared on destroy
   free_list:   [3]
                 ↑ LIFO of indices ready for reuse

The next spawn pops `3`, sets alive[3] = true, and returns
Entity { index: 3, generation: 1 }. The old `Entity { index: 3,
generation: 0 }` no longer matches generation[3] — is_alive returns
false. The slot's index is recycled; its *identity* is not.
```

Live demonstration via the public API:

```rust
use spark_ecs::{Component, World};

#[derive(Component)]
struct Tag;

let mut world = World::new();
let old = world.spawn().insert(Tag).id();
world.despawn(old);

let fresh = world.spawn().id();
// Same slot under the hood, but distinct handles.
assert_ne!(old, fresh);
assert!(!world.is_alive(old));       // stale → dead
assert!(world.is_alive(fresh));      // fresh → alive
assert!(world.get::<Tag>(old).is_none());   // the Tag is gone too
assert!(world.get::<Tag>(fresh).is_none()); // the new tenant doesn't inherit it
```

Costs:

| Operation | Cost |
|-|-|
| Spawn (no free slot) | O(1) — push onto `generation` |
| Spawn (free slot available) | O(1) — pop free list, return existing index |
| Despawn | O(1) — bump generation, push onto free list |
| `is_alive(entity)` | O(1) — compare against `generation[entity.index]` |

### Driving the allocator directly

You can use [`EntityAllocator`] without going through `World` — useful
when writing entity machinery that doesn't need component storage:

```rust
use spark_ecs::EntityAllocator;

let mut alloc = EntityAllocator::new();
let a = alloc.allocate();
let b = alloc.allocate();
alloc.destroy(a);
let c = alloc.allocate();          // reuses a's slot, new generation
assert!(!alloc.is_alive(a));        // stale
assert!(alloc.is_alive(b));         // untouched
assert!(alloc.is_alive(c));         // fresh tenant of a's slot
assert_eq!(alloc.len(), 2);         // live count
```

[`EntityMut`]: struct.EntityMut.html
[`EntityAllocator`]: struct.EntityAllocator.html

## Components

A component is any data struct that opts in with
`#[derive(Component)]`. The derive emits an empty marker impl; the
trait's `Send + Sync + 'static` bound means a struct holding an `Rc`
or `RefCell` won't compile — move that into a `Resource` instead:

```rust
use spark_ecs::Component;

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }
#[derive(Component)]
struct Operational;          // marker component (zero-sized)
```

Inserting, reading, mutating, removing — all through `World`:

```rust
use spark_ecs::{Component, World};

#[derive(Component)]
struct Health(u32);

let mut world = World::new();
let e = world.spawn().insert(Health(100)).id();

// Read.
assert_eq!(world.get::<Health>(e).unwrap().0, 100);

// Mutate.
world.get_mut::<Health>(e).unwrap().0 -= 25;
assert_eq!(world.get::<Health>(e).unwrap().0, 75);

// Replace — `insert` returns the displaced value.
let old = world.insert(e, Health(50)).unwrap();
assert_eq!(old.0, 75);

// Detach.
let final_hp = world.remove::<Health>(e).unwrap();
assert_eq!(final_hp.0, 50);
assert!(world.get::<Health>(e).is_none());
```

### Sparse-set storage: O(1) everything, packed iteration

Each component type gets its own [`ComponentStorage<T>`] — three
parallel vectors:

```text
ComponentStorage<Position> for entities e0, e2, e4 holding Position:

  sparse:        [Some(0), None, Some(1), None, Some(2)]
                   ↑              ↑              ↑
                   e0             e2             e4
                                  (e1, e3 lack Position)
  dense:         [Pos₀,           Pos₂,          Pos₄]
                  dense_idx 0     dense_idx 1    dense_idx 2
  entity_index:  [e0,             e2,            e4]
```

Three vecs, each pulling its weight:

- **`sparse[entity.index]`** points to where in `dense` this entity's
  component lives, or `None`. Mostly empty — that's where "sparse"
  comes from.
- **`dense`** holds the packed `T` values. No gaps. Iteration is a
  straight walk — cache-friendly.
- **`entity_index`** mirrors `dense` and tells you which entity owns
  each dense slot. Required by swap-remove (next) and by `iter()` so
  it can yield `(Entity, &T)` pairs.

Why three vecs instead of `HashMap<Entity, T>`?

- **Iteration order is deterministic** (vec order), not randomized
  like `HashMap`. The simulation needs that for save/replay/multiplayer.
- **Iteration is cache-friendly** — `dense` is packed.
- **Insert/remove/get are still O(1)** with the sparse map.

To **read** `Position` for `e2`: `sparse[e2.index] = Some(1)` →
`dense[1]`. Two index reads, O(1).

To **remove** `Position` from `e0`: swap-remove `dense[0]` with
`dense[2]` and pop. `e4` (the swapped-in entity) gets its sparse
pointer fixed up:

```text
  sparse:        [None, None, Some(0), None, Some(1)]
                                              ↑
                                              e4 now points at dense[0]
  dense:         [Pos₄,         Pos₂]
  entity_index:  [e4,           e2]
```

That's the **swap-remove trick**: removal is O(1) and keeps `dense`
densely packed. The trade-off is that iteration order isn't stable
across removes; systems don't depend on that, only on determinism
within a single frame, which the engine still guarantees.

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
> component churn from construction/repair/operational state —
> sparse sets are simpler, fast enough, and easier to reason about.
> Stage 24 of the build plan keeps the door open for an archetype
> refactor behind the same public API.

You'll rarely touch [`ComponentStorage`] directly — `World` is the
ergonomic surface — but the type is public so tests and low-level
code can poke at it:

```rust
use spark_ecs::{Component, ComponentStorage, EntityAllocator};

#[derive(Component)]
struct Health(u32);

let mut alloc = EntityAllocator::new();
let e = alloc.allocate();
let mut storage = ComponentStorage::<Health>::new();
storage.insert(e, Health(100));
assert_eq!(storage.get(e).unwrap().0, 100);
assert_eq!(storage.len(), 1);
```

### How `despawn` cleans up every storage at once

`World::despawn(entity)` doesn't know which components `entity` has
— that's the cost of type-erasure. Instead it walks every
`Box<dyn AnyStorage>` in its map and says "remove this entity, if
you have it":

```text
World::despawn(e)
    │
    ├── for every storage in world.components:
    │       storage.borrow_mut().remove_entity(e)    ◀── type-erased remove
    │                                                    no-op if absent
    │
    └── world.entities.destroy(e)                    ◀── bump generation, free slot
```

Cost: O(K) in the number of *component types* in the world (not the
number of components on this entity). That's cheap until you have
hundreds of types — every storage gets a `RefCell::borrow_mut`, a
`sparse[entity.index]` check, and (if absent) an immediate return.

Type-erased `remove_entity` is what makes `despawn` work without
`World` knowing the entity's component types. The trait that powers
it is [`AnyStorage`]; you'll only meet it if you're writing low-level
storage glue.

[`ComponentStorage<T>`]: struct.ComponentStorage.html
[`ComponentStorage`]: struct.ComponentStorage.html
[`AnyStorage`]: trait.AnyStorage.html

## Memory evolution: step by step

The diagrams above show what a `ComponentStorage<T>` looks like at
rest. This section walks through what happens in memory **step by
step** as you spawn entities, attach components, remove them, despawn
entities, and spawn again. Every operation here is O(1) — but
understanding *why* makes it easier to write systems that stay fast,
and to read the code when something goes wrong.

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
├── entities:   EntityAllocator
├── components: HashMap<TypeId, RefCell<Box<dyn AnyStorage>>>
└── resources:  HashMap<TypeId, RefCell<Box<dyn Any>>>
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

The components map is still empty — no components yet.

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
that; they only need *deterministic* iteration within a single
frame, which the engine still guarantees.

### T = 6 — `world.despawn(E2)` — type-erased cleanup

The `World` doesn't know which components are on E2 — that's the
cost of type-erasure. So `despawn` walks **every** storage and tells
each one "remove this entity if you have it":

```rust
use spark_ecs::{Component, World};

#[derive(Debug, Component)] struct Position { x: f32, y: f32 }
#[derive(Debug, Component)] struct Velocity { x: f32, y: f32 }

let mut world = World::new();
let e = world.spawn()
    .insert(Position { x: 20.0, y: 0.0 })
    .insert(Velocity { x: -1.0, y: 0.0 })
    .id();

assert!(world.despawn(e));
assert!(world.get::<Position>(e).is_none());
assert!(world.get::<Velocity>(e).is_none());
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
floating around in user code is now provably stale. The check is the
public `is_alive`:

```rust
use spark_ecs::{Component, World};

#[derive(Component)]
struct Tag;

let mut world = World::new();
let old = world.spawn().insert(Tag).id();
world.despawn(old);

let fresh = world.spawn().id();
// Same slot under the hood; distinct identities.
assert_ne!(old, fresh);
assert!(!world.is_alive(old));        // stale handle → dead ✓
assert!(world.is_alive(fresh));        // fresh handle → alive ✓
```

Without the generation field, `spawn` would return `Entity(index=2)`
again and the stale handle would silently point at the new tenant —
the classic **ABA bug**. The generation makes an entity's identity
unique in practice, while still letting the underlying slot be
reused.

### Cost table

| Operation | What it touches | Cost |
|-|-|-|
| `spawn()` | `allocator`: pop `free_list`, or push to `generation` + `alive` | O(1) amortised |
| `insert<T>(e, v)` | `storage<T>`: maybe grow `sparse`, push `dense` + `entity_index` | O(1) amortised |
| `get<T>(e)` | one hash on `TypeId`, two index reads | O(1) |
| `remove<T>(e)` | swap-remove `dense` + `entity_index`, patch the neighbour's `sparse` | O(1) |
| `despawn(e)` | for **every** storage in the components map: `remove_entity` | O(K), K = component types |

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

## Systems and parameters

A system is a plain Rust function. Its parameter types declare what
it reads and writes; the engine wires up the access.

```rust
use spark_ecs::{IntoSystem, Res, ResMut, Resource, World};

#[derive(Resource)]
struct GameTime { dt: f32 }
#[derive(Resource)]
struct Score(u32);

fn tick_score(time: Res<GameTime>, mut score: ResMut<Score>) {
    score.0 += (time.dt * 1000.0) as u32;
}

let mut world = World::new();
world.add_resource(GameTime { dt: 0.016 });
world.add_resource(Score(0));

// In normal use, `Application::add_system(stage, fn)` does this for you.
let mut system = IntoSystem::into_system(tick_score);
system(&world);
system(&world);
// 0.016 * 1000 = 16, truncated. Called twice → 32.
assert_eq!(world.resource::<Score>().0, 32);
```

Three parameter types ship today:

| Param | Effect |
|-|-|
| `Res<T>` | Immutable borrow of resource `T`. Derefs to `&T`. |
| `ResMut<T>` | Mutable borrow of resource `T`. Derefs to `&mut T`. |
| `Query<D, F>` | Walks every entity matching the data shape `D` (a single `&T`/`&mut T` or a tuple of those), optionally narrowed by a filter `F` (`With`/`Without`/`And`/`Or`; defaults to no filter). See *Walking entities with `Query<D>`* below. |

The wrapper supports arities 0..=4. Adding a fifth would mean adding
one more line to the `impl_into_system!` macro list.

Two `ResMut` over the *same* `T` inside one system panic at runtime
(the `RefCell` check). Two `ResMut` over *different* `T` types are
fine — they target disjoint storage cells. The M4 parallel scheduler
will catch same-type conflicts at registration time and turn them
into a compile-friendlier error.

```rust
use spark_ecs::{IntoSystem, ResMut, Resource, World};

#[derive(Resource)]
struct A(u32);
#[derive(Resource)]
struct B(&'static str);

// Two ResMut on different types — fine, disjoint cells.
fn write_both(mut a: ResMut<A>, mut b: ResMut<B>) {
    a.0 += 1;
    b.0 = "ok";
}

let mut world = World::new();
world.add_resource(A(0));
world.add_resource(B(""));

let mut sys = IntoSystem::into_system(write_both);
sys(&world);
assert_eq!(world.resource::<A>().0, 1);
assert_eq!(world.resource::<B>().0, "ok");
```

### Walking entities with `Query<D>`

`Query<D>` is the third `SystemParam`. While `Res<T>` / `ResMut<T>`
borrow a *single* resource, `Query<D>` walks *every* entity that
matches the data shape `D`. The shape can be one component (`&T` or
`&mut T`) or a tuple of components; in the tuple case, only entities
that hold **every** element in the shape are yielded.

Path B (Bevy-style): `iter()` / `iter_mut()` yield `D::Item<'_>`
directly — `Query<&T>` yields `&T`, not `(Entity, &T)`. The entity id
becomes available later via the planned `Query<(Entity, &T)>` shape.

**Single-component read.** The simplest query — every entity with a
`Health` component:

```rust
use spark_ecs::{Component, Query, World};

#[derive(Component)]
struct Health(u32);

let mut world = World::new();
world.spawn().insert(Health(50));
world.spawn().insert(Health(100));

let total: u32 = Query::<&Health>::from_world(&world)
    .iter()
    .map(|h| h.0)
    .sum();
assert_eq!(total, 150);
```

**Two-component join — the canonical movement system.** Drives the
first storage and sparse-looks-up the second. Entities missing either
component are skipped:

```rust
use spark_ecs::{Component, Query, World};

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }

let mut world = World::new();
world.spawn()
    .insert(Position { x: 0.0, y: 0.0 })
    .insert(Velocity { x: 1.0, y: 0.5 });
// Lonely Position with no Velocity — the join must skip it.
world.spawn().insert(Position { x: 100.0, y: 100.0 });

{
    // Scope the query so its `RefMut` drops before the read below.
    let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
    for (mut pos, vel) in q.iter_mut() {
        pos.x += vel.x;
        pos.y += vel.y;
    }
}

let xs: Vec<f32> = Query::<&Position>::from_world(&world)
    .iter()
    .map(|p| p.x)
    .collect();
assert!(xs.contains(&1.0));     // joined entity moved by (1.0, 0.5)
assert!(xs.contains(&100.0));   // lonely entity untouched
```

**As a `SystemParam` in a real system.** No explicit `from_world` —
the runner threads the query in for you, the same way it does
`Res<T>` / `ResMut<T>`. This is also the right place to introduce a
`Time`-style resource (a tiny stand-in until the real `spark-time`
crate lands):

```rust
use spark_ecs::{Component, IntoSystem, Query, Res, Resource, World};

// Stand-in for the future `Time` resource — same shape, no engine
// integration. Real per-frame `delta` arrives with the frame-loop PR.
#[derive(Resource)]
struct Time { delta: f32 }

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }

fn integrate(time: Res<Time>, mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.x += vel.x * time.delta;
        pos.y += vel.y * time.delta;
    }
}

let mut world = World::new();
world.add_resource(Time { delta: 0.5 });
world.spawn()
    .insert(Position { x: 0.0, y: 0.0 })
    .insert(Velocity { x: 2.0, y: 4.0 });

let mut sys = IntoSystem::into_system(integrate);
sys(&world);
sys(&world);
// Two ticks at delta = 0.5 → position advanced by (2.0, 4.0) total.
// Pull the values out before the `Query` drops.
let (x, y) = Query::<&Position>::from_world(&world)
    .iter()
    .map(|p| (p.x, p.y))
    .next()
    .unwrap();
assert!((x - 2.0).abs() < f32::EPSILON);
assert!((y - 4.0).abs() < f32::EPSILON);
```

Tuples scale flat to arity 4 (`(&A, &B, &C)`, `(&A, &B, &C, &D)`, with
the same mut-driver and multi-mut variants as arity 2). Tuples do
**not** nest: `Query<((&A, &B), &C)>` and `Query<(&A, (&B, &C))>` are
not supported shapes — flatten to `Query<(&A, &B, &C)>` instead.
See *Narrowing with filters* just below for `With`/`Without`/`And`/`Or`,
and *`Query<D, F>`: finding entities* under *What's next* for the full
data-shape table plus the `Entity`-as-data and `Option<&T>` shapes still
coming in follow-up PRs.

**Two-mut join — both elements mutable.** Useful when one component
needs to be both read and mutated alongside another (e.g. apply drag to
`Velocity` while integrating it into `Position`):

```rust
use spark_ecs::{Component, Query, World};

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }

let mut world = World::new();
world.spawn()
    .insert(Position { x: 0.0, y: 0.0 })
    .insert(Velocity { x: 1.0, y: 0.5 });

{
    let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
    for (mut pos, mut vel) in q.iter_mut() {
        pos.x += vel.x;
        pos.y += vel.y;
        // Drag on the way out — only possible because Velocity is `&mut` too.
        vel.x *= 0.9;
        vel.y *= 0.9;
    }
}

let (vx, vy) = Query::<&Velocity>::from_world(&world)
    .iter()
    .map(|v| (v.x, v.y))
    .next()
    .unwrap();
assert!((vx - 0.9).abs() < 1e-6);
assert!((vy - 0.45).abs() < 1e-6);
```

Under the hood, the multi-mut path uses one tightly contracted
`unsafe fn` to look up the non-driver storage's `&mut T` by entity.
That contract — "each entity is visited once per join" — is upheld by
the driver iterator's shape (`slice::iter_mut`, structurally one pass)
plus a runtime check that the data shape never names the same
component twice. The check (`QueryAccess::assert_no_self_conflict`)
runs inside `Query::from_world` **before** any storage is borrowed and
turns the would-be `(&mut A, &A)` aliasing case into a precise panic:

```rust,should_panic
use spark_ecs::{Component, Query, World};

#[derive(Component)]
struct Position(f32, f32);

let mut world = World::new();
world.spawn().insert(Position(0.0, 0.0));

// Panics: "query has conflicting access to component `Position` (written and read)".
let _q = Query::<(&mut Position, &Position)>::from_world(&world);
```

**`for`-loop sugar — `&q` and `&mut q`.** Both forms also work directly
in a `for` header, no `.iter()` call needed. `for x in &q` desugars to
`q.iter()` (so it needs `D: ReadOnlyQueryData` — read-only shapes only),
and `for x in &mut q` desugars to `q.iter_mut()` (any shape):

```rust
use spark_ecs::{Component, Query, World};

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }

let mut world = World::new();
world.spawn()
    .insert(Position { x: 0.0, y: 0.0 })
    .insert(Velocity { x: 3.0, y: 1.0 });

// Shared — `&q` requires `D: ReadOnlyQueryData` (no `&mut T` in the shape).
// Scoped so its shared borrows drop before the mut query below (see
// *Borrow rules*).
{
    let q = Query::<(&Position, &Velocity)>::from_world(&world);
    for (pos, vel) in &q {
        assert_eq!(pos.x + vel.x, 3.0);
    }
}

// Exclusive — `&mut q` works for any shape, including `&mut T`.
{
    let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
    for (mut pos, vel) in &mut q {
        pos.x += vel.x;
        pos.y += vel.y;
    }
}

let moved = Query::<&Position>::from_world(&world)
    .iter()
    .map(|p| p.x)
    .next()
    .unwrap();
assert!((moved - 3.0).abs() < f32::EPSILON);
```

The sugar boxes the underlying iterator — one heap allocation when the
loop starts and one extra indirect call per item, both negligible next
to the work each iteration does. Reach for `.iter()` / `.iter_mut()`
directly when you need an adapter chain (`q.iter().map(…).sum()`) or the
very tightest loop. `for x in q` **by value** is intentionally not
implemented: consuming the query would drop its storage borrow guards
mid-iteration.

**Borrow rules.** Two `Query<&T>` over the same `T` in one system
coexist — shared borrows of the same storage stack. Two `Query<&mut T>`
over the same `T` panic on the second fetch (the `RefCell` rule). Two
`Query<&mut T>` over *different* `T`s are fine — disjoint cells. Two
mut references to the *same* component inside a single query
(`Query<(&mut A, &mut A)>`, `Query<(&mut A, &A)>`, or the reversed
`(&A, &mut A)`) panic at `from_world` from the self-conflict check,
with a message naming the offending component. The M4 scheduler will
hoist same-type conflicts *across* systems to registration-time
errors.

### Narrowing with filters: `Query<D, F>`

A second generic narrows *which* entities iterate without changing what
each yields. `Query<&Plant, With<Operational>>` reads `Plant` and still
yields `&Plant` — just for the operational ones. `F` defaults to `()`
(match everything), so a plain `Query<D>` *is* `Query<D, ()>`.

Four filters ship today:

| Filter | Keeps entities that… |
|-|-|
| `With<T>` | have a `T` (without fetching it) |
| `Without<T>` | lack a `T` |
| `And<(F1, F2, …)>` | match **every** inner filter |
| `Or<(F1, F2, …)>` | match **any** inner filter |

`And` is spelled out explicitly rather than as a bare tuple, so it stays
symmetric with `Or` and unambiguous when the two nest
(`And<(With<Online>, Or<(With<Powered>, With<Backup>)>)>`). That's a
deliberate divergence from Bevy's implicit tuple-AND.

```rust
// ✅ Compiles and runs today.
use spark_ecs::{And, Component, Or, Query, With, Without, World};

#[derive(Component)]
struct Plant { output_mw: f32 }
#[derive(Component)]
struct Operational;          // marker — zero-sized
#[derive(Component)]
struct UnderMaintenance;
#[derive(Component)]
struct Backup;

let mut world = World::new();
world.spawn().insert(Plant { output_mw: 4.0 }).insert(Operational);
world.spawn()
    .insert(Plant { output_mw: 9.0 })
    .insert(Operational)
    .insert(UnderMaintenance);
world.spawn().insert(Plant { output_mw: 2.0 }).insert(Backup);

// With: operational plants (online or not).
let operational = Query::<&Plant, With<Operational>>::from_world(&world)
    .iter()
    .count();
assert_eq!(operational, 2);

// And: operational AND not under maintenance.
type Healthy = And<(With<Operational>, Without<UnderMaintenance>)>;
let healthy: f32 = Query::<&Plant, Healthy>::from_world(&world)
    .iter()
    .map(|p| p.output_mw)
    .sum();
assert_eq!(healthy, 4.0);

// Or: has grid power *or* a backup source.
type Supplied = Or<(With<Operational>, With<Backup>)>;
let supplied = Query::<&Plant, Supplied>::from_world(&world).iter().count();
assert_eq!(supplied, 3);
```

`With<T>` reports a **read** of `T` to the access model, so
`Query<&mut T, With<T>>` is a self-conflict and panics at `from_world`,
exactly like `Query<(&mut T, &T)>`. `Without<T>` reports no access — it
is a pure exclusion. Either way filters ride on top of the safe
iteration path and add no `unsafe`.

# What's next

The types below are **spec-frozen**. Some ship today (✅) and some
don't (⏳). Code blocks tagged `rust,ignore` are spec-frozen but use
types that haven't landed yet — read them as "here's the shape coming
next". Code blocks tagged `rust` compile and run today.

## `Query<D, F>`: finding entities

A query is a declarative spec of which entities a system wants. The
**data shape** says which components to read or write, the **filter**
says how to narrow the set.

> **Status at a glance.** Data shapes for `&T`, `&mut T`, and every
> `&` / `&mut` combination of flat 2-/3-/4-/5-tuples (including
> multi-mut at any arity) are ✅ shipping today via
> [`Query<D>`](struct.Query.html), as is the filter generic
> `Query<D, F>` (`With` / `Without` / `And` / `Or`). `Option<&T>` and
> `Entity`-as-data remain ⏳ follow-up PRs. Each subsection below calls
> out which bits are runnable now and which are spec-frozen.

### Data shapes

```rust,ignore
// ✅ today — every `&` / `&mut` combination of 2-/3-/4-/5-tuples,
//          generated by one `impl_all_tuple!` invocation per arity:
Query<&Position>                            // immutable single
Query<&mut Position>                        // mutable single
Query<(&Position, &Velocity)>               // read-read
Query<(&mut Position, &Velocity)>           // mut driver, read non-driver
Query<(&Position, &mut Velocity)>           // read driver, mut non-driver
Query<(&mut Position, &mut Velocity)>       // multi-mut (self-conflict checked)
Query<(&A, &B, &C)>                         // arity 3, all-read
Query<(&mut A, &B, &C)>                     // arity 3, mut at any position…
Query<(&A, &mut B, &mut C)>                 //   …including multiple positions
Query<(&mut A, &mut B, &mut C)>             //   …or all of them
Query<(&A, &B, &C, &D)>                     // arity 4, same story
Query<(&mut A, &mut B, &mut C, &mut D)>     //   …up to fully mutable
Query<(&A, &B, &C, &D, &E)>                 // arity 5, same story
Query<(&mut A, &mut B, &mut C, &mut D, &mut E)>  //   …up to fully mutable

// ⏳ coming:
Query<(Entity, &Position)>                  // include the Entity ID
Query<(&Position, Option<&Velocity>)>       // Velocity may be absent
```

Iteration shape today is **path B** (Bevy-style): `Query<&T>::iter()`
yields `&T`, **not** `(Entity, &T)`. If the system needs the entity, it
will be asked for via the `Query<(Entity, &T)>` shape once that's
landed.

```rust
// ✅ Compiles and runs today.
use spark_ecs::{Component, Query, World};

#[derive(Component)]
struct Position(f32, f32);
#[derive(Component)]
struct Velocity(f32, f32);

let mut world = World::new();
world.spawn().insert(Position(0.0, 0.0)).insert(Velocity(1.0, 0.0));

let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
for (mut pos, vel) in q.iter_mut() {
    pos.0 += vel.0;
    pos.1 += vel.1;
}
```

```rust
// ✅ Runs today with a local `Time` stand-in. The real `spark-time`
// `Time` resource lands with the frame-loop PR; the shape stays
// identical.
use spark_ecs::{Component, IntoSystem, Query, Res, Resource, World};

#[derive(Resource)]
struct Time { delta: f32 }
#[derive(Component)]
struct Position(f32, f32);
#[derive(Component)]
struct Velocity(f32, f32);

fn integrate(time: Res<Time>, mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0 * time.delta;
    }
}

let mut world = World::new();
world.add_resource(Time { delta: 0.5 });
world.spawn().insert(Position(0.0, 0.0)).insert(Velocity(2.0, 0.0));

let mut sys = IntoSystem::into_system(integrate);
sys(&world);
// Pull the value out before the `Query` drops.
let x = Query::<&Position>::from_world(&world).iter().map(|p| p.0).next().unwrap();
assert!((x - 1.0).abs() < f32::EPSILON);
```

### Filters

✅ **Ships today.** A second type parameter narrows the entity set
without fetching anything — filters only *gate* the iteration. See
*Narrowing with filters* above for runnable examples; the shapes:

```rust,ignore
// Operational plants only (Operational is a marker — zero-sized).
Query<&Plant, With<Operational>>

// Workers that don't currently have a job.
Query<&Worker, Without<CurrentJob>>

// And<(…)> = every inner filter must match. Spelled explicitly (not a
// bare tuple) so it stays symmetric with Or.
Query<&Plant, And<(With<Operational>, Without<UnderMaintenance>)>>

// Or<(…)> = any inner filter matches.
Query<&Plant, Or<(With<Operational>, With<Backup>)>>

// And / Or nest freely — each is itself a filter.
Query<&Plant, And<(With<Online>, Or<(With<Powered>, With<Backup>)>)>>
```

### Change detection: `Changed<T>` / `Added<T>`

✅ **Ships today.** Two filters that ask *when* a component was last
touched, so a system processes only what moved:

- `Changed<T>` — `T` was **written** since this system last ran (insert,
  overwrite, or a `&mut T` write). React to edits: redraw an HP bar only
  when health changed, rebuild a spatial index only when a `Transform`
  moved.
- `Added<T>` — `T` was **first attached** since this system last ran. A
  one-shot: run setup for an entity the frame after it gains a component.

The model is **per-component clocks**. Each component type owns its own
tick that advances when something writes that type — so `Position`'s clock
and `Velocity`'s clock move independently. A system remembers, per
component it touches, the tick it last saw; a write bumps the clock past
that, and `changed_tick > last_seen` answers "changed since I looked." No
global frame counter.

```rust
use spark_ecs::{Changed, Component, Query, World};

#[derive(Component)]
struct Health(u32); // #[derive(Component)] required for Query to see it

let mut world = World::new();
world.spawn().insert(Health(100)); // Health's clock advances on insert

// A real system just writes `fn ui(q: Query<&Health, Changed<Health>>)`;
// the scheduler tracks each system's per-component baseline for you. With
// no prior run (baseline 0), the fresh insert reads as changed:
let changed = Query::<&Health, Changed<Health>>::from_world(&world)
    .iter()
    .count();
assert_eq!(changed, 1);
```

Three properties that fall out of this design — all verified by tests:

- **Precise marking.** `Query<&mut T>` yields a `Mut<T>` guard that marks
  the component changed only when you actually write through it
  (`hp.0 -= 1`). Iterate a thousand, write three, and only three are
  marked — no false positives from merely visiting, joining, or filtering.
  (The ergonomic cost: write `for mut hp in q.iter_mut()`, like Bevy.)
- **Pre-existing components are visible on first run.** Clocks start at 1
  and a fresh system's baseline is 0, so a component attached during setup
  (before any system ran) is seen by a system's first `Changed`/`Added`.
- **Command-spawned entities are seen next run.** A `commands.spawn()`
  flush advances the component's clock, so an `Added<T>` reaction sees it
  on its next run regardless of system order.

Both filters report a **read** of `T` (like `With<T>`), so
`Query<&mut T, Changed<T>>` is a self-conflict — read with `&T`, or detect
a *different* component (`Query<&mut Sprite, Changed<Transform>>`). They
compose with the combinators: `And<(With<Powered>, Changed<Load>)>`.

### All variants at a glance

```rust,ignore
// — DATA SHAPES —
Query<&T>                                    // ✅ read
Query<&mut T>                                // ✅ write
// Every `&` / `&mut` combination at arity 2, 3, 4, and 5 is ✅ today;
// runtime self-conflict check catches `(&mut A, &A)` / `(&mut A, &mut A)`.
Query<(&A, &B)>                              // ✅ arity 2: all 4 combos
Query<(&mut A, &mut B)>                      // ✅   (incl. multi-mut)
Query<(&A, &B, &C)>                          // ✅ arity 3: all 8 combos
Query<(&mut A, &mut B, &mut C)>              // ✅   (incl. fully mutable)
Query<(&A, &B, &C, &D)>                      // ✅ arity 4: all 16 combos
Query<(&mut A, &mut B, &mut C, &mut D)>      // ✅   (incl. fully mutable)
Query<(&A, &B, &C, &D, &E)>                  // ✅ arity 5: all 32 combos
Query<(&mut A, &mut B, &mut C, &mut D, &mut E)>  // ✅ (incl. fully mutable)
Query<Entity>                                // ⏳ just the ID, no component
Query<(Entity, &Position, &mut Velocity)>    // ⏳ ID + multiple components
Query<&Position, ()>                         // ✅ explicit empty filter (the default)

// — FILTERS —                        ✅ filters + change-detection PRs
With<T>            // entity must have T (but don't fetch T)
Without<T>         // entity must NOT have T
And<(F1, F2, …)>   // AND of filters (explicit, not a bare tuple)
Or<(F1, F2, …)>    // OR of filters — nests with And
Changed<T>         // ✅ T written since this system last ran
Added<T>           // ✅ T first attached since this system last ran

// — OPTIONAL DATA —                  ⏳ optional-fetch PR (added when needed)
Option<&T>         // fetch T if present, give None otherwise

// — ARITY 6+ —                       follow-up (one line per arity)
// Add `impl_all_tuple!(A, B, C, D, E, F);` in query.rs to unlock all
// 64 `&` / `&mut` combinations at arity 6. Pure mechanical extension —
// but monomorphisation cost doubles per step, weigh against need.
```

### Iteration and cost

The driver-storage trick is what keeps queries fast. To resolve
`Query<(&A, &B)>` the engine:

1. Today: picks the **first element** as the *driver*. The smaller-
   storage optimisation (pick the smaller of A's and B's storage) is
   a planned ⏳ follow-up.
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
| `Query<&A, And<(With<B>, With<C>)>>` | O(\|A\|) + one sparse lookup per filter term per item |
| `Query<(&A, Option<&T>)>` | O(over the rest of the query) — Option doesn't gate (⏳) |

Filters are essentially free: each filter borrows its storage **once**
per iteration (in `init_state`), then `With<T>` / `Without<T>` do a single
sparse lookup per candidate entity — no component fetch, no repeated
`RefCell` borrow. `Changed<T>` / `Added<T>` add one tick compare per item
against a baseline also fetched once.

> **Every `&` / `&mut` combination ships at arity 2-5.** Reads use
> the storage's safe `get`; mutable non-driver lookups fetch per
> entity via a tightly contracted `unsafe fn` (`DenseMut::get`).
> Soundness rests on two facts the engine *enforces*: each driver
> iteration visits an entity at most once (structural), and the
> data shape never names the same component twice (runtime check in
> `QueryAccess::assert_no_self_conflict`, run from `Query::from_world`
> before any storage borrow). `Query<(&mut A, &mut A)>`,
> `Query<(&mut A, &A)>`, and the reversed `Query<(&A, &mut A)>` panic
> at `from_world` with a precise message rather than tripping the
> `RefCell` "already borrowed" later. To unlock arity 6+, add one
> `impl_all_tuple!(A, B, C, D, E, F);` line in `query.rs` — the
> Cartesian-product macro generates every combination automatically,
> though monomorphisation cost doubles per step.

### Mixing queries, resources, commands, events

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
            events.send(CityTierUp { city: id, new_tier: 2 });
        }
        if city.population == 0 {
            cmd.despawn(id);                          // queued, applied later
        }
    }
}
```

The compiler reads the parameter types, the engine wires up the
borrows. No `world.get_thing()` calls; no manual locking.

## `Commands`: deferred mutations from inside a system

Systems can't mutate the world's *structure* directly — they hold
borrows on component storages, and structural changes (spawning,
inserting, despawning) would invalidate those borrows mid-iteration.
Instead, systems queue **commands** that the engine applies at the
end of the stage, when no system holds any borrow. This keeps
iteration stable (a system iterating `Query<&Plant>` can't have new
plants pop into existence mid-loop) and keeps determinism intact for
parallel execution.

> **Status at a glance.** Today's [`Commands`](struct.Commands.html)
> ships ✅ `spawn`, `despawn(entity)`, `EntityCommands::insert<T>`,
> and `EntityCommands::id()`. Resource-touching commands
> (`insert_resource`, `update_resource`), event sends (`send_event`),
> bundle inserts (`spawn((A, B, C))`), component removes
> (`.remove::<T>()`), and the `cmd.entity(e)` accessor are ⏳
> follow-up PRs.

```rust
// ✅ Compiles and runs today.
use spark_ecs::{Commands, Component, IntoSystem, Query, World};

#[derive(Component)]
struct Position { x: f32, y: f32 }
#[derive(Component)]
struct Velocity { x: f32, y: f32 }

fn spawn_pair(mut commands: Commands) {
    commands.spawn()
        .insert(Position { x: 0.0, y: 0.0 })
        .insert(Velocity { x: 1.0, y: 0.5 });
    commands.spawn()
        .insert(Position { x: 5.0, y: 5.0 })
        .insert(Velocity { x: -0.5, y: 0.5 });
}

let mut world = World::new();
let mut sys = IntoSystem::into_system(spawn_pair);
sys(&world);
// Component inserts are still queued — Query won't see them yet.
assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 0);
world.flush_commands();
// After flush the entities are reachable everywhere.
assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 2);
```

The handle returned by `spawn().id()` is real **immediately** —
allocating a fresh slot is a counter bump on the `EntityAllocator`,
no storage is touched, so there's nothing to defer for that piece.
The component-insert operations chained after `spawn` are the queued
parts. That's why the entity id is usable inside the same system —
e.g. to despawn it on the same frame for a quick round-trip.

```rust
// ✅ Compiles and runs today.
use spark_ecs::{Commands, Component, IntoSystem, Query, World};

#[derive(Component)]
struct Tag;

fn round_trip(mut commands: Commands) {
    let id = commands.spawn().insert(Tag).id();
    commands.despawn(id);
}

let mut world = World::new();
let mut sys = IntoSystem::into_system(round_trip);
sys(&world);
world.flush_commands();
assert_eq!(Query::<&Tag>::from_world(&world).iter().count(), 0);
```

### Flush timing

`Application::run_stage(stage)` flushes once after the stage's **sequential**
systems (those registered with `app.add_system(stage, fn)`), then runs the
stage's workload `Schedule` if one exists. So:

- A sequential `spawn` in `Startup` is visible in every `PreUpdate` /
  `Update` / `PostUpdate` system that follows.
- A sequential `spawn` in `Update` is visible in `PostUpdate` of the same
  frame — and to that stage's *own* workloads, which run after the flush.
- Two **sequential** systems both running in `Update`: the second does
  **not** see entities the first queued. They all run before the
  post-sequential flush.

Inside a `Schedule` (the workload batcher in *Ordering with workloads*
below) the flush is finer-grained: commands flush at every **workload**
boundary, so a later workload *does* see an earlier workload's queued
commands.

### Commands available today

| Command | Effect | Status |
|-|-|-|
| `commands.spawn()` | Allocates a fresh entity synchronously; returns `EntityCommands` for chained queued inserts. | ✅ |
| `commands.despawn(entity)` | Queues `World::despawn(entity)` for the next flush. | ✅ |
| `commands.spawn().insert::<T>(value)` | Queues an `insert::<T>(entity, value)` on the just-spawned entity. Chainable. | ✅ |
| `commands.spawn().id()` | Synchronously-allocated `Entity` — usable inside the same system. | ✅ |
| `commands.spawn((A, B, C))` | Bundle insert — `spawn` with a tuple of components. | ⏳ Bundle PR |
| `commands.entity(e).insert(c)` / `.remove::<T>()` | Mutate an existing entity. | ⏳ EntityCommands-for-existing-entity PR |
| `commands.insert_resource(r)` / `.update_resource::<T>(\|t\| …)` | Resource touches via commands. | ⏳ additive |
| `commands.send_event(e)` | Convenience for `EventWriter<E>::send(e)` from a command. | ⏳ follow-up |

### Why disjoint cells make this work

A single system can take both `Commands` and `Query<&mut T>` for any
`T` without `RefCell` panicking. The reason: `Commands` borrows the
[`World`](struct.World.html)'s `entities` cell (for synchronous
`spawn`) and `pending` cell (for queued ops); `Query<&mut T>`
borrows the storage cell for `T`. Disjoint cells, no runtime
collision.

```rust
// ✅ Compiles and runs today. Commands + Query<&mut T> in one signature.
use spark_ecs::{Commands, Component, IntoSystem, Query, World};

#[derive(Component)]
struct Position { x: f32, y: f32 }

fn mirror_each(q: Query<&Position>, mut commands: Commands) {
    // Iterate live positions, queue a mirrored sibling for each.
    let snapshots: Vec<(f32, f32)> = q.iter().map(|p| (p.x, p.y)).collect();
    for (x, y) in snapshots {
        commands.spawn().insert(Position { x: -x, y: -y });
    }
}

let mut world = World::new();
world.spawn().insert(Position { x: 1.0, y: 2.0 });

let mut sys = IntoSystem::into_system(mirror_each);
sys(&world);
world.flush_commands();
// Originals (1, 2) + mirrored (-1, -2) = 2 entities.
assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 2);
```

## `Events<T>`: messages between systems

Events are how systems talk without depending on each other. A system
emits a `CityTierUp`; any number of systems read it. Emitter and reader
never reference each other directly.

`Events<T>` is **double-buffered**: writers push into a `current` buffer,
readers iterate the `previous` one, and a per-type swap rotates the two
once per frame. So a reader sees **last frame's** writes — and every
reader in a frame sees the same set, exactly once, no matter what order
the systems run in. That one-frame delay buys determinism: nothing a
reader observes depends on intra-frame scheduling, which is exactly what
the save/replay mandate needs.

```rust
use spark_ecs::{Event, Events, EventReader, EventWriter, IntoSystem, ResMut, Resource, World};

#[derive(Event)]
struct CityTierUp { new_tier: u32 }

#[derive(Resource)]
struct Banner { last_tier: u32 }

fn emit_tierups(mut writer: EventWriter<CityTierUp>) {
    writer.send(CityTierUp { new_tier: 2 });
}

fn react_to_tierups(reader: EventReader<CityTierUp>, mut banner: ResMut<Banner>) {
    for ev in reader.read() {
        banner.last_tier = ev.new_tier; // … play sound, animate a banner, etc.
    }
}

let mut world = World::new();
world.add_resource(Events::<CityTierUp>::default());
world.add_resource(Banner { last_tier: 0 });

// Frame N: emit. The write lands in `current`, invisible to readers so far.
IntoSystem::into_system(emit_tierups)(&world);
IntoSystem::into_system(react_to_tierups)(&world);
assert_eq!(world.resource::<Banner>().last_tier, 0); // reader saw nothing yet

// Frame boundary: the swap rotates `current` into `previous`.
world.resource_mut::<Events<CityTierUp>>().swap();

// Frame N+1: the reader now sees last frame's write.
IntoSystem::into_system(react_to_tierups)(&world);
assert_eq!(world.resource::<Banner>().last_tier, 2);
```

You rarely call `swap` by hand. In an app,
`Application::add_event::<CityTierUp>()` (in `spark-core`) inserts the
`Events<T>` buffer and registers the swap on `Stage::Input`, pumped first
each frame by the window runner — so sending is just `EventWriter::send`
and reading next frame is `EventReader::read`.

`EventReader` is **stateless**: it holds no per-system cursor, so it always
reads the previous-frame snapshot rather than "resuming where it left off."
Same-frame reads (a writer and reader communicating *within* one frame)
would need a per-system cursor (`Local<T>`), which lands later — see
[`docs/ECS_ROADMAP.md`](../../docs/ECS_ROADMAP.md).

## Workloads and the schedule

Systems are grouped into **workloads** (named units of related work)
and workloads are ordered inside **schedules** (frame-shape slots).

```text
   ┌─────────────────────── one frame ─────────────────────────────┐
   │                                                                │
   │   First                                                        │
   │     │                                                          │
   │     ▼                                                          │
   │   Input                           ◀── event swap, input state  │
   │     │                                                          │
   │     ▼                                                          │
   │   PreUpdate                       ◀── time tick                │
   │     │                                                          │
   │     ▼                                                          │
   │   FixedUpdate × N                 ◀── 60 Hz deterministic sim  │
   │     │                                                          │
   │     ▼                                                          │
   │   Update                          ◀── variable-rate logic      │
   │     │                                                          │
   │     ▼                                                          │
   │   PostUpdate                      ◀── cleanup                  │
   │     │                                                          │
   │     ▼                                                          │
   │   Render                          ◀── draw lists, GPU submit   │
   │     │                                                          │
   │     ▼                                                          │
   │   Last                                                         │
   │                                                                │
   │   Commands flush  ───── between every workload                 │
   │   Events swap     ───── Input stage, top of frame              │
   └────────────────────────────────────────────────────────────────┘
```

### Batching systems: `Schedule`

Spark registers work two ways, by **intent** — two separate mechanisms that
share only the `Stage` they sit in:

- **Sequential systems** live on the `Application` (in `spark-core`):
  `app.add_system(stage, fn)` runs systems in the calling thread, in
  registration order, with no batching. Reach for these when you want
  predictability and no parallelism. (That side is documented in
  `spark-core`; this crate is the engine below it.)
- **Parallel-capable workloads** live in a `Schedule`. A `Schedule` is a
  *container for named workloads* — `add_workload(label, |w| { … })` is its
  only entry point. The scheduler reads each system's parameter types and
  packs systems that touch disjoint data into the same **batch** — the unit
  the M4 executor will hand to a thread pool.

A `Schedule` is **one stage's worth of workloads**. Inside a workload, from a
system's *parameter types alone* the scheduler knows what each reads and
writes; `batches(label)` reports the grouping for one workload, for tests and
diagnostics:

```rust
use spark_ecs::{Component, Query, Schedule, WorkloadLabel, World};

#[derive(WorkloadLabel)]
enum Motion {
    Step,
}

#[derive(Component)]
struct Position {
    x: f32,
}

#[derive(Component)]
struct Velocity {
    x: f32,
}

// Touch different components → no conflict → one shared batch.
fn move_positions(mut q: Query<&mut Position>) {
    for mut p in q.iter_mut() {
        p.x += 1.0;
    }
}
fn move_velocities(mut q: Query<&mut Velocity>) {
    for mut v in q.iter_mut() {
        v.x += 1.0;
    }
}

let mut schedule = Schedule::new();
schedule.add_workload(Motion::Step, |w| {
    w.add_systems((move_positions, move_velocities));
});
assert_eq!(schedule.batches(Motion::Step).len(), 1); // disjoint → one batch

let mut world = World::new();
schedule.run(&mut world); // runs the batch, then flushes commands
```

`Commands` is the exception to conflict tracking: it records *deferred*
edits, so a `Commands`-using system never conflicts with anything and
shares a batch freely.

A system whose *own* parameters conflict — two that write the same
component, or one writing what another reads, like
`fn(Query<&mut Pos>, Query<&mut Pos>)` — is refused by `w.add_system` at
registration, naming the offending type, rather than surfacing as a
`RefCell` "already borrowed" panic deep inside a later `run`.

### Ordering with workloads

When two systems in a workload *conflict* — one writes what the other reads —
sharing a batch is unsound, so you must say which runs first. `w.add_system`
hands back a handle for exactly that — order the systems against each other
inside the closure:

```rust
use spark_ecs::{Component, Query, Res, Resource, Schedule, WorkloadLabel, World};

// One enum per subsystem; each variant is a workload label.
#[derive(WorkloadLabel)]
enum Physics {
    Step,
}

#[derive(Resource)]
struct Gravity(f32);
#[derive(Component)]
struct Velocity {
    y: f32,
}
#[derive(Component)]
struct Position {
    y: f32,
}

fn apply_gravity(g: Res<Gravity>, mut q: Query<&mut Velocity>) {
    for mut v in q.iter_mut() {
        v.y -= g.0;
    }
}
fn integrate(mut q: Query<(&mut Position, &Velocity)>) {
    for (mut p, v) in q.iter_mut() {
        p.y += v.y;
    }
}

let mut world = World::new();
world.add_resource(Gravity(9.8));
world.spawn().insert(Position { y: 100.0 }).insert(Velocity { y: 0.0 });

let mut schedule = Schedule::new();
schedule.add_workload(Physics::Step, |w| {
    // `integrate` reads Velocity, which `apply_gravity` writes — declare it.
    let gravity = w.add_system(apply_gravity);
    w.add_system(integrate).after(gravity);
});
schedule.run(&mut world); // gravity, then integration, then a command flush
```

`w.add_system(..)` hands back a `SystemRef` you order against, directly —
no `.id()` step. It's a *handle*, not the function itself, so the same `fn`
added twice stays two distinct systems. `.after` / `.before` accumulate, so
one system can wait on several: real dependencies form a *partial* order (a
diamond), not a line.

```rust
use spark_ecs::{Schedule, WorkloadLabel, World};

#[derive(WorkloadLabel)]
enum Assets {
    Load,
}

fn read_files() {}
fn parse_meshes() {}
fn parse_textures() {}
fn upload_to_gpu() {}

let mut schedule = Schedule::new();
schedule.add_workload(Assets::Load, |w| {
    let files = w.add_system(read_files);
    let meshes = w.add_system(parse_meshes).after(files);
    let textures = w.add_system(parse_textures).after(files);
    w.add_system(upload_to_gpu).after(meshes).after(textures); // waits for both
});
schedule.run(&mut World::new());
```

Workloads order against *each other* by **label** — the same `.after` /
`.before` verb, on the value `add_workload` returns, with a
`WorkloadLabel` argument instead of a handle. Labels resolve lazily, so a
workload may be ordered against one registered later:

```rust
use spark_ecs::{Schedule, WorkloadLabel, World};

#[derive(WorkloadLabel)]
enum Grid {
    Supply,
    Distribute,
}

fn collect_supply() {}
fn route_power() {}

let mut schedule = Schedule::new();
schedule
    .add_workload(Grid::Distribute, |w| {
        w.add_system(route_power);
    })
    .after(Grid::Supply); // ordered against a workload not yet registered
schedule.add_workload(Grid::Supply, |w| {
    w.add_system(collect_supply);
});
schedule.run(&mut World::new()); // Supply runs before Distribute
```

### The conflict policy

A write-overlap with **no** declared order — between two systems, or two
workloads — is a registration error, surfaced when the schedule first
runs. `.any_order_with` asserts, per pair, that the two may run in either
order: they still land in separate batches (a conflict can never share
one), but you waive the *requirement to declare which comes first*. The
scheduler can't verify that commutativity — it doesn't see the system
bodies — so reach for `.after` / `.before` when in doubt. Because
`Commands` declares zero access, this fires only on real
component/resource clashes.

```rust
use spark_ecs::{ResMut, Resource, Schedule, WorkloadLabel, World};

#[derive(WorkloadLabel)]
enum Cleanup {
    Sweep,
}

#[derive(Resource)]
struct DeadCount(u32);

fn sweep_dead(mut d: ResMut<DeadCount>) {
    d.0 = 0;
}
fn compact_storage(mut d: ResMut<DeadCount>) {
    d.0 += 1;
}

let mut world = World::new();
world.add_resource(DeadCount(3));
let mut schedule = Schedule::new();
schedule.add_workload(Cleanup::Sweep, |w| {
    let sweep = w.add_system(sweep_dead);
    // Both write DeadCount; the result is the same in either order.
    w.add_system(compact_storage).any_order_with(sweep);
});
schedule.run(&mut world); // no panic — the conflict is acknowledged
```

Commands flush at every **workload boundary**: a workload's queued
spawns/despawns all apply before the next workload begins, which is what
makes a workload the atomic unit. The within-workload batching is the same
access analysis shown above; at M4 each batch's non-conflicting systems
will run in parallel via Rayon instead of one at a time. Between
workloads, everything stays sequential and deterministic.

> **Where the `Stage` enum lives.** The per-frame phases are the closed
> `Stage` enum (`Stage::Startup`, `Stage::Update`, …), and you register
> a system with `app.add_system(Stage::Update, my_fn)`. `Stage` lives in
> `spark-core` — the frame/app layer — not in `spark-ecs`, mirroring how
> Bevy's `bevy_app` owns `Update` / `Startup` while `bevy_ecs` owns the
> generic schedule machinery. The `Schedule` batcher above already lives
> in `spark-ecs`; the workload layer, the parallel executor, and the
> wiring that routes each `Stage` to a `Schedule` are still ahead. When
> that wiring lands, `add_system` will reach `Stage` through a
> `StageLabel` trait, keeping the dependency direction (`spark-ecs`
> *below* `spark-core`) cycle-free.

## Derive macros

`#[derive(Component)]`, `#[derive(Resource)]`, `#[derive(Event)]`, and
`#[derive(WorkloadLabel)]` ship today, from a nested `spark-ecs-macros`
crate at `lib/ecs/macros/`. Consumers depend only on `spark-ecs`, which
re-exports the derives — so one `use spark_ecs::Component;` brings in both
the trait and its derive.

`#[derive(WorkloadLabel)]` is the odd one out: it applies to an *enum*,
not a struct, and generates real method bodies rather than an empty marker
impl. The derive matches over the enum's unit variants to produce a
`WorkloadId` (the enum's `TypeId` plus the variant index) and a qualified
`"Enum::Variant"` name per variant — which is why it needs an enum, where
each variant is exactly one workload label. A tuple/struct variant, or a
non-enum, is a compile error.

The derive makes ECS membership explicit: a type is a component only
if it `#[derive(Component)]`s, a resource only if it
`#[derive(Resource)]`s. There's no blanket impl, so the two can't be
mistaken for one another — a resource no longer silently satisfies
`Query`. `Component` carries a `Send + Sync + 'static` bound (the
safety proof the M4 parallel executor needs to iterate component
storages across threads); a non-thread-safe struct fails to derive.
`Resource` carries only `'static`, because resources are the home for
inherently non-`Send` singletons (a `wgpu` surface, an OS handle) —
parallel-safety for those is the scheduler's job (keep the touching
system on the main thread), not a bound enforced at the type level.

```rust,ignore
// `ignore`: forward-looking spec — uses `Vec2`, which isn't in spark-ecs
// yet. Every derive shown here ships today; only the `Vec2` field type
// keeps this snippet from compiling as a doctest.
#[derive(Component, Debug, Clone, Copy)]
pub struct Position(pub Vec2);

#[derive(Component)]
pub struct Operational;          // marker (zero-sized)

#[derive(Resource, Default)]
pub struct PowerNetwork {
    pub supply: f32,
    pub demand: f32,
    pub ratio: f32,
}

#[derive(Event)]
pub struct CityTierUp { pub city: Entity, pub new_tier: u32 }
```

# Reference

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
                                EventWriter<CityTierUp>::send(…)
     ↓
   [Commands flush]      no commands queued
   [Events written]      land in `current`; read next frame after the
                         Stage::Input swap, not this frame
```

**Step 4: `Update` workloads** — variable-rate game logic.
`react_to_tierups` runs an `EventReader<CityTierUp>`. Readers see the
**previous** frame's events (the double-buffer swaps at the top of each
frame, in `Stage::Input`), so this reacts to a tierup emitted on frame 41 —
the one `CityTick` just emitted on frame 42 is read on frame 43. It
schedules a tile-banner animation via `Commands`.

```text
   Workload::Reactions
     ↓
   react_to_tierups      ◀── reads last frame's CityTierUp (read-previous)
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

**Step 7: `Last`** — frame finalisation. (Event buffers do **not** rotate
here: the per-type swap runs at the *top* of each frame, in `Stage::Input`.
So this frame's `CityTierUp` write becomes readable when the next frame's
`Input` swap rotates `current` into `previous` — a reader then picks it up
exactly once before it falls off.)

End of frame. Back to Step 1 for frame 43.

## Quick reference: every query shape

| Query | What it iterates | Iteration item |
|-|-|-|
| `Query<&T>` | All entities with `T` | `&T` |
| `Query<&mut T>` | All entities with `T` | `&mut T` |
| `Query<(&A, &B)>` | Entities with both `A` and `B` | `(&A, &B)` |
| `Query<(&mut A, &B)>` | Entities with both | `(&mut A, &B)` |
| `Query<(&A, &B, &C)>` | Entities with all three | `(&A, &B, &C)` |
| `Query<Entity>` ⏳ | Every alive entity | `Entity` |
| `Query<(Entity, &T)>` ⏳ | Entities with `T`, including ID | `(Entity, &T)` |
| `Query<(&T, Option<&U>)>` ⏳ | Entities with `T`, `U` if present | `(&T, Option<&U>)` |
| `Query<&T, With<U>>` | Entities with both `T` and `U`, only fetch `T` | `&T` |
| `Query<&T, Without<U>>` | Entities with `T` but not `U` | `&T` |
| `Query<&T, And<(With<U>, Without<V>)>>` | AND of filters | `&T` |
| `Query<&T, Or<(With<U>, With<V>)>>` | OR of filters | `&T` |

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

## Configuration

`spark-ecs` has no environment variables, no Cargo features, no
runtime knobs. It's stdlib-only by design.

## Using it from an engine crate (`lib/*`)

Engine crates depend on `spark-ecs` directly when they need ECS
items, *not* through `spark-core`:

```toml
[dependencies]
spark-ecs = { path = "../ecs" }
```

`spark-core` does not re-export `World`, `Res`, `ResMut`,
`IntoSystem`, or `SystemParam`. The split keeps the crate boundary
honest: anything that touches the ECS adds a direct edge to it.

## Errors and pitfalls

- **Holding a `Ref` / `RefMut` across a `World` mutation** — the
  guard's lifetime is tied to the `&World` borrow. Hold one, then
  call something that wants `&mut self`, and the borrow checker
  stops you at compile time. If two `RefMut<T>` go live over the
  same type, you get a runtime panic from `RefCell` ("already
  borrowed"). The fix: drop the first guard before taking the
  second, or restructure to take both at once into separate
  variables (different `T`s coexist fine).
- **Stale `Entity` handles** — every `World::*` entity method
  silently returns `None` / `false` on a stale handle. There's no
  panic. If you genuinely need to detect "the handle I had is no
  longer valid", check `world.is_alive(e)` first.
- **`insert` on a never-allocated `Entity`** — same as stale: returns
  `None`, doesn't allocate the slot. Always `world.spawn()` first,
  then `insert`.
- **`despawn` is O(K)** — K = number of component types registered
  in the world. Despawning is still cheap, but it isn't free if you
  end up with hundreds of types and despawn a lot.

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
        │    EngineError)                  │
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
above can pull `World`, `Res`, `ResMut`, `Entity`, and friends
directly without circular-dependency worries.

For the full milestone plan see [`docs/PLAN.md`](../../docs/PLAN.md);
for the engineering rationale behind every decision in this README
see [`docs/ECS_DESIGN.md`](../../docs/ECS_DESIGN.md).
