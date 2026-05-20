# spark ECS — issue roadmap & drafts

Consolidates the design decisions into a paste-ready issue plan. Each roadmap
entry lists **Work**, **Fixed decisions** (already chosen, not open for debate),
and **Warnings** (gotchas to honour during implementation). Two issues are
expanded into full drafts in the project's `#10–#12` format below.

---

## Roadmap

### Shipped

- **#10 → PR #20 — Entities + component storage.** Sparse-set storage, `World` API.
- **#11 → PR #22 — Queries.** `Query` as `SystemParam`. Ships the
  **`QueryData` shared/exclusive split** (`ReadOnlyQueryData` marker,
  `iter` / `iter_mut`, `&mut T` impl, mut-driver tuples, read-tuple
  arity 2/3/4) — i.e. Draft 1 below is done, and roadmap item 5's
  read-tuple arities. **#26 then extends item 5** to every `&` / `&mut`
  combination via `impl_all_tuple!` (see item 5 below). Trait surface:
  [`lib/ecs/src/query.rs`](../lib/ecs/src/query.rs).
- **#12 → PR #24 — Commands + frame loop.** Deferred spawn/despawn,
  per-frame stages, `WindowPlugin` runner owns `Application`.

### In flight

- **#26 — Multi-mut query joins + self-conflict detection.** The
  legitimate remainder of the original Draft-3 plan: `(&mut A, &mut B)` via
  one localised `unsafe fn`, plus `QueryAccess` and
  `assert_no_self_conflict`. Body in Draft 3 below.

### To create — in order

> **Status legend** — ✅ Done in main · 🟡 Filed (in flight) · ⬜ Not yet filed.

**1. ~~Finish `&mut` query iteration~~ — ✅ DONE in main (PR #22).**
The `QueryData` shared/exclusive split, `ReadOnlyQueryData` gate,
`Query::iter` / `iter_mut`, and `&mut T` impl all landed with the
`Query as SystemParam` work. Draft 1 below is preserved for archaeology;
do not refile it. Originally filed as #25 and closed as stale.

**1b. Multi-mut query joins + query self-conflict detection — 🟡 #26.**
- *Work:* `(&mut A, &mut B)` (and wider mut tuples) via a small, localised
  `unsafe` block; query-level access collection on `QueryData`; reject any
  query where one component `TypeId` appears twice with a `&mut`
  (`(&mut A, &mut A)`, `(&mut A, &A)`).
- *Decision:* `unsafe` chosen over the safe `for_each` form — it gives a
  uniform real `iter_mut()` and matches bevy/hecs/shipyard. The `unsafe` is
  made concern-free by pairing it with the self-conflict check, not by hope.
- *Soundness contract:* the `unsafe` hands out `&mut` only because (1) the
  query yields each entity **at most once** — structural, the driving storage
  lists each entity once and storages are exclusively borrowed for the whole
  iteration so no structural mutation can occur mid-walk; (2) the same
  component never appears twice mutably — **enforced** by the self-conflict
  check. Distinct entity → distinct dense slot; distinct component → distinct
  storage; therefore no two live `&mut` ever alias.
- *Warnings:* in M3 the same-storage case (`(&mut A, &A)` etc.) is *also*
  caught by the `RefCell` double-borrow panic — the explicit check is a
  nicer diagnostic now and the **required** guard once M4 swaps storages to
  `UnsafeCell` (no `RefCell` backstop then). Keep the `unsafe` block minimal
  and commented with this contract. The query-level access collection
  introduced here is the primitive the scheduler (item 3) extends to
  `SystemParam` level — design it to be reused, not rewritten.

**2. derive(Component/Resource) + `Send+Sync` + drop prelude — ✅ shipped with #29.** Draft below (Draft 2).
- *Decisions:* explicit derive over blanket impl; the proc-macro crate is
  **nested inside the ECS crate** at `lib/ecs/macros/`, not a top-level
  workspace sibling. `Component` carries `Send + Sync + 'static`;
  **`Resource` carries only `'static`** — a deliberate divergence from the
  original draft (rationale in the Draft 2 callout below). There was no
  `prelude` module to drop — the crate already used flat re-exports, so
  that sub-goal was a no-op.
- *Shipped:* `spark-ecs-macros` (`#[derive(Component)]` /
  `#[derive(Resource)]`), blanket impl removed, resource APIs
  (`add_resource` / `Res` / `ResMut`) gated on `Resource`, every demo and
  test migrated to the derives.

**3. Scheduler / workload — ⬜ not filed.**
- *Work:* `Access` declaration on every `SystemParam` (aggregating the
  `QueryAccess` primitive from issue 1b); conflict detection; explicit
  `.before()/.after()`; topo-sorted DAG; system batching. Executor is
  **sequential** but the DAG/batch structure is built now.
- *Stage-shape migration:* replace the M1–M3 stand-in
  (`pub mod stages { pub const STARTUP: &str = "startup"; … }` in
  `spark-core`) with the canonical, **closed** `pub enum Stage { Startup, First,
  PreUpdate, FixedUpdate, Update, PostUpdate, Render, Last }` from
  ECS_DESIGN.md. `add_system(stages::FOO, …)` becomes
  `add_systems(Stage::Foo, …)`. The enum gives compile-time
  exhaustiveness for the editor's stage view and removes the
  stringly-typed footgun. `Stage` is **closed**: one frame timeline means
  one shared phase set, so a subsystem extends the frame via *workloads*,
  not new stages. A genuinely new global phase is a deferred,
  non-breaking upgrade — widen to `impl StageLabel`, and existing
  `Stage::Foo` calls still compile. Existing callers (#9, #12) are
  updated in the same PR.
- *Workload labels:* introduce a `WorkloadLabel` trait + `#[derive(WorkloadLabel)]`
  (proc-macro in `spark-ecs-macros`), applied to a *per-subsystem enum* — the
  derive matches over its variants to generate per-variant identity + name —
  and `app.add_workload(label, Stage::Foo, |w| {…})`
  per ECS_DESIGN.md. Workloads are how plugins group related systems
  under a name; they sit *inside* a `Stage` and are the granularity
  the scheduler topo-sorts and batches.
- *Decisions:* **B2** (explicit ordering + topo-sort, not registration order);
  **C1** (`Access` + DAG now, parallel executor committed for M4).
- *Warnings:* `Access` is the **safety proof** for M4 lockless parallelism, not
  just an ambiguity check — it must be complete and correct from day one.
  All storage access must funnel through one chokepoint so the M4
  `RefCell → UnsafeCell` swap stays local. `world_mut()` must remain
  unreachable from inside a system. Batches must exist even though the
  executor walks them sequentially. The `stages::` constants migration is
  cross-cutting — touches `spark-core`, `spark-window`'s `EventLoopRunner`,
  the binary, and every doc test that calls `add_system`.

**4. Query filters — `With<T>` / `Without<T>` — ⬜ not filed.**
> ⚠️ **Discuss before opening an implementation issue.** The design
> sketch below (`Query<'w, D, F = ()>` with a separate `QueryFilter`
> trait) is one option, not a final decision. Open questions: filter
> composition (tuples vs explicit `And<…>`), where `Or` fits without
> wedging the API, whether filters should be a *third* generic on
> `Query` or fold into `D` (`Query<(&Plant, With<Operational>)>`),
> how filter access interacts with the scheduler's per-system access
> aggregation, and whether the syntax should mirror Bevy's exactly or
> diverge for clarity. Resolve these in a design doc / draft issue
> first; the implementation issue lands after.
- *Work (tentative):* `QueryFilter` trait; `With`/`Without` impls;
  `Query` gets a filter param.
- *Decision (tentative):* **A2** — separate trait, second generic:
  `Query<'w, D, F = ()>`.
- *Warnings:* `With<U>` contributes a **read** of `U` to the `Access`
  model — filters must report into `collect_access`. `Or` is deferred.

**5. ~~Query tuple arity 3–4~~ — ✅ DONE (PR #22, extended by #26).**
Read-only tuple arities 2/3/4 shipped alongside #11. **#26 then
replaced the per-arity `impl_query_data_tuple!` macros with the
unified `impl_all_tuple!`**, which Cartesian-products the `&` / `&mut`
flags to cover *every* combination at arities 2–5 (read, mixed,
mut-not-first, and fully-mutable multi-mut). Extending to arity 6+ is
one line — `impl_all_tuple!(A, B, C, D, E, F);` — with no new
mechanism (monomorphisation cost doubles per step).

**6. `Time` + `WindowSize` resources — ⬜ not filed.**
- *Work:* `Time { delta, elapsed }` updated each frame in `EventLoopRunner`;
  `WindowSize` updated on `WindowEvent::Resized`.
- *Warnings:* movement must use `delta` — the `#12` demo's `p.x += v.x` is
  FPS-dependent and is a bug. `WindowSize` as a resource lets render react to
  resize without the full event system.

**7. Component change-tick storage slot — ⬜ not filed.**
- *Work:* add `changed_tick: Vec<u32>` to `ComponentStorage`, parallel to
  `dense`; set on `insert` / `get_mut`.
- *Warnings:* `changed_tick` must be `swap_remove`d in lockstep with
  `dense`/`entity_index` (same discipline as #10). Reserve the slot only — the
  `Changed<T>` filter itself is a fast-follow after render, do not implement it
  here. Needs a `World`-level tick counter.

**8. Bundles — ⬜ not filed.**
- *Work:* `Bundle` trait; tuple impl via `macro_rules!`; `#[derive(Bundle)]`.
- *Warnings:* define overwrite semantics — inserting a bundle component an
  entity already has should overwrite (consistent with `World::insert`).

**9. `IntoIterator` for `&Query` / `&mut Query` (loop sugar) — ⬜ not filed.**
- *Work:* impl `IntoIterator` for `&Query<'_, D>` (yields `D::Item` via
  `iter_ref`, bound `D: ReadOnlyQueryData`) and for `&mut Query<'_, D>`
  (yields `D::Item` via `iter`, any `D: QueryData`). Lets systems write
  `for x in &q` / `for (pos, vel) in &mut q` instead of
  `for x in q.iter()` / `for (pos, vel) in q.iter_mut()`. Path B is
  preserved — yielded items carry no `Entity` prefix, exactly matching
  the existing `iter` / `iter_mut`.
- *Decision:* **additive, not a replacement.** Keep `iter` / `iter_mut`
  — they read clearer at call sites and are required for adapter chains
  (`q.iter().map(…).filter(…)`). `IntoIterator` is sugar for the bare
  `for` loop only. `IntoIter` is the same `Box<dyn Iterator<Item =
  D::Item<'a>> + 'a>` the trait methods already return, so no new
  iterator type is introduced.
- *Warnings:* **do not** impl `IntoIterator for Query` by value — that
  consumes the query and drops its `Ref` / `RefMut` storage guards
  mid-iteration. Only the `&Query` / `&mut Query` reference forms are
  sound. The `&Query` impl must carry the `D: ReadOnlyQueryData` bound
  (same gate as `Query::iter`) so a `&mut`-containing shape can't be
  iterated through a shared borrow. Small, self-contained, non-blocking
  — can ride along with any query-touching PR rather than waiting in
  line.

**Then: Render milestone** — does not need parallelism.

**Then: M4 — parallelism (committed, not optional).**
`World` becomes `Sync`; storages `RefCell → UnsafeCell` under the scheduler's
proven-disjoint access; `EntityAllocator` thread-safe; per-system
`CommandQueue` merged at flush; thread-pool executor; `par_iter()`.

---

## Draft 1 — spark-ecs: finish `Query<&mut T>` iteration — `QueryData` shared/exclusive split (M3 Issue B-fix)

> ✅ **DONE in main (PR #22).** Preserved for historical context — do not
> refile. Originally filed as #25 and closed as stale. The shared/exclusive
> split, `ReadOnlyQueryData` gate, mut-driver tuples, and read-tuple arity
> 2/3/4 are all live in [`lib/ecs/src/query.rs`](../lib/ecs/src/query.rs).
> Note the tuple-impl mechanism this draft sketches (`impl_query_data_tuple!`)
> was later replaced by `impl_all_tuple!` in #26 — see roadmap item 5.

### Context

#11 landed `Query` as a `SystemParam`, but `<&mut T as QueryData>::iter_items`
is `unimplemented!`. The cause is the trait signature: `iter_items` takes
`&State`, and `&mut T` items cannot be produced from a shared borrow of the
state. Until this is fixed `Query<&mut T>` — and therefore the canonical
`movement` system from #12 — does not run.

This is a **precondition for M3.5**: it must merge before the scheduler issue.

### Goals & non-goals

**Goals**

- Change `QueryData` iteration so `&mut T` items are reachable: exclusive
  iteration takes `&mut State`.
- `ReadOnlyQueryData` marker subtrait that gates `Query::iter(&self)`.
- `Query::iter_mut(&mut self)` works for any `D`; `Query::iter(&self)` exists
  only for read-only `D`.
- Working iteration for `&T`, `&mut T`, `(&A, &B)`, `(&mut A, &B)`, `(&A, &mut B)`.
- **No `unsafe`** in `query.rs`.

**Non-goals**

- `(&mut A, &mut B)` and wider mut joins — deferred to the dedicated issue
  **1b**, not because they are unsolvable but to keep this precondition small
  and `unsafe`-free. 1b adds them with a localised `unsafe` plus the
  self-conflict check. Note this is a *single-threaded* iteration concern —
  it does **not** wait for M4.
- Change detection on `&mut` access (`Mut<T>` wrapper) — arrives with the
  change-tick slot (roadmap item 7).
- `par_iter` — M4.
- Tuple arity 3–4 — roadmap item 5.

### Proposed shape

`lib/ecs/src/query.rs`

```rust
pub trait QueryData {
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
pub trait ReadOnlyQueryData: QueryData {
    fn iter_ref<'s>(
        state: &'s Self::State<'_>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>;
}

impl<T: Component> QueryData for &T {
    type Item<'w> = &'w T;
    type State<'w> = Ref<'w, ComponentStorage<T>>;
    fn init_state(world: &World) -> Self::State<'_> { /* world.storage::<T>() */ }
    fn iter<'s>(state: &'s mut Self::State<'_>)
        -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's> {
        Box::new(state.iter())
    }
}
impl<T: Component> ReadOnlyQueryData for &T {
    fn iter_ref<'s>(state: &'s Self::State<'_>)
        -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's> {
        Box::new(state.iter())
    }
}

impl<T: Component> QueryData for &mut T {
    type Item<'w> = &'w mut T;
    type State<'w> = RefMut<'w, ComponentStorage<T>>;
    fn init_state(world: &World) -> Self::State<'_> { /* world.storage_mut::<T>() */ }
    fn iter<'s>(state: &'s mut Self::State<'_>)
        -> Box<dyn Iterator<Item = (Entity, &'s mut T)> + 's> {
        // Trivial once the signature is &mut State: delegate to the storage.
        Box::new(state.iter_mut())
    }
}
// No ReadOnlyQueryData for &mut T.

// Tuple (D1, D2): join. Drive whichever side is mutable via its `iter`,
// look the other side up per-entity. (&mut A, &mut B) is excluded.
impl<D1: QueryData, D2: QueryData> QueryData for (D1, D2) {
    type Item<'w>  = (D1::Item<'w>, D2::Item<'w>);
    type State<'w> = (D1::State<'w>, D2::State<'w>);
    // init_state: (D1::init_state(world), D2::init_state(world))
    // iter: split `&mut state` into the two sub-states; drive the mut side's
    //       iterator; per yielded entity, look up the read side; emit when both
    //       are present. Detail in the PR.
}

impl<'w, D: QueryData> Query<'w, D> {
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, D::Item<'_>)> {
        D::iter(&mut self.state)
    }
}
impl<'w, D: ReadOnlyQueryData> Query<'w, D> {
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
  `ReadOnlyQueryData` and may be iterated through `&self`. A query with any
  `&mut` is `QueryData`-only and needs `&mut self`. This is a simplified form
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
- **`(&mut A, &mut B)` is simply not implemented in this issue** — the tuple
  impl for two mutable sides lands in issue 1b. Until then it fails to compile
  for lack of an impl; that is an absence, *not* a designed rejection. Do not
  add a deliberate negative impl to block it — 1b will add the real one.
- **`Box<dyn Iterator>` allocates per `iter`/`iter_mut` call** — once per system
  per stage, negligible; keep it (consistent with #11).
- **M4 chokepoint.** This iteration path runs through `RefMut` — the future
  `RefCell → UnsafeCell` migration point. Keep storage access funnelled through
  `QueryData`; systems must never touch storages directly.

### File-tree diff

```
lib/ecs/src/
└── query.rs   (modified — QueryData::iter takes &mut State; ReadOnlyQueryData;
                 impls for &T / &mut T / (D1,D2); Query iter / iter_mut blocks)
```

### Acceptance criteria

- [ ] `QueryData` exclusive iteration takes `&mut State`; lifetimes decoupled.
- [ ] `ReadOnlyQueryData` implemented for `&T` and read-only tuples; gates `Query::iter`.
- [ ] `Query<&mut T>::iter_mut` yields `&mut T`; the #12 `movement` demo runs.
- [ ] `(&mut A, &B)` and `(&A, &mut B)` iterate correctly.
- [ ] `(&mut A, &mut B)` fails to compile here (no impl yet) — added in issue 1b.
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
| Issue 1b | `(&mut A, &mut B)` via `unsafe` + query self-conflict detection |
| Scheduler (item 3) | Depends on this — `Query` must iterate before systems batch; reuses 1b's access primitive |
| Change-tick (item 7) | `&mut` access later wrapped in `Mut<T>` for change marking |
| M4 | `par_iter` only |

### Out of scope

`(&mut A, &mut B)` and query self-conflict detection (both → issue 1b);
change detection; `par_iter`; tuple arity 3–4; leading-storage `min`
optimisation (follow-up).

---

## Draft 2 — spark-ecs: derive(Component/Resource) + `Send+Sync` bound + drop prelude

> ✅ **Shipped with #29.** Kept as the design record. One decision changed
> during implementation — see the divergence callout below.

> ⚠️ **Divergence from this draft (decided during #29).** Only `Component`
> became `Send + Sync + 'static`. **`Resource` carries only `'static`** — not
> `Send + Sync`. Resources are the home for inherently non-thread-safe
> singletons (a `wgpu` surface, an OS handle, an `Rc`-based cache); forcing
> `Send + Sync` would lock those out of the `World` with no escape hatch.
> Parallel-safety for resources is deferred to the M4 scheduler, which will
> (Bevy-style: `Resource` vs `NonSend`) keep a system that touches a
> non-`Send` resource on the main thread rather than rejecting it at the type
> level. The explicit-derive *membership* goal is still met for both traits.
> Also: there was no `prelude` module to remove — the crate already used flat
> re-exports.

### Context

Fixed design decision: component and resource membership is **explicit via a
derive**, not a blanket impl. The blanket impl is a coherence dead-end and a
footgun — a resource type silently satisfies `Query`. The same refactor moves
`Component` to `Send + Sync + 'static` ahead of the M4 parallel scheduler
(`Resource` stays `'static`; see the divergence callout above), and the
crate uses flat re-exports rather than a `prelude` module.

### Goals & non-goals

**Goals**

- New proc-macro crate `spark-ecs-macros` (Rust import path: `spark_ecs_macros`).
- `#[derive(Component)]` and `#[derive(Resource)]`.
- `Component` becomes `Send + Sync + 'static`; `Resource` becomes `'static`
  (see divergence callout); the blanket `Component` impl is removed.
- Flat `pub use` re-exports at the crate root (no `prelude` module existed).
- Every existing component/resource in demos and tests updated.

**Non-goals**

- `#[component(...)]` helper attributes — added later, non-breaking.
- `#[derive(Bundle)]` / `#[derive(SystemParam)]` — roadmap item 8 / post-render.
- `'static` bound injection for generic components — small follow-up.

### Proposed shape

`lib/ecs/macros/Cargo.toml`  *(nested inside the ECS crate)*

```toml
[package]
name = "spark-ecs-macros"
version = "0.1.0"

# Inherited from [workspace.package] in the root manifest.
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[lib]
proc-macro = true

# Add these to [workspace.dependencies] in the root Cargo.toml as
# exact pinned versions (project convention — never caret ranges).
# Look up the latest stable at implementation time.
[dependencies]
syn = { workspace = true, features = ["full"] }
quote.workspace = true
proc-macro2.workspace = true
```

`lib/ecs/macros/src/lib.rs`

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

`lib/ecs/Cargo.toml`

```toml
[dependencies]
spark-ecs-macros = { path = "macros" }   # relative — the crate is nested inside lib/ecs/
```

### Warnings (implementation)

- A `proc-macro = true` crate can export **only** proc macros — so
  `spark-ecs-macros` must be a separate crate; this is unavoidable. It is
  **nested at `lib/ecs/macros/`** (not a top-level workspace sibling) so it
  stays organizationally owned by `spark-ecs`. The only outward trace is one
  `members` entry in the workspace `Cargo.toml`; that entry is mandatory in a
  workspace and cannot be dropped.
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
lib/ecs/
├── Cargo.toml           (modified — dependency spark-ecs-macros = { path = "macros" })
├── src/lib.rs           (modified — Send+Sync traits; no blanket impl; no prelude)
└── macros/              (NEW nested crate — spark-ecs-macros, proc-macro = true)
    ├── Cargo.toml
    └── src/lib.rs
Cargo.toml               (modified — "lib/ecs/macros" added to workspace members)
```

### Acceptance criteria

- [x] `spark-ecs-macros` crate exists with `proc-macro = true`, nested at `lib/ecs/macros/`.
- [x] `#[derive(Component)]` / `#[derive(Resource)]` generate the marker impls.
- [x] `Component` is `Send + Sync + 'static`; `Resource` is `'static`; blanket impl gone.
- [x] No `prelude` (none existed); imports are explicit throughout demos and tests.
- [x] Workspace builds; clippy clean; all demos updated and run.
- [x] A component holding a non-`Send` field fails to derive (`compile_fail` doctest on the `Component` trait).

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

---

## Draft 3 — spark-ecs: multi-mut query joins `(&mut A, &mut B)` + query self-conflict detection (issue 1b)

> 🟡 **Tracked as [#26](https://github.com/AlexTiTanium/spark/issues/26).**
> The precondition this draft assumes (Draft 1's shared/exclusive split) is
> already in main, so the issue body has been rewritten to reference main
> instead of Draft 1.

### Context

Draft 1 ships query iteration for the cases that need no `unsafe`: `&mut T`
alone, and one-mut-many-read tuples (`(&mut A, &B)`). It deliberately leaves
out `(&mut A, &mut B)` — two mutable sides — because the second mutable side is
accessed at *random* dense indices, and safe Rust cannot hand out `&mut` by
arbitrary index repeatedly (the borrow checker cannot prove the indices
distinct).

A heavy simulation hits multi-mut joins early (a physics integrator writing
both `Velocity` and `Position`), so this is not deferrable to M4. It is a
**single-threaded iteration concern**, fully solvable on the current `RefCell`
storage. This issue adds `(&mut A, &mut B)` via one small `unsafe` function,
and pairs it with the self-conflict check that makes that `unsafe` rest on
*enforced* facts rather than hope.

Depends on Draft 1 (must merge after it). Produces the query-level access
primitive that the scheduler issue (roadmap item 3) extends.

### Goals & non-goals

**Goals**

- `(&mut A, &mut B)` iterates via `Query::iter_mut`, both sides mutable.
- Exactly **one** `unsafe fn` in `query.rs`, with a `# Safety` contract doc
  and `SAFETY:` comments at its single call site.
- Query-level access collection: `QueryData::collect_access`.
- Self-conflict check at query construction: reject any query where one
  component `TypeId` appears twice with a `&mut` — `(&mut A, &mut A)`,
  `(&mut A, &A)`. The check runs **before** the storages are borrowed.
- The access type (`QueryAccess`) and conflict logic are reusable by the
  scheduler, not query-private.

**Non-goals**

- Making `(&mut A, &A)` / `(&mut A, &mut A)` *work* — they are rejected; that
  is the point.
- ~~Wider mut tuples `(&mut A, &mut B, &mut C)` — ride on tuple arity 3–4
  (roadmap item 5)~~ **Folded in.** The unified `impl_all_tuple!` macro
  Cartesian-products the `&` / `&mut` flags, so every combination at
  arity 2–4 (including all-mut at any arity, and read-driver +
  mut-non-driver) ships with this issue. Adding arity 5+ is one line.
- Hoisting the conflict check to registration time — that is the scheduler's
  job (item 3). Here it runs per query construction (a few `TypeId`
  comparisons, negligible).
- `par_iter`, change detection, leading-storage `min` optimisation.

### Proposed shape

`lib/ecs/src/query.rs` — the one `unsafe` function, a mutable random-access
view into a storage's `dense`:

```rust
/// Mutable random-access view into one storage's `dense`, used by
/// multi-mut joins to hand out `&mut T` by entity.
struct DenseMut<'s, T> {
    ptr: *mut T,                 // dense.as_mut_ptr()
    len: usize,                  // dense.len()  (bounds check only)
    sparse: &'s [Option<u32>],   // entity.index -> dense index
    _marker: PhantomData<&'s mut [T]>,   // ties lifetime + variance to the borrow
}

impl<'s, T> DenseMut<'s, T> {
    /// # Safety
    /// Across the whole lifetime `'s`, `get` must never be called twice
    /// with the same `entity`. Two `&mut T` to one dense slot would alias.
    ///
    /// Upheld by callers via two facts, neither of them hope:
    ///  1. the query iterator visits each entity at most once (structural);
    ///  2. the same component type never appears twice in the query
    ///     (enforced by the self-conflict check below).
    unsafe fn get(&self, entity: Entity) -> Option<&'s mut T> {
        let dense_idx = (*self.sparse.get(entity.index as usize)?)? as usize;
        debug_assert!(dense_idx < self.len, "sparse/dense desync — see #10");
        // SAFETY: dense_idx < len (ComponentStorage's swap_remove invariant,
        // asserted in debug); and by the contract above this entity is
        // fetched at most once, so no other live &mut overlaps this slot.
        Some(&mut *self.ptr.add(dense_idx))
    }
}
```

The `(&mut A, &mut B)` tuple iterator — driver stays safe, only the lookup
is `unsafe`:

```rust
// state = (RefMut<Storage<A>>, RefMut<Storage<B>>) — two different RefCells
let (sa, sb) = &mut *state;

// driver A: SAFE — slice::iter_mut zipped with the entity list
let driver = sa.dense.iter_mut().zip(sa.entity_index.iter());

// lookup B: the unsafe random-access view
let b_view = DenseMut::new(&mut sb.dense, &sb.sparse);

driver.filter_map(move |(a, &entity)| {
    // SAFETY: the driver yields each entity exactly once, so b_view.get
    // is called at most once per entity; A != B is guaranteed by the
    // self-conflict check, so A's and B's storages are disjoint RefCells.
    let b = unsafe { b_view.get(entity)? };
    Some((entity, (a, b)))
})
```

`lib/ecs/src/access.rs` (new) — the access primitive + self-conflict check:

```rust
#[derive(Default)]
pub struct QueryAccess {
    pub reads:  Vec<TypeId>,
    pub writes: Vec<TypeId>,
}

pub trait QueryData {
    // … existing items from Draft 1 …
    fn collect_access(access: &mut QueryAccess);
}
// &T  -> access.reads.push(TypeId::of::<T>())
// &mut T -> access.writes.push(TypeId::of::<T>())
// (D1, D2) -> D1::collect_access(a); D2::collect_access(a)

impl QueryAccess {
    /// Panics if one component is written and also written/read again.
    fn assert_no_self_conflict(&self) { /* TypeId comparisons */ }
}
```

### Access diagram

```
Query<(&mut A, &mut B)>::iter_mut
   │
   │ 1. collect_access  → writes:[A,B]  reads:[]
   │ 2. assert_no_self_conflict()        ← BEFORE any storage is borrowed
   │       A != B, no TypeId twice  → OK
   │       ( (&mut A,&A) would be writes:[A] reads:[A] → panic, clear message )
   │
   ▼ 3. init_state → (RefMut<Storage<A>>, RefMut<Storage<B>>)   two RefCells
   ├── driver A : dense_a.iter_mut().zip(entity_index_a)    SAFE (slice::iter_mut)
   │                   │ (&mut A, Entity), dense order
   │                   ▼
   └── lookup B : DenseMut{ ptr: dense_b.as_mut_ptr(), sparse:&sparse_b }
                       │ unsafe get(entity) -> &mut B        ← the ONLY unsafe
                       ▼
   per entity (each visited once): yield (e, (&mut A, &mut B))

Soundness: each entity once → distinct dense_b slot per get() → B-refs never alias.
           A != B (self-conflict check) → A-storage, B-storage are disjoint cells.
```

### Learn

- **Why a raw pointer, not `iter_mut`.** `slice::iter_mut` walks a slice
  *linearly*. The B side is visited in *entity-of-A* order — random relative
  to B's `dense`. Random `&mut` access needs a `*mut T` + index; the price is
  leaving the borrow checker and taking a written contract instead.
- **The `unsafe fn` contract.** `DenseMut::get` takes `&self` yet returns
  `&'s mut T` — the compiler would never allow that. The `unsafe` is the
  caller's promise "I guarantee distinct entities." `# Safety` documents the
  promise; `SAFETY:` comments at the call site discharge it.
- **The self-conflict check is what makes it sound — not the `unsafe`.** The
  `unsafe` block assumes two things. *Distinct entities* is structural — the
  iterator's shape. *Same component never twice* is **enforced** by
  `assert_no_self_conflict`. So the `unsafe` rests on one structural fact and
  one checked fact. This is the whole reason `unsafe` here is acceptable.
- **`PhantomData<&'s mut [T]>`.** A bare `*mut T` carries no lifetime and is
  variance-wrong. `PhantomData<&'s mut [T]>` ties `DenseMut` to the storage
  borrow `'s` (so it cannot dangle) and makes it invariant in `T` (correct for
  `&mut`).
- **Why `(&mut A, &A)` is a runtime panic, not a compile error.** Rust has no
  "these two types are distinct" bound — you cannot write `where A != B`. So
  type-level rejection is not feasible; bevy, the reference, detects this at
  runtime too. spark panics at query construction with a message naming the
  component. `compile_fail` is the wrong tool here — `should_panic` is. (
  `compile_fail` *is* right for Draft 1's `ReadOnlyQueryData` gate: calling
  `.iter()` on a `&mut` query is a genuine compile error.)
- **Two layers of protection in M3.** With `RefCell` storage, `(&mut A, &A)`
  is *also* caught by the double-borrow panic when the second state is
  fetched. The explicit check is a nicer diagnostic now — and becomes the
  **sole** guard once M4 swaps storages to `UnsafeCell` and the `RefCell`
  backstop is gone. The `unsafe fn` contract itself does not change at M4.

### Warnings (implementation)

- **Order matters: check before borrow.** `assert_no_self_conflict` must run
  *before* `init_state` borrows the storages. Otherwise `(&mut A, &A)`
  surfaces as a cryptic `RefCell` `BorrowMutError` instead of "query has
  conflicting access to component `A`".
- **The contract depends on no mid-iteration structural mutation.** "Each
  entity once" holds only because `Query` exclusively borrows the storages for
  the whole walk, so nothing can `insert`/`remove` mid-iteration (structural
  changes go through `Commands`, deferred). Do not add any API that mutates a
  storage while a query over it is live.
- **The `unsafe` trusts `ComponentStorage`'s sparse/dense invariant.** `get`
  assumes `sparse` only ever stores in-bounds `dense` indices — that is #10's
  `swap_remove` discipline. If that invariant breaks, this `unsafe` becomes
  unsound. Keep the `debug_assert!` and treat the two as coupled.
- **One `unsafe` function, period.** Do not spread raw-pointer access across
  the tuple impls. The driver stays safe (`slice::iter_mut`); only the
  random-access lookup is `unsafe`.
- **`compile_fail` is not usable for the self-conflict case** — use
  `should_panic`, and assert the panic message text so a future refactor that
  accidentally lets the `RefCell` panic fire first is caught.

### File-tree diff

```
lib/ecs/src/
├── query.rs    (modified — DenseMut + its one unsafe fn; (&mut A,&mut B)
│                tuple impl; collect_access wired through QueryData)
└── access.rs   (NEW — QueryAccess, assert_no_self_conflict; reused by the scheduler)
```

### Acceptance criteria

- [ ] `Query<(&mut A, &mut B)>::iter_mut` iterates; a unit test asserts both
      components are updated for entities that have both.
- [ ] Exactly one `unsafe fn` in `query.rs`, with a `# Safety` doc; a
      `SAFETY:` comment at its call site.
- [ ] `should_panic` test: `(&mut A, &A)` and `(&mut A, &mut A)` panic at
      query construction, with a message naming the conflicting component.
- [ ] The panic originates from `assert_no_self_conflict`, not the `RefCell`
      borrow — assert the message text.
- [ ] `compile_fail` test: `.iter()` (read-only) rejected on a query
      containing `&mut` (re-asserts Draft 1's gate).
- [ ] `QueryAccess` lives in its own module, usable without constructing a `Query`.
- [ ] clippy clean, including `clippy::undocumented_unsafe_blocks`.
- [ ] PR uses the project template with diagram + Learn + Warnings filled in.

### Verification plan

1. `cargo fmt --all -- --check`
2. `cargo build --workspace`
3. `cargo clippy --workspace --all-targets --all-features -- -D warnings`
4. `cargo test --workspace` — unit test for the mut-mut pass, `should_panic`
   tests for both self-conflict shapes, `compile_fail` test for the `.iter()`
   gate, doc tests.
5. `cargo run -p spark` — existing behaviour unchanged.

### Forward evolution

| Where next | Adds |
| --- | --- |
| This issue | `impl_all_tuple!` macro — Cartesian product of `&` / `&mut` at arities 2–5 (every combination, including all-mut); one `unsafe fn`; `QueryAccess` + self-conflict check |
| Arity 6+ | one line per arity (`impl_all_tuple!(A, B, C, D, E, F);`) — no new mechanism needed; monomorphisation cost doubles each step |
| Scheduler (item 3) | reuses `QueryAccess`; aggregates to `SystemParam` level; hoists conflict detection to registration time; adds cross-system conflicts → DAG |
| M4 | `RefCell → UnsafeCell` — the self-conflict check becomes the *sole* same-component guard; the `unsafe fn` contract is unchanged |

### Out of scope

`par_iter`; registration-time conflict hoisting (item 3); arities beyond 5
(one-line extension when needed); leading-storage `min` optimisation;
change detection.
