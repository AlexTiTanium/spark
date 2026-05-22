# spark-ecs-macros

The derive macros behind [`spark-ecs`](../README.md): `#[derive(Component)]`,
`#[derive(Resource)]`, and `#[derive(WorkloadLabel)]`.

> **Why a separate crate?** Rust forces every procedural macro into its
> own `proc-macro = true` crate — such a crate can export *only* macros,
> nothing else. So this crate exists for a compiler rule, not because
> it's a distinct part of the engine. It's a build-time artefact of
> `spark-ecs` and nests inside it at `lib/ecs/macros/`.

## You almost certainly don't depend on this directly

`spark-ecs` re-exports both derives, so downstream code imports them from
there and never names this crate:

```rust,ignore
// `ignore`: this crate can't depend on `spark-ecs` (that would be a
// cycle — the macro crate is a build-time artefact of `spark-ecs`, not
// the reverse), so its own doctests can't import the traits. Runnable
// versions of these examples live in `lib/ecs/README.md`.
use spark_ecs::{Component, Resource, WorkloadLabel};

#[derive(Component)]
struct Position { x: f32, y: f32 }

#[derive(Resource)]
struct FrameCount(u64);

#[derive(WorkloadLabel)]            // applies to an enum — one variant per label
enum Grid { Supply, Distribute }
```

The single outward trace of this crate is one mandatory line in the
workspace `members` list (`"lib/ecs/macros"`) — a workspace requires
every package to be a member, and that can't be elided.

## What the derives generate

`Component` and `Resource` each emit one empty marker `impl`:

```text
#[derive(Component)] struct Position { … }
        ⇓
impl ::spark_ecs::Component for Position {}
```

The trait does the real work. `Component` is declared
`Send + Sync + 'static`, so the generated impl only compiles when the
type is genuinely thread-safe — a field holding an `Rc`, `RefCell`, or
raw pointer is rejected at the derive site. That is the compile-time
proof the M4 parallel scheduler will lean on. `Resource` carries only a
`'static` bound, so non-`Send` resources (GPU handles, OS state) still
derive cleanly; their thread-safety is handled at the scheduler, not the
type system.

`WorkloadLabel` is the exception: instead of an empty marker, it matches
over an enum's unit variants to generate real method bodies — `id()`
returns the enum's `TypeId` paired with the variant index, and `name()`
returns the qualified `"Enum::Variant"` string:

```text
#[derive(WorkloadLabel)] enum Grid { Supply, Distribute }
        ⇓
impl ::spark_ecs::WorkloadLabel for Grid {
    fn id(&self) -> ::spark_ecs::WorkloadId { /* match self → (TypeId, index) */ }
    fn name(&self) -> &'static str          { /* match self → "Grid::Supply" … */ }
}
```

That is why it applies to an *enum*: the scheduler needs one identity and
one name per workload label, and an enum's variants enumerate exactly
those.

The path is emitted fully qualified (`::spark_ecs::…`) so it resolves
from the consumer crate — and from inside `spark-ecs` itself, which
makes that work with `extern crate self as spark_ecs;`.

## Errors / pitfalls

- **`Position: Send` is not satisfied.** A `#[derive(Component)]` on a
  type holding a non-`Send`/`!Sync` field fails here by design. Move the
  non-thread-safe state into a `Resource`, or store a thread-safe handle
  (`Arc` instead of `Rc`).
- **Generic components need their own `'static` bounds.** The derive
  threads generics through but does not add `T: 'static` per parameter;
  a generic component must spell that bound out itself for now.
- **`#[derive(WorkloadLabel)]` only works on enums of unit variants.** A
  struct, a union, or an enum with a tuple/struct variant is a compile
  error — there is no single label to derive from one. Use one enum per
  subsystem, each variant a workload.
