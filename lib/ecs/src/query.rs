//! Queries — declarative iteration over entities by component shape.
//!
//! `Query<&T>` walks every entity with a `T`. `Query<&mut T>` does the
//! same with exclusive access. `Query<(&mut A, &B)>` joins two storages
//! and yields only entities that hold both components.
//!
//! # Iteration shape — *path B* (Bevy-style)
//!
//! [`Query::iter`] yields the data shape itself, not an `(Entity, …)`
//! pair. So `Query<&T>::iter()` yields `&T`, `Query<(&mut A, &B)>`
//! yields `(&mut A, &B)`. If a system needs the entity id, it asks
//! for it explicitly by naming [`Entity`] as a query-data element:
//! `Query<(Entity, &T)>` yields `(Entity, &T)`, and `Query<Entity>`
//! yields the id alone (every live entity). See the [`Entity`] impl and
//! [`impl_all_tuple_entity!`] for how the id rides the existing internal
//! `(Entity, Item)` thread.
//!
//! **This is a deliberate choice — do not "fix" it by re-introducing
//! the always-present `(Entity, Item)` pair.** Two candidates were
//! considered:
//!
//! - *Path A* — every iter call yields `(Entity, Item)`. Simpler
//!   internally; one iterator type. Cost is paid in API honesty: every
//!   call site that doesn't need the entity writes `_e` and the
//!   signature mis-represents what the system reads.
//! - *Path B* — what's implemented here. Iteration yields the data
//!   shape; entity is fetched via an explicit `Entity` element in the
//!   shape (`Query<(Entity, &T)>`). The trait machinery still threads
//!   entities internally for sparse-set joins (the
//!   [`QueryData::iter`] trait method yields `(Entity, Item)`); the
//!   public [`Query::iter`] just strips the entity off.
//!
//! Path B was chosen because it lines up cleanly with the `Access`
//! model the scheduler is being built around: `Entity` becomes a
//! regular [`QueryData`] element with an empty access set, so the
//! *access* machinery (`collect_access`, self-conflict) handles it with
//! no special-case for an always-pair. Driver selection is the one
//! exception — `Entity` has no storage to drive, so standalone
//! `Query<Entity>` drives off a live-entity snapshot and entity-prefixed
//! tuples off their first *component* (see *Joins*). The internal
//! `(Entity, Item)` thread the join path carries is an implementation
//! detail of that layer.
//!
//! # The fetch / iterate split
//!
//! [`QueryData::init_state`] turns a `&World` into a tuple of
//! [`Ref`] / [`RefMut`] guards over the underlying
//! [`ComponentStorage`]s — that's the *fetch* phase. The state lives in
//! the [`Query`] for as long as the system holds it, so the borrow
//! guards survive the whole iteration. [`QueryData::iter`] then walks
//! the state and emits `(Entity, Item)` pairs — that's the *iterate*
//! phase. This is the single chokepoint M4 will swap from `RefCell` to
//! `UnsafeCell` once the parallel scheduler can prove disjoint access.
//!
//! # Shared vs exclusive iteration
//!
//! Exclusive iteration takes `&mut state` so `&mut T` items are
//! reachable from `slice::iter_mut` underneath — that's why
//! [`QueryData::iter`] takes `&'s mut Self::State<'_>`. The state's
//! own borrow lifetime stays decoupled from the iteration's lifetime
//! `'s`; coupling them would lock the state across calls.
//!
//! Shared iteration is gated by [`ReadOnlyQueryData`], which `&mut T`
//! deliberately does not implement. The compiler routes `Query::iter`
//! through `ReadOnlyQueryData::iter_ref` for read-only data shapes,
//! while `Query::iter_mut` always uses [`QueryData::iter`].
//!
//! # Joins
//!
//! Tuple queries drive the **smallest** element (by entity count; the
//! earliest element breaks a tie) and look the rest up per entity — driver
//! selection is frozen once at [`Query::from_world`] (see *Driver selection*
//! below and issue #65). **Every `&` / `&mut` combination** at arities 2–5 is
//! supported: a single [`impl_all_tuple!`] invocation per arity
//! Cartesian-products the flag positions and emits one `QueryData`
//! impl for each combination (plus a `ReadOnlyQueryData` impl for the
//! all-read case). At arity 2 that's 4 impls; arity 3 is 8; arity 4
//! is 16; arity 5 is 32. All bounds are on concrete `Component`
//! parameters, never on a generic `D1: QueryData`, so nested tuple
//! shapes (`((A, B), C)`, `(A, (B, C))`) don't match any impl.
//!
//! Adding arity 6+ is one line — `impl_all_tuple!(A, B, C, D, E, F);`
//! — and gives 64 impls for every flag combination at that arity.
//! Note that monomorphisation cost doubles with each step: weigh it
//! against actual need before extending past 5.
//!
//! **Optional elements** — `Option<&T>` / `Option<&mut T>` — are a parallel
//! family from [`impl_all_tuple_opt!`]: a tuple whose first element is
//! required (`&T`/`&mut T`, so a driver always exists) and whose trailing
//! elements may be optional, at arities 2–3. An optional fetches `T` when
//! present and yields `None` otherwise without dropping the row, so it never
//! drives. See the `Option<&T>` impl below.
//!
//! Entity-prefixed shapes — `Query<(Entity, &A)>` through
//! `Query<(Entity, &A, &B, &C)>` — come from a parallel
//! [`impl_all_tuple_entity!`] family (1–3 trailing components, the same
//! 2^N Cartesian expansion). `Entity` can't drive, so the first
//! *component* drives and `Entity` rides the id that driver already
//! threads; it adds nothing to `State` or `collect_access`.
//!
//! Read-only lookups use the storage's safe
//! [`ComponentStorage::get`]. Mutable non-driver lookups use a
//! [`DenseMut`] random-access view — the crate's single `unsafe fn`.
//! Its contract — "each entity is touched at most once per
//! iteration" — rests on two facts the engine *enforces*:
//!
//!  1. The driver iterator visits each entity at most once (structural
//!     property of `slice::iter_mut` zipped with `entity_index`).
//!  2. The same component type never appears twice in the data shape —
//!     enforced at [`Query::from_world`] by
//!     [`QueryAccess::assert_no_self_conflict`] *before* any storage is
//!     borrowed. `(&mut A, &A)` (in either element order) and
//!     `(&mut A, &mut A)` panic with a precise diagnostic instead of
//!     the `RefCell`'s "already borrowed".
//!
//! # Filters
//!
//! [`Query`] takes a second generic `F: QueryFilter`, defaulting to `()`
//! (always true), so `Query<D>` is shorthand for `Query<D, ()>`. A
//! filter narrows *which* entities iterate without touching the yielded
//! item: `Query<&Position, With<Powered>>` still yields `&Position`,
//! just for fewer entities. [`Query::from_world`] calls
//! [`QueryFilter::init_state`] once (fetching the filter's storage borrows +
//! tick baselines, kept for the query's lifetime) and folds
//! [`QueryFilter::collect_access`] into the same self-conflict check the
//! data shape runs; each `iter` then wraps the driver in a `.filter(…)` that
//! calls [`QueryFilter::matches`] per candidate against that state. A filter
//! can also *lead* the loop when it holds the smallest candidate — see
//! *Driver selection*. See the [`filter`] module for the filter set and the
//! access-reporting rules.
//!
//! [`filter`]: crate::filter
//! [`QueryFilter::matches`]: crate::QueryFilter::matches
//! [`QueryFilter::collect_access`]: crate::QueryFilter::collect_access

// The root keeps only the driver runtime; `Entity` is the one production
// dependency it still names (in `DriverIter` / `DriveSource`). The query
// API's heavier imports moved into `runner`; the test fixtures' imports
// moved into `tests`.
use crate::entity::Entity;

// -------- Driver selection (issue #65) --------

/// The entity stream a [`Query`] loops over — the *driver* — once driver
/// selection has picked the smallest candidate set ([`Query::from_world`]).
///
/// Always a borrowed slice into a dense entity list, so driving costs nothing
/// beyond walking it: usually a storage's own list (a data element's or a
/// filter's), and for an [`Or`](crate::Or) filter a slice into the union
/// materialized **once** at `from_world` (the [`Query`] owns the `Vec`). The
/// live-entity-set fallback doesn't go through here — it drives via the data
/// shape's own `iter`.
///
/// You only name this when hand-implementing [`QueryData::drive`]; the shipped
/// impls construct it for you.
pub struct DriverIter<'s>(std::slice::Iter<'s, Entity>);

impl<'s> DriverIter<'s> {
    /// Wraps a dense entity slice as a driver. `Entity` is `Copy`, so yielding
    /// is a plain copy out of the slice.
    fn new(entities: &'s [Entity]) -> Self {
        Self(entities.iter())
    }
}

impl Iterator for DriverIter<'_> {
    type Item = Entity;

    fn next(&mut self) -> Option<Entity> {
        self.0.next().copied()
    }
}

/// How a [`QueryData`] shape should be driven, handed to
/// [`QueryData::drive`] / [`ReadOnlyQueryData::drive_ref`] once
/// [`Query::from_world`] has chosen the smallest candidate across the whole
/// query.
///
/// You only name this when hand-implementing [`QueryData`]; the shipped impls
/// receive it from the [`Query`] dispatch.
pub enum DriveSource<'s> {
    /// Drive off the data element at this index (the data shape itself owns
    /// the smallest candidate). The impl walks that element's entities and
    /// looks the rest up per entity.
    Data(usize),
    /// Drive off an external entity stream — a filter's candidate. Every data
    /// element is looked up per entity. (The live-set fallback does *not* use
    /// this — it drives via the data shape's own `iter`; see [`DriverPlan`].)
    External(DriverIter<'s>),
}

/// The driver decision frozen at [`Query::from_world`], replayed on every
/// `iter` / `iter_mut`. Computed once from the data shape's per-element
/// populations and the filter's candidate population; it never depends on a
/// per-call lookup.
#[derive(Clone, Copy)]
enum DriverPlan {
    /// No part offers a candidate (`Query<Entity>`, `Query<Entity, Without<T>>`,
    /// `Query<Entity, ()>`, standalone `Query<Option<&T>>` /
    /// `Query<Option<&mut T>>`): drive the live-entity snapshot via the data
    /// shape's own `iter`. The structurally exempt case — a shape naming no
    /// *required* component has no smaller candidate to drive off, so this is
    /// the permanent fallback, not a temporary one.
    LiveSet,
    /// A data element holds the smallest candidate; drive it (its index).
    Data(usize),
    /// The filter holds the smallest candidate; it leads the loop.
    Filter,
}

// Counts one driver advance per yielded candidate, for the deterministic
// cost-contract tests (issue #65). `counted!` wraps each driver iterator; in
// non-test builds it expands to the iterator unchanged, so there is provably
// zero per-iteration overhead in release.
#[cfg(test)]
thread_local! {
    static DRIVER_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records one driver advance. Test-only; see [`take_driver_steps`].
#[cfg(test)]
pub(crate) fn record_driver_step() {
    DRIVER_STEPS.with(|c| c.set(c.get() + 1));
}

/// Returns the driver-advance count since the last call and resets it to 0.
/// The cost-contract tests assert this tracks the smallest candidate
/// population, never the live set.
///
/// Read only by the `driver_cost` tests, which are `#[cfg(not(miri))]`
/// (a 10k-entity step-count oracle has no aliasing surface — see
/// `query/tests.rs`). `record_driver_step` still fires under Miri via
/// `counted!`, so the counter stays live; only this reader goes unused
/// there, hence the targeted `allow`.
#[cfg(test)]
#[cfg_attr(miri, allow(dead_code))]
pub(crate) fn take_driver_steps() -> usize {
    DRIVER_STEPS.with(|c| c.replace(0))
}

/// Wraps a driver iterator so each advance is counted in test builds; expands
/// to the iterator untouched otherwise (zero release overhead).
#[cfg(test)]
macro_rules! counted {
    ($it:expr) => {
        $it.inspect(|_| $crate::query::record_driver_step())
    };
}
#[cfg(not(test))]
macro_rules! counted {
    ($it:expr) => {
        $it
    };
}

// ---- Submodules -----------------------------------------------------
//
// The query layer spans several files; this root keeps the module docs,
// the driver runtime ([`DriverIter`] / [`DriveSource`] / [`DriverPlan`]),
// and the test-step harness the generated code's
// `$crate::query::record_driver_step()` path resolves to. `counted!` and
// those private items are visible to every child below by textual scope
// (declared here, before the `mod`s) and ancestor-private access.
//
// Declared after `counted!` so the macro is in scope inside each child.
mod data;
mod dense_mut;
mod runner;
mod tuple_codegen;

#[cfg(test)]
mod tests;

// Re-exported so `crate::query::{QueryData, ReadOnlyQueryData}` (and the
// `lib.rs` re-export) keep resolving, and so the tuple codegen can name the
// traits it implements.
pub use data::{QueryData, ReadOnlyQueryData};
pub use runner::Query;
