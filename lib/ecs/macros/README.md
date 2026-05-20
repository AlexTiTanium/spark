# spark-ecs-macros

The derive macros behind [`spark-ecs`](../README.md): `#[derive(Component)]`
and `#[derive(Resource)]`.

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
use spark_ecs::{Component, Resource};

#[derive(Component)]
struct Position { x: f32, y: f32 }

#[derive(Resource)]
struct FrameCount(u64);
```

The single outward trace of this crate is one mandatory line in the
workspace `members` list (`"lib/ecs/macros"`) — a workspace requires
every package to be a member, and that can't be elided.

## What the derives generate

Each derive emits one empty marker `impl`:

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
