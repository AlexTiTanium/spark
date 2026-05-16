# spark ECS — issue roadmap & drafts

Consolidates the design decisions into a paste-ready issue plan. Each roadmap
entry lists **Work**, **Fixed decisions** (already chosen, not open for debate),
and **Warnings** (gotchas to honour during implementation). Two issues are
expanded into full drafts in the project's `#10–#12` format below.

---

## Roadmap

### Open issues (in flight)

- **#10 — Entities + component storage.** Sparse-set storage, `World` API.
- **#11 — Queries.** `Query` as `SystemParam`. ⚠ Ships with
  `<&mut T as WorldQuery>::iter_items` left `unimplemented!` — `Query<&mut T>`
  does not actually work yet. Closed by **Draft 1** below.
- **#12 — Commands + frame loop.** Deferred spawn/despawn, per-frame stages.

### To create — in order

**1. Finish `&mut` query iteration** — full draft below (Draft 1).
Precondition for everything else; without it `Query<&mut T>` and the `#12`
movement demo do not run. Must merge before the scheduler.

**2. derive(Component/Resource) + `Send+Sync` + drop prelude** — draft below (Draft 2).
- *Decisions:* explicit derive over blanket impl; traits become
  `Send + Sync + 'static`; no `prelude` module.
- *Warnings:* breaking-ish refactor, touches every component/resource in
  demos+tests — do it now while they are few.

**3. Scheduler / workload.**
- *Work:* `Access` declaration on every `SystemParam`; conflict detection;
  explicit `.before()/.after()`; topo-sorted DAG; system batching.
  Executor is **sequential** but the DAG/batch structure is built now.
- *Decisions:* **B2** (explicit ordering + topo-sort, not registration order);
  **C1** (`Access` + DAG now, parallel executor committed for M4).
- *Warnings:* `Access` is the **safety proof** for M4 lockless parallelism, not
  just an ambiguity check — it must be complete and correct from day one.
  All storage access must funnel through one chokepoint so the M4
  `RefCell → UnsafeCell` swap stays local. `world_mut()` must remain
  unreachable from inside a system. Batches must exist even though the
  executor walks them sequentially.

**4. Query filters — `With<T>` / `Without<T>`.**
- *Work:* `QueryFilter` trait; `With`/`Without` impls; `Query` gets a filter param.
- *Decision:* **A2** — separate trait, second generic: `Query<'w, D, F = ()>`.
- *Warnings:* `With<U>` contributes a **read** of `U` to the `Access` model —
  filters must report into `collect_access`. `Or` is deferred.

**5. Query tuple arity 3–4.**
- *Work:* extend the `macro_rules!` arity list.
- *Warnings:* reuse the arity macro shared with `IntoSystem` (see #11) — do not
  hand-write the 3- and 4-tuple impls.

**6. `Time` + `WindowSize` resources.**
- *Work:* `Time { delta, elapsed }` updated each frame in `EventLoopRunner`;
  `WindowSize` updated on `WindowEvent::Resized`.
- *Warnings:* movement must use `delta` — the `#12` demo's `p.x += v.x` is
  FPS-dependent and is a bug. `WindowSize` as a resource lets render react to
  resize without the full event system.

**7. Component change-tick storage slot.**
- *Work:* add `changed_tick: Vec<u32>` to `ComponentStorage`, parallel to
  `dense`; set on `insert` / `get_mut`.
- *Warnings:* `changed_tick` must be `swap_remove`d in lockstep with
  `dense`/`entity_index` (same discipline as #10). Reserve the slot only — the
  `Changed<T>` filter itself is a fast-follow after render, do not implement it
  here. Needs a `World`-level tick counter.

**8. Bundles.**
- *Work:* `Bundle` trait; tuple impl via `macro_rules!`; `#[derive(Bundle)]`.
- *Warnings:* define overwrite semantics — inserting a bundle component an
  entity already has should overwrite (consistent with `World::insert`).

**Then: Render milestone** — does not need parallelism.

**Then: M4 — parallelism (committed, not optional).**
`World` becomes `Sync`; storages `RefCell → UnsafeCell` under the scheduler's
proven-disjoint access; `EntityAllocator` thread-safe; per-system
`CommandQueue` merged at flush; thread-pool executor; `par_iter()`.

---

## Draft 1 — spark-ecs: finish `Query<&mut T>` iteration — `WorldQuery` shared/exclusive split (M3 Issue B-fix)

### Context

#11 landed `Query` as a `SystemParam`, but `<&mut T as WorldQuery>::iter_items`
is `unimplemented!`. The cause is the trait signature: `iter_items` takes
`&State`, and `&mut T` items cannot be produced from a shared borrow of the
state. Until this is fixed `Query<&mut T>` — and therefore the canonical
`movement` system from #12 — does not run.

This is a **precondition for M3.5**: it must merge before the scheduler issue.

### Goals & non-goals

**Goals**

- Change `WorldQuery` iteration so `&mut T` items are reachable: exclusive
  iteration takes `&mut State`.
- `ReadOnlyWorldQuery` marker subtrait that gates `Query::iter(&self)`.
- `Query::iter_mut(&mut self)` works for any `D`; `Query::iter(&self)` exists
  only for read-only `D`.
- Working iteration for `&T`, `&mut T`, `(&A, &B)`, `(&mut A, &B)`, `(&A, &mut B)`.
- **No `unsafe`** in `query.rs`.

**Non-goals**

- `(&mut A, &mut B)` — two mutable joins; cannot be composed from two
  `iter_mut` walks without aliasing reasoning. Out of scope.
- Change detection on `&mut` access (`Mut<T>` wrapper) — arrives with the
  change-tick slot (roadmap item 7).
- `par_iter` — M4.
- Tuple arity 3–4 — roadmap item 5.

### Proposed shape

`lib/ecs/src/query.rs`

```rust
pub trait WorldQuery {
    type Item<'w>;
    type State<'w>;

    fn init_state(world: &World) -> Self::State<'_>;

    /// Exclusive iteration. Takes `&mut State` so `&mut T` items are reachable.
    /// Lifetimes are decoupled — `'s` is the short borrow, State keeps its own.
    fn iter<'s>(
        state: &'s mut Self::State<'_>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>;
}

/// Implemented only by queries that borrow nothing mutably.
/// This is what gates `Query::iter(&self)`.
pub trait ReadOnlyWorldQuery: WorldQuery {
    fn iter_ref<'s>(
        state: &'s Self::State<'_>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>;
}

impl<T: Component> WorldQuery for &T {
    type Item<'w> = &'w T;
    type State<'w> = Ref<'w, ComponentStorage<T>>;
    fn init_state(world: &World) -> Self::State<'_> { /* world.storage::<T>() */ }
    fn iter<'s>(state: &'s mut Self::State<'_>)
        -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's> {
        Box::new(state.iter())
    }
}
impl<T: Component> ReadOnlyWorldQuery for &T {
    fn iter_ref<'s>(state: &'s Self::State<'_>)
        -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's> {
        Box::new(state.iter())
    }
}

impl<T: Component> WorldQuery for &mut T {
    type Item<'w> = &'w mut T;
    type State<'w> = RefMut<'w, ComponentStorage<T>>;
    fn init_state(world: &World) -> Self::State<'_> { /* world.storage_mut::<T>() */ }
    fn iter<'s>(state: &'s mut Self::State<'_>)
        -> Box<dyn Iterator<Item = (Entity, &'s mut T)> + 's> {
        // Trivial once the signature is &mut State: delegate to the storage.
        Box::new(state.iter_mut())
    }
}
// No ReadOnlyWorldQuery for &mut T.

// Tuple (D1, D2): join. Drive whichever side is mutable via its `iter`,
// look the other side up per-entity. (&mut A, &mut B) is excluded.
impl<D1: WorldQuery, D2: WorldQuery> WorldQuery for (D1, D2) {
    type Item<'w>  = (D1::Item<'w>, D2::Item<'w>);
    type State<'w> = (D1::State<'w>, D2::State<'w>);
    // init_state: (D1::init_state(world), D2::init_state(world))
    // iter: split `&mut state` into the two sub-states; drive the mut side's
    //       iterator; per yielded entity, look up the read side; emit when both
    //       are present. Detail in the PR.
}

impl<'w, D: WorldQuery> Query<'w, D> {
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, D::Item<'_>)> {
        D::iter(&mut self.state)
    }
}
impl<'w, D: ReadOnlyWorldQuery> Query<'w, D> {
    pub fn iter(&self) -> impl Iterator<Item = (Entity, D::Item<'_>)> {
        D::iter_ref(&self.state)
    }
}
```

### Architecture diagram

```
Query<(&mut Position, &Velocity)>::iter_mut(&mut self)
    │
    ▼  split &mut (RefMut<Storage<Position>>, Ref<Storage<Velocity>>)
    ├── pos_state : &mut RefMut<Storage<Position>>   ── drive ──┐
    └── vel_state : &Ref<Storage<Velocity>>          ── look up ┤
                                                                ▼
   for (e, &mut pos) in pos_state.iter_mut():        ← safe: ComponentStorage::iter_mut
       if let Some(vel) = vel_state.get(e):          ← safe: many shared &Velocity
           yield (e, (&mut pos, &vel))

Position storage borrowed &mut, Velocity storage borrowed & — different
RefCells, no conflict. &mut Position items are distinct (slice::iter_mut).
```

### Learn

- **Why `&State` can't yield `&mut T`.** `&mut` requires a unique borrow;
  a shared `&State` can never produce one. The fix is purely the signature —
  exclusive iteration takes `&mut State`, and the single-component `&mut T`
  impl becomes a one-line delegate to `ComponentStorage::iter_mut`.
- **Why no `unsafe` is needed for M3 shapes.** `ComponentStorage::iter_mut`
  is built on `slice::iter_mut`, whose soundness (distinct `&mut` to distinct
  elements) is std's responsibility. A mixed tuple drives one safe `iter_mut`
  and reads a *different* storage — disjoint `RefCell`s, no aliasing.
- **The shared/exclusive split.** A query that borrows nothing mutably is
  `ReadOnlyWorldQuery` and may be iterated through `&self`. A query with any
  `&mut` is `WorldQuery`-only and needs `&mut self`. This is a simplified form
  of bevy's read-only-query machinery.

### Warnings (implementation)

- **The `&'a mut State<'a>` trap.** Tying the borrow lifetime to the State
  lifetime (`&'a mut Self::State<'a>`) makes the mutable borrow last as long as
  the data — the state is then locked forever. Use decoupled lifetimes:
  `&'s mut Self::State<'_>`, with the yielded `Item<'s>` tied to `'s`.
- **Lending-iterator boundary.** A normal `Iterator` cannot yield items that
  borrow the iterator itself. `ComponentStorage::iter_mut` works because its
  `Item` lifetime is the *fixed* `&mut self` borrow (slice-iter pattern), not
  tied to `next()`. Keep the same shape — do not invent a `next(&mut self)`
  that yields borrows of `self`.
- **Tuple borrow split.** Destructure `&mut state` into the two sub-states
  (`let (a, b) = &mut *state;`) so the A and B borrows split cleanly; driving
  one and reading the other must not re-borrow the same field.
- **Drive the mutable side.** For `(&mut A, &B)` the mut side must be the
  driver (it needs `iter_mut`); the read side is looked up per-entity. The
  leading-storage `min` optimisation from #11 is a follow-up — correctness via
  "drive first sub-query, look up the rest" is acceptable here.
- **`(&mut A, &mut B)` is excluded** — make it not compile rather than
  half-work. It needs aliasing reasoning that belongs with M4.
- **`Box<dyn Iterator>` allocates per `iter`/`iter_mut` call** — once per system
  per stage, negligible; keep it (consistent with #11).
- **M4 chokepoint.** This iteration path runs through `RefMut` — the future
  `RefCell → UnsafeCell` migration point. Keep storage access funnelled through
  `WorldQuery`; systems must never touch storages directly.

### File-tree diff

```
lib/ecs/src/
└── query.rs   (modified — WorldQuery::iter takes &mut State; ReadOnlyWorldQuery;
                 impls for &T / &mut T / (D1,D2); Query iter / iter_mut blocks)
```

### Acceptance criteria

- [ ] `WorldQuery` exclusive iteration takes `&mut State`; lifetimes decoupled.
- [ ] `ReadOnlyWorldQuery` implemented for `&T` and read-only tuples; gates `Query::iter`.
- [ ] `Query<&mut T>::iter_mut` yields `&mut T`; the #12 `movement` demo runs.
- [ ] `(&mut A, &B)` and `(&A, &mut B)` iterate correctly.
- [ ] `(&mut A, &mut B)` does not compile.
- [ ] No `unsafe` anywhere in `query.rs`.
- [ ] Doc tests for `iter`, `iter_mut`, and the mixed-tuple join.
- [ ] PR uses the project template with diagram + Learn + Warnings filled in.

### Verification plan

1. `cargo fmt --all -- --check`
2. `cargo build --workspace`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo test --workspace` — doc tests + a unit test asserting a
   `(&mut Position, &Velocity)` pass updates exactly the entities with both.
5. `cargo run -p spark` — the movement demo actually moves.

### Forward evolution

| Where next | Adds |
| --- | --- |
| This issue | Working `iter` / `iter_mut`; shared/exclusive split |
| Scheduler (item 3) | Depends on this — `Query` must iterate before systems batch |
| Change-tick (item 7) | `&mut` access later wrapped in `Mut<T>` for change marking |
| M4 | `par_iter`; `(&mut A, &mut B)` revisited under proven-disjoint access |

### Out of scope

`(&mut A, &mut B)`; change detection; `par_iter`; tuple arity 3–4;
leading-storage `min` optimisation (follow-up).

---

## Draft 2 — spark-ecs: derive(Component/Resource) + `Send+Sync` bound + drop prelude

### Context

Fixed design decision: component and resource membership is **explicit via a
derive**, not a blanket impl. The blanket impl is a coherence dead-end and a
footgun — a resource type silently satisfies `Query`. The same refactor moves
the traits to `Send + Sync + 'static` ahead of the M4 parallel scheduler, and
removes the `prelude` module in favour of explicit imports.

### Goals & non-goals

**Goals**

- New proc-macro crate `spark_ecs_macros`.
- `#[derive(Component)]` and `#[derive(Resource)]`.
- `Component` / `Resource` traits become `Send + Sync + 'static`; the blanket
  impl is removed.
- `prelude` module removed; flat `pub use` re-exports at the crate root.
- Every existing component/resource in demos and tests updated.

**Non-goals**

- `#[component(...)]` helper attributes — added later, non-breaking.
- `#[derive(Bundle)]` / `#[derive(SystemParam)]` — roadmap item 8 / post-render.
- `'static` bound injection for generic components — small follow-up.

### Proposed shape

`lib/ecs-macros/Cargo.toml`

```toml
[package]
name = "spark_ecs_macros"
edition = "2021"

[lib]
proc-macro = true

[dependencies]
syn = "2"
quote = "1"
proc-macro2 = "1"
```

`lib/ecs-macros/src/lib.rs`

```rust
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput};

#[proc_macro_derive(Component)]
pub fn derive_component(input: TokenStream) -> TokenStream {
    impl_marker(input, quote!(::spark_ecs::Component))
}

#[proc_macro_derive(Resource)]
pub fn derive_resource(input: TokenStream) -> TokenStream {
    impl_marker(input, quote!(::spark_ecs::Resource))
}

fn impl_marker(input: TokenStream, trait_path: proc_macro2::TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let (impl_g, ty_g, where_c) = input.generics.split_for_impl();
    quote! { impl #impl_g #trait_path for #name #ty_g #where_c {} }.into()
}
```

`lib/ecs/src/lib.rs`

```rust
extern crate self as spark_ecs;   // so the generated ::spark_ecs path resolves here too

pub trait Component: Send + Sync + 'static {}
pub trait Resource:  Send + Sync + 'static {}

pub use spark_ecs_macros::{Component, Resource};   // trait + derive share the name

mod query;
mod commands;
pub use query::Query;
pub use commands::Commands;
// no `prelude` module
```

### Warnings (implementation)

- A `proc-macro = true` crate can export **only** proc macros — that is why
  `spark_ecs_macros` is a separate crate.
- `extern crate self as spark_ecs;` is required so the generated
  `::spark_ecs::Component` path resolves when the derive is used *inside*
  `spark_ecs` itself.
- `split_for_impl()` is mandatory for correct generics/lifetimes. Generic
  components additionally need a `T: 'static` bound added per type parameter —
  out of scope here; concrete components are unaffected.
- The `Send + Sync` bound will reject components/resources holding `Rc`,
  `RefCell`, or raw pointers **at the derive site** — this is intended.
- Dependency direction is one-way: `spark_ecs → spark_ecs_macros`. The macro
  crate does **not** depend on `spark_ecs`; the emitted path resolves in the
  consumer crate. No cycle.
- This is a **breaking-ish refactor**: it touches every component/resource
  definition in demos and tests, and every `use` line that referenced the
  prelude. Do it now while those are few.

### File-tree diff

```
lib/ecs-macros/          (NEW crate — spark_ecs_macros)
├── Cargo.toml
└── src/lib.rs
lib/ecs/
├── Cargo.toml           (modified — dependency on spark_ecs_macros)
└── src/lib.rs           (modified — Send+Sync traits; no blanket impl; no prelude)
Cargo.toml               (modified — new workspace member)
```

### Acceptance criteria

- [ ] `spark_ecs_macros` crate exists with `proc-macro = true`.
- [ ] `#[derive(Component)]` / `#[derive(Resource)]` generate the marker impls.
- [ ] `Component` / `Resource` are `Send + Sync + 'static`; blanket impl gone.
- [ ] `prelude` removed; imports are explicit throughout demos and tests.
- [ ] Workspace builds; clippy clean; all demos updated and run.
- [ ] A component holding a non-`Send` field fails to derive (compile-fail test, optional).

### Verification plan

1. `cargo fmt --all -- --check`
2. `cargo build --workspace`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo test --workspace`
5. `cargo run -p spark` — existing behaviour unchanged.

### Forward evolution

| Where next | Adds |
| --- | --- |
| This issue | `derive(Component/Resource)`, `Send+Sync` bound, explicit imports |
| Later | `#[proc_macro_derive(Component, attributes(component))]` for config — non-breaking |
| Item 8 / post-render | `#[derive(Bundle)]`, `#[derive(SystemParam)]` |

### Out of scope

`#[component(...)]` attributes; `Bundle` / `SystemParam` derives; generic
`'static` bound injection.
