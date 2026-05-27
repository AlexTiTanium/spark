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

use std::cell::{Ref, RefMut};

use crate::Component;
use crate::access::{Access, QueryAccess};
use crate::entity::Entity;
use crate::filter::QueryFilter;
use crate::storage::{ComponentStorage, Mut};
use crate::system::SystemParam;
use crate::world::World;

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
#[cfg(test)]
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

use dense_mut::DenseMut;
// Re-exported so `crate::query::{QueryData, ReadOnlyQueryData}` (and the
// `lib.rs` re-export) keep resolving, and so the tuple codegen below can
// name the traits it implements.
pub use data::{QueryData, ReadOnlyQueryData};

// -------- Tuple impls (arity 2 – 5) --------
//
// One public macro covers every supported tuple shape:
// [`impl_all_tuple!($T1, $T2, …)`] emits **every 2^N combination** of
// `&` / `&mut` flags across the type parameters at that arity. The
// shapes you get from `impl_all_tuple!(A, B)`:
//
// ```text
//   (&A, &B)         (&mut A, &B)
//   (&A, &mut B)     (&mut A, &mut B)
// ```
//
// Arity 3 / 4 / 5 expand to 8 / 16 / 32 impls respectively. Each impl
// is monomorphised once per concrete instantiation, so users only pay
// for shapes they actually construct.
//
// **No nested tuples.** All bounds are on concrete `Component`
// parameters (`A: Component`, `B: Component`, …), never on a generic
// `D1: QueryData`. That structurally rules out `((A, B), C)`,
// `(A, (B, C))`, and similar — they don't match any impl pattern.
// Arity grows through additional `impl_all_tuple!` invocations, not
// through tuple recursion.
//
// **Extending arity past 5.** Add one line below:
// `impl_all_tuple!(A, B, C, D, E, F);` unlocks arity 6 (64 combos).
// Monomorphisation cost doubles with each step — weigh it against
// real need before extending. Optional-bearing shapes live in the
// companion `impl_all_tuple_opt!` family further down (currently arities
// 2–3); extending *those* past 3 means adding a line there too.

// ---- Helper macros (file-private) -----------------------------------
//
// Each helper takes a flag token (`R` for read = `&T`, `W` for write =
// `&mut T`) followed by a type ident, and emits the right Rust syntax
// for that position. They keep `impl_one_combo!` body free of
// per-flag branching.

/// `&'w T` for `R`, `&'w mut T` for `W` — the type a user *writes* in the
/// query shape, used as the `impl QueryData for (…)` target.
macro_rules! decl_type {
    (R $T:ident, $w:lifetime) => { &$w $T };
    (W $T:ident, $w:lifetime) => { &$w mut $T };
    (O $T:ident, $w:lifetime) => { Option<&$w $T> };
    (OW $T:ident, $w:lifetime) => { Option<&$w mut $T> };
}

/// `&'w T` for `R`, `Mut<'w, T>` for `W` — the type iteration *yields*
/// (the `Item<'w>` associated type), so a tuple's mutable elements stamp
/// only on actual write. Distinct from [`decl_type!`]: `Query<(&A, &mut B)>`
/// is spelled with `&mut B` but yields `Mut<'_, B>`.
macro_rules! item_type {
    (R $T:ident, $w:lifetime) => { &$w $T };
    (W $T:ident, $w:lifetime) => { Mut<$w, $T> };
    (O $T:ident, $w:lifetime) => { Option<&$w $T> };
    (OW $T:ident, $w:lifetime) => { Option<Mut<$w, $T>> };
}

/// `Option<Ref<'w, ComponentStorage<T>>>` for `R`, `Option<RefMut<…>>`
/// for `W`. Matches the per-element borrow guard the `Query` holds for
/// the iteration's lifetime.
macro_rules! state_type {
    (R $T:ident, $w:lifetime) => { Option<Ref<$w, ComponentStorage<$T>>> };
    (W $T:ident, $w:lifetime) => { Option<RefMut<$w, ComponentStorage<$T>>> };
    (O $T:ident, $w:lifetime) => { Option<Ref<$w, ComponentStorage<$T>>> };
    (OW $T:ident, $w:lifetime) => { Option<RefMut<$w, ComponentStorage<$T>>> };
}

/// `world.storage::<T>()` for `R`, `world.storage_mut::<T>()` for `W`.
macro_rules! init_storage {
    (R $T:ident, $world:expr) => {
        $world.storage::<$T>()
    };
    (W $T:ident, $world:expr) => {
        $world.storage_mut::<$T>()
    };
    (O $T:ident, $world:expr) => {
        $world.storage::<$T>()
    };
    (OW $T:ident, $world:expr) => {
        $world.storage_mut::<$T>()
    };
}

/// `access.add_read::<T>()` for `R`/`O`, `access.add_write::<T>()` for
/// `W`/`OW` — an optional borrows its component exactly like the required
/// form, so `(&A, Option<&mut A>)` still trips the self-conflict check.
macro_rules! access_call {
    (R $T:ident, $access:expr) => {
        $access.add_read::<$T>();
    };
    (W $T:ident, $access:expr) => {
        $access.add_write::<$T>();
    };
    (O $T:ident, $access:expr) => {
        $access.add_read::<$T>();
    };
    (OW $T:ident, $access:expr) => {
        $access.add_write::<$T>();
    };
}

/// Builds the per-iteration lookup handle for one non-driver position.
///
/// - `R`: reborrow the state as `&'s Option<Ref<…>>`. The closure
///   reads through `.as_ref()?.get(entity)`.
/// - `W`: build an `Option<DenseMut<'s, T>>` from
///   [`ComponentStorage::split_for_join`]. The closure reads through
///   the `unsafe fn` [`DenseMut::get`].
///
/// `O` is identical to `R` and `OW` to `W` — the optional/required
/// difference lives in [`fetch_non_driver!`], not in how the handle is
/// built.
macro_rules! build_non_driver_fetch {
    (R, $state:ident) => {
        &*$state
    };
    (W, $state:ident) => {
        $state.as_mut().map(|refmut| {
            let (dense, changed_tick, sparse, entity_index, current_tick) = refmut.split_for_join();
            DenseMut::new(dense, changed_tick, sparse, entity_index, current_tick)
        })
    };
    (O, $state:ident) => {
        &*$state
    };
    (OW, $state:ident) => {
        $state.as_mut().map(|refmut| {
            let (dense, changed_tick, sparse, entity_index, current_tick) = refmut.split_for_join();
            DenseMut::new(dense, changed_tick, sparse, entity_index, current_tick)
        })
    };
}

/// Per-entity lookup for one non-driver position.
///
/// - `R`: safe `storage.get(entity)`.
/// - `W`: `DenseMut::get(entity)`, the crate's single `unsafe fn`. The
///   safety contract is upheld by the impl's overall design: the
///   driver visits each entity at most once, and the self-conflict
///   check in `Query::from_world` rules out the same component
///   appearing twice. See the *Joins* section in the module-level
///   docs for the full argument.
///
/// The `O`/`OW` arms are the heart of optional fetch: they drop the
/// trailing `?`, so an absent component yields a `None` *value* in the
/// tuple instead of short-circuiting the whole row away. (`R`/`W` keep the
/// `?` — a missing required component skips the entity.)
macro_rules! fetch_non_driver {
    (R, $fetch:expr, $entity:expr) => {
        $fetch.as_ref()?.get($entity)?
    };
    (W, $fetch:expr, $entity:expr) => {
        // SAFETY: driver visits each entity once, conflict check
        // guarantees disjoint storages. Module-level docs for details.
        unsafe { $fetch.as_ref()?.get($entity)? }
    };
    (O, $fetch:expr, $entity:expr) => {
        $fetch.as_ref().and_then(|s| s.get($entity))
    };
    (OW, $fetch:expr, $entity:expr) => {
        // SAFETY: same contract as the `W` arm — the driver visits each
        // entity once and the self-conflict check rules out a duplicate.
        unsafe { $fetch.as_ref().and_then(|v| v.get($entity)) }
    };
}

/// Driver iterator for the **first-element** path — `iter` / `iter_ref`, and
/// the `DriveSource::Data(0)` fast path that delegates to them. `R` uses safe
/// `.iter()`, `W` uses safe `.iter_mut()` (both from `ComponentStorage`).
/// Driving a *non-first* element goes through `build_elem!` + [`DriverIter`]
/// instead.
///
/// No `O`/`OW` arms: an optional element never sits in the first (driver)
/// position — `impl_all_tuple_opt_cartesian!` only ever assigns `R`/`W` to
/// the first slot — so this macro is never invoked with an optional flag.
macro_rules! drive_iter {
    (R, $state:ident) => {
        match $state {
            Some(s) => Box::new(s.iter()),
            None => Box::new(std::iter::empty()),
        }
    };
    (W, $state:ident) => {
        match $state {
            Some(s) => Box::new(s.iter_mut()),
            None => Box::new(std::iter::empty()),
        }
    };
}

/// Population of one element's storage (`0` if absent), flag-agnostic — used
/// by `min_data_candidate` to size each element's candidate set in O(1).
macro_rules! len_of {
    ($state:expr) => {
        $state.as_ref().map_or(0, |s| s.len())
    };
}

/// One read element's dense entity slice (empty if the storage is absent) —
/// the driver slice for a `&T` element in the shared (`drive_ref`) path.
macro_rules! slice_of_read {
    ($state:expr) => {
        $state.as_ref().map_or(&[][..], |s| s.entities())
    };
}

/// Builds `(entity slice, fetch handle)` for one element in the exclusive
/// (`drive`) path. The slice is a standalone `&[Entity]` (so it can drive the
/// loop) and the handle looks the element up per entity:
///
/// - `R`: `(entities, &Option<Ref<…>>)` — looked up via `storage.get`.
/// - `W`: `(entities, Option<DenseMut<…>>)` — looked up via the `unsafe`
///   [`DenseMut::get`]. The slice comes from the *same* `split_for_join` as
///   the `DenseMut`, so the shared `entity_index` borrow (driver) and the
///   `&mut dense` borrow (lookup) are disjoint — no aliasing.
///
/// For `O`/`OW` the entity slice is always empty (`&[][..]`): an optional
/// never wins driver selection (its `cand_len!` is `None`), so its slice is
/// never indexed as the driver. Only the fetch handle is used.
macro_rules! build_elem {
    (R, $state:ident) => {{
        let fetch = &*$state;
        let ents: &[Entity] = slice_of_read!(fetch);
        (ents, fetch)
    }};
    (W, $state:ident) => {
        match $state.as_mut() {
            Some(refmut) => {
                let (dense, changed_tick, sparse, entity_index, current_tick) =
                    refmut.split_for_join();
                (
                    entity_index,
                    Some(DenseMut::new(
                        dense,
                        changed_tick,
                        sparse,
                        entity_index,
                        current_tick,
                    )),
                )
            }
            None => (&[][..], None),
        }
    };
    (O, $state:ident) => {{
        let fetch = &*$state;
        (&[][..], fetch)
    }};
    (OW, $state:ident) => {
        match $state.as_mut() {
            Some(refmut) => {
                let (dense, changed_tick, sparse, entity_index, current_tick) =
                    refmut.split_for_join();
                (
                    &[][..],
                    Some(DenseMut::new(
                        dense,
                        changed_tick,
                        sparse,
                        entity_index,
                        current_tick,
                    )),
                )
            }
            None => (&[][..], None),
        }
    };
}

/// Candidate population for driver selection: `Some(len)` for the required
/// flags `R`/`W`, `None` for the optional flags `O`/`OW`. An optional
/// contributes no candidate — it narrows nothing, so it must never win the
/// driver. Used by `impl_one_combo_opt!`'s `min_data_candidate`.
macro_rules! cand_len {
    (R, $state:expr) => {
        Some($state.as_ref().map_or(0, |s| s.len()))
    };
    // Same as `R` — both required flags expose a candidate.
    (W, $state:expr) => {
        Some($state.as_ref().map_or(0, |s| s.len()))
    };
    (O, $state:expr) => {
        None::<usize>
    };
    // Same as `O` — both optional flags narrow nothing, so no candidate.
    (OW, $state:expr) => {
        None::<usize>
    };
}

/// Per-entity lookup on the shared (`ReadOnlyQueryData`) path, reading the
/// raw `&Option<Ref<…>>` state element directly. `R` keeps the `?` (a
/// missing required component skips the row); `O` drops it (a missing
/// optional yields a `None` value). The read-only side never sees `W`/`OW`.
macro_rules! fetch_ro {
    (R, $state:expr, $entity:expr) => {
        $state.as_ref()?.get($entity)?
    };
    (O, $state:expr, $entity:expr) => {
        $state.as_ref().and_then(|s| s.get($entity))
    };
}

/// Read-only (`drive_ref`) mirror of `build_elem!`'s slice: `R` yields the
/// storage's own entity slice, `O` an empty one (an optional never wins
/// `min_data_candidate`, so its slot is never the chosen driver index).
macro_rules! slice_ro {
    (R, $state:expr) => {
        slice_of_read!($state)
    };
    (O, $state:expr) => {
        &[][..]
    };
}

// ---- impl_one_combo: one impl per flag sequence ----------------------
//
// Takes a sequence `$flag $T,` of `R`/`W` flags paired with type idents.
// First arm matches all-`R` sequences and emits both `QueryData` and
// `ReadOnlyQueryData`. Second arm matches any sequence with at least
// one `W` (because the first arm requires the literal `R` at every
// position) and emits only `QueryData`. The `@gen` and `@readonly`
// internal arms hold the shared bodies.

macro_rules! impl_one_combo {
    // All-R: also emit ReadOnly.
    (R $First:ident, $(R $Rest:ident,)+) => {
        impl_one_combo!(@gen R $First, $(R $Rest,)+);
        impl_one_combo!(@readonly $First $(, $Rest)+);
    };
    // Mixed (at least one W): only QueryData.
    ($first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)+) => {
        impl_one_combo!(@gen $first_flag $First, $($rest_flag $Rest,)+);
    };

    (@gen $first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)+) => {
        #[allow(unsafe_code)]
        impl<$First: Component, $($Rest: Component),+>
            QueryData for (decl_type!($first_flag $First, '_), $(decl_type!($rest_flag $Rest, '_),)+)
        {
            type Item<'w>
                = (item_type!($first_flag $First, 'w), $(item_type!($rest_flag $Rest, 'w),)+)
            where
                Self: 'w;
            type State<'w>
                = (state_type!($first_flag $First, 'w), $(state_type!($rest_flag $Rest, 'w),)+)
            where
                Self: 'w;

            fn init_state<'w>(world: &'w World) -> Self::State<'w>
            where
                Self: 'w,
            {
                (init_storage!($first_flag $First, world), $(init_storage!($rest_flag $Rest, world),)+)
            }

            #[allow(non_snake_case)]
            fn iter<'s, 'w>(
                state: &'s mut Self::State<'w>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)+) = state;
                $(let $Rest = build_non_driver_fetch!($rest_flag, $Rest);)+
                let driver: Box<dyn Iterator<Item = (Entity, item_type!($first_flag $First, 's))> + 's> =
                    drive_iter!($first_flag, $First);
                Box::new(counted!(driver).filter_map(move |(entity, item_first)| {
                    Some((
                        entity,
                        (
                            item_first,
                            $(fetch_non_driver!($rest_flag, $Rest, entity),)+
                        ),
                    ))
                }))
            }

            fn collect_access(access: &mut QueryAccess) {
                access_call!($first_flag $First, access);
                $(access_call!($rest_flag $Rest, access);)+
            }

            #[allow(non_snake_case)]
            fn min_data_candidate<'w>(state: &Self::State<'w>) -> Option<(usize, usize)>
            where
                Self: 'w,
            {
                let ($First, $($Rest,)+) = state;
                // Positional array → array index *is* the element index; the
                // first minimum wins ties (strict `<`), so the earliest
                // element leads on a tie.
                let pops = [len_of!($First), $(len_of!($Rest),)+];
                let mut best = (0usize, pops[0]);
                for (i, &p) in pops.iter().enumerate().skip(1) {
                    if p < best.1 {
                        best = (i, p);
                    }
                }
                Some(best)
            }

            #[allow(non_snake_case)]
            fn drive<'s, 'w>(
                state: &'s mut Self::State<'w>,
                driver: DriveSource<'s>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                // First element is the natural driver: keep the zero-overhead
                // direct path (yields its value, no redundant lookup).
                if let DriveSource::Data(0) = driver {
                    return Self::iter(state);
                }
                let ($First, $($Rest,)+) = state;
                // `build_elem!` yields `(entity slice, fetch handle)` per
                // element; shadow each ident with that pair.
                let $First = build_elem!($first_flag, $First);
                $(let $Rest = build_elem!($rest_flag, $Rest);)+
                // Positional slice array, indexed by the chosen data index.
                let slices = [$First.0, $($Rest.0,)+];
                let di: DriverIter<'s> = match driver {
                    DriveSource::Data(k) => DriverIter::new(slices[k]),
                    DriveSource::External(di) => di,
                };
                Box::new(counted!(di).filter_map(move |entity| {
                    Some((
                        entity,
                        (
                            fetch_non_driver!($first_flag, $First.1, entity),
                            $(fetch_non_driver!($rest_flag, $Rest.1, entity),)+
                        ),
                    ))
                }))
            }
        }
    };

    (@readonly $First:ident $(, $Rest:ident)+) => {
        impl<$First: Component, $($Rest: Component),+>
            ReadOnlyQueryData for (&$First, $(&$Rest,)+)
        {
            #[allow(non_snake_case)]
            fn iter_ref<'s, 'w>(
                state: &'s Self::State<'w>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)+) = state;
                let driver: Box<dyn Iterator<Item = (Entity, &'s $First)> + 's> = match $First {
                    Some(s) => Box::new(s.iter()),
                    None => Box::new(std::iter::empty()),
                };
                Box::new(counted!(driver).filter_map(move |(entity, item_first)| {
                    Some((
                        entity,
                        (
                            item_first,
                            $($Rest.as_ref()?.get(entity)?,)+
                        ),
                    ))
                }))
            }

            #[allow(non_snake_case)]
            fn lookup<'s, 'w>(
                state: &'s Self::State<'w>,
                entity: Entity,
            ) -> Option<Self::Item<'s>>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)+) = state;
                Some((
                    $First.as_ref()?.get(entity)?,
                    $($Rest.as_ref()?.get(entity)?,)+
                ))
            }

            #[allow(non_snake_case)]
            fn drive_ref<'s, 'w>(
                state: &'s Self::State<'w>,
                driver: DriveSource<'s>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                if let DriveSource::Data(0) = driver {
                    return Self::iter_ref(state);
                }
                // Read shapes look every element up via `lookup`, so the
                // driver just supplies entities — a borrowed slice (data /
                // filter) or an owned list (`Or` union / live set).
                let di: DriverIter<'s> = match driver {
                    DriveSource::Data(k) => {
                        let ($First, $($Rest,)+) = state;
                        let slices = [slice_of_read!($First), $(slice_of_read!($Rest),)+];
                        DriverIter::new(slices[k])
                    }
                    DriveSource::External(di) => di,
                };
                Box::new(counted!(di).filter_map(move |entity| {
                    Self::lookup(state, entity).map(|item| (entity, item))
                }))
            }
        }
    };
}

// ---- impl_all_tuple_cartesian: tt-muncher Cartesian product ----------
//
// Recursively assigns each remaining type ident both an `R` and a `W`
// flag, accumulating the flag-type pairs. When the type list is empty,
// the accumulator names one specific flag sequence and `impl_one_combo!`
// emits its impl(s). For N input types this generates 2^N
// `impl_one_combo!` calls.

macro_rules! impl_all_tuple_cartesian {
    (@start [$($acc:tt)*]) => {
        impl_one_combo!($($acc)*);
    };
    (@start [$($acc:tt)*] $Head:ident, $($Tail:ident,)*) => {
        impl_all_tuple_cartesian!(@start [$($acc)* R $Head,] $($Tail,)*);
        impl_all_tuple_cartesian!(@start [$($acc)* W $Head,] $($Tail,)*);
    };
}

/// Emits every `QueryData` impl (and the `ReadOnlyQueryData` impl for
/// the all-read combination) for a tuple of the given type parameters
/// at *every* `&` / `&mut` combination.
///
/// One line per arity. `impl_all_tuple!(A, B, C)` generates 2^3 = 8
/// impls covering `(&A, &B, &C)`, `(&mut A, &B, &C)`,
/// `(&A, &mut B, &C)`, …, `(&mut A, &mut B, &mut C)`. The macro
/// requires arity ≥ 2 (single-component shapes are handled by the
/// `&T` / `&mut T` impls earlier in the file).
macro_rules! impl_all_tuple {
    ($A:ident, $($B:ident),+) => {
        impl_all_tuple_cartesian!(@start [] $A, $($B,)+);
    };
}

impl_all_tuple!(A, B);
impl_all_tuple!(A, B, C);
impl_all_tuple!(A, B, C, D);
impl_all_tuple!(A, B, C, D, E);

// ---- Optional-bearing tuples: Query<(&A, Option<&B>)> ----------------
//
// A second family beside impl_one_combo! / impl_all_tuple!, covering
// tuples whose trailing elements may be `Option<&T>` (`O`) or
// `Option<&mut T>` (`OW`). It reuses every per-flag helper (decl_type!,
// item_type!, state_type!, init_storage!, access_call!,
// build_non_driver_fetch!, fetch_non_driver!, drive_iter!, build_elem!)
// — those gained `O`/`OW` arms — and adds two: `cand_len!` (optionals
// offer no driver candidate) and `fetch_ro!` (read-only optional lookup).
//
// THE FIRST ELEMENT IS ALWAYS REQUIRED (`R`/`W`). An optional narrows
// nothing, so it can never *drive*; keeping a required element first
// guarantees a driver always exists, which is why the all-optional and
// optional-first corners simply don't arise here. (Standalone
// `Query<Option<&T>>` is handled by the hand-written impl above, which
// drives the live-entity snapshot.)
//
// `impl_all_tuple_opt!` emits ONLY shapes with at least one optional —
// the pure-`R`/`W` combinations are already covered by `impl_all_tuple!`,
// so re-emitting them would collide. Arities 2–3 only: arity 2 adds 4
// impls, arity 3 adds 24.

/// Emits `impl_one_combo_opt!(@readonly …)` iff every flag in the scanned
/// list is read-only (`R` or `O`); a `W`/`OW` aborts emission. `macro_rules`
/// can't test "is any flag a write?" inside a repetition, so this walks the
/// flag/type list as a tt-muncher, carrying the full combo to replay at the
/// base case.
macro_rules! emit_readonly {
    (@scan [] -> [$($combo:tt)+]) => {
        impl_one_combo_opt!(@readonly $($combo)+);
    };
    (@scan [W $T:ident, $($rest:tt)*] -> [$($combo:tt)+]) => {};
    (@scan [OW $T:ident, $($rest:tt)*] -> [$($combo:tt)+]) => {};
    (@scan [R $T:ident, $($rest:tt)*] -> [$($combo:tt)+]) => {
        emit_readonly!(@scan [$($rest)*] -> [$($combo)+]);
    };
    (@scan [O $T:ident, $($rest:tt)*] -> [$($combo:tt)+]) => {
        emit_readonly!(@scan [$($rest)*] -> [$($combo)+]);
    };
}

macro_rules! impl_one_combo_opt {
    (@gen $first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)+) => {
        #[allow(unsafe_code)]
        impl<$First: Component, $($Rest: Component),+>
            QueryData for (decl_type!($first_flag $First, '_), $(decl_type!($rest_flag $Rest, '_),)+)
        {
            type Item<'w>
                = (item_type!($first_flag $First, 'w), $(item_type!($rest_flag $Rest, 'w),)+)
            where
                Self: 'w;
            type State<'w>
                = (state_type!($first_flag $First, 'w), $(state_type!($rest_flag $Rest, 'w),)+)
            where
                Self: 'w;

            fn init_state<'w>(world: &'w World) -> Self::State<'w>
            where
                Self: 'w,
            {
                (init_storage!($first_flag $First, world), $(init_storage!($rest_flag $Rest, world),)+)
            }

            #[allow(non_snake_case)]
            fn iter<'s, 'w>(
                state: &'s mut Self::State<'w>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)+) = state;
                $(let $Rest = build_non_driver_fetch!($rest_flag, $Rest);)+
                let driver: Box<dyn Iterator<Item = (Entity, item_type!($first_flag $First, 's))> + 's> =
                    drive_iter!($first_flag, $First);
                // `filter_map` (not `map`): a required (`R`/`W`) non-driver
                // still short-circuits the row via its `?`; an optional
                // (`O`/`OW`) yields a `None` value and keeps the row.
                Box::new(counted!(driver).filter_map(move |(entity, item_first)| {
                    Some((
                        entity,
                        (
                            item_first,
                            $(fetch_non_driver!($rest_flag, $Rest, entity),)+
                        ),
                    ))
                }))
            }

            fn collect_access(access: &mut QueryAccess) {
                access_call!($first_flag $First, access);
                $(access_call!($rest_flag $Rest, access);)+
            }

            // `unused_variables`: `cand_len!` ignores its binding for
            // optional positions, so an `O`/`OW` element's name is unused.
            #[allow(non_snake_case, unused_variables)]
            fn min_data_candidate<'w>(state: &Self::State<'w>) -> Option<(usize, usize)>
            where
                Self: 'w,
            {
                let ($First, $($Rest,)+) = state;
                // Optionals report `None` (they narrow nothing); required
                // elements compete on population, earliest wins ties (strict
                // `<` in `reduce`). The first element is always required, so
                // this never returns `None` for a tuple.
                [cand_len!($first_flag, $First), $(cand_len!($rest_flag, $Rest),)+]
                    .into_iter()
                    .enumerate()
                    .filter_map(|(i, c)| c.map(|p| (i, p)))
                    .reduce(|best, cur| if cur.1 < best.1 { cur } else { best })
            }

            #[allow(non_snake_case)]
            fn drive<'s, 'w>(
                state: &'s mut Self::State<'w>,
                driver: DriveSource<'s>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                if let DriveSource::Data(0) = driver {
                    return Self::iter(state);
                }
                let ($First, $($Rest,)+) = state;
                let $First = build_elem!($first_flag, $First);
                $(let $Rest = build_elem!($rest_flag, $Rest);)+
                // Optionals contribute an empty slice here; they are never the
                // chosen `Data(k)` (their `cand_len!` is `None`).
                let slices = [$First.0, $($Rest.0,)+];
                let di: DriverIter<'s> = match driver {
                    DriveSource::Data(k) => DriverIter::new(slices[k]),
                    DriveSource::External(di) => di,
                };
                Box::new(counted!(di).filter_map(move |entity| {
                    Some((
                        entity,
                        (
                            fetch_non_driver!($first_flag, $First.1, entity),
                            $(fetch_non_driver!($rest_flag, $Rest.1, entity),)+
                        ),
                    ))
                }))
            }
        }
    };

    (@readonly $first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)+) => {
        impl<$First: Component, $($Rest: Component),+>
            ReadOnlyQueryData for (decl_type!($first_flag $First, '_), $(decl_type!($rest_flag $Rest, '_),)+)
        {
            #[allow(non_snake_case)]
            fn iter_ref<'s, 'w>(
                state: &'s Self::State<'w>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)+) = state;
                // The first element is `R` in any read-only shape (the
                // Cartesian forbids `O` first, and this arm fires only with no
                // `W`/`OW`), so it drives off its own storage.
                let driver: Box<dyn Iterator<Item = (Entity, &'s $First)> + 's> = match $First {
                    Some(s) => Box::new(s.iter()),
                    None => Box::new(std::iter::empty()),
                };
                Box::new(counted!(driver).filter_map(move |(entity, item_first)| {
                    Some((
                        entity,
                        (
                            item_first,
                            $(fetch_ro!($rest_flag, $Rest, entity),)+
                        ),
                    ))
                }))
            }

            #[allow(non_snake_case)]
            fn lookup<'s, 'w>(
                state: &'s Self::State<'w>,
                entity: Entity,
            ) -> Option<Self::Item<'s>>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)+) = state;
                Some((
                    fetch_ro!($first_flag, $First, entity),
                    $(fetch_ro!($rest_flag, $Rest, entity),)+
                ))
            }

            // `unused_variables`: `slice_ro!` ignores its binding for optional
            // positions (it yields `&[][..]`), so an `O` element's name is
            // unused in the `slices` array below.
            #[allow(non_snake_case, unused_variables)]
            fn drive_ref<'s, 'w>(
                state: &'s Self::State<'w>,
                driver: DriveSource<'s>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                if let DriveSource::Data(0) = driver {
                    return Self::iter_ref(state);
                }
                let di: DriverIter<'s> = match driver {
                    DriveSource::Data(k) => {
                        let ($First, $($Rest,)+) = state;
                        // Optionals contribute an empty slice (`slice_ro!`); they
                        // never win `min_data_candidate`, so `k` is always a
                        // required index here.
                        let slices = [slice_ro!($first_flag, $First), $(slice_ro!($rest_flag, $Rest),)+];
                        DriverIter::new(slices[k])
                    }
                    DriveSource::External(di) => di,
                };
                Box::new(counted!(di).filter_map(move |entity| {
                    Self::lookup(state, entity).map(|item| (entity, item))
                }))
            }
        }
    };

    // Entry: emit QueryData always; emit ReadOnlyQueryData iff every flag
    // is read-only (the `emit_readonly!` scan decides).
    ($first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)+) => {
        impl_one_combo_opt!(@gen $first_flag $First, $($rest_flag $Rest,)+);
        emit_readonly!(
            @scan [$first_flag $First, $($rest_flag $Rest,)+]
            -> [$first_flag $First, $($rest_flag $Rest,)+]
        );
    };
}

// ---- impl_all_tuple_opt_cartesian: first {R,W}, rest {R,W,O,OW} -------
//
// Like impl_all_tuple_cartesian! but the first slot is restricted to the
// required flags and the rest admit the optional flags. The `y`/`n`
// sentinel tracks whether any optional has been chosen so the base case
// can skip pure-`R`/`W` combinations (already covered by impl_all_tuple!).
macro_rules! impl_all_tuple_opt_cartesian {
    // ≥1 optional → emit; pure R/W → skip (impl_all_tuple! owns it).
    (@done y [$($acc:tt)*]) => { impl_one_combo_opt!($($acc)*); };
    (@done n [$($acc:tt)*]) => {};
    // First slot: required only.
    (@first $Head:ident, $($Tail:ident,)*) => {
        impl_all_tuple_opt_cartesian!(@rest n [R $Head,] $($Tail,)*);
        impl_all_tuple_opt_cartesian!(@rest n [W $Head,] $($Tail,)*);
    };
    // Trailing slot: R/W keep the sentinel, O/OW flip it to `y`.
    (@rest $opt:tt [$($acc:tt)*] $Head:ident, $($Tail:ident,)*) => {
        impl_all_tuple_opt_cartesian!(@rest $opt [$($acc)* R $Head,] $($Tail,)*);
        impl_all_tuple_opt_cartesian!(@rest $opt [$($acc)* W $Head,] $($Tail,)*);
        impl_all_tuple_opt_cartesian!(@rest y [$($acc)* O $Head,] $($Tail,)*);
        impl_all_tuple_opt_cartesian!(@rest y [$($acc)* OW $Head,] $($Tail,)*);
    };
    // Trailing slots exhausted.
    (@rest $opt:tt [$($acc:tt)*]) => {
        impl_all_tuple_opt_cartesian!(@done $opt [$($acc)*]);
    };
}

/// Emits every optional-bearing `QueryData` impl (and `ReadOnlyQueryData`
/// for the all-read shapes) for a tuple of the given type parameters: the
/// first element required (`&T`/`&mut T`), the rest any of
/// `&T`/`&mut T`/`Option<&T>`/`Option<&mut T>`, restricted to combinations
/// with at least one optional.
macro_rules! impl_all_tuple_opt {
    ($A:ident, $($B:ident),+) => {
        impl_all_tuple_opt_cartesian!(@first $A, $($B,)+);
    };
}

impl_all_tuple_opt!(A, B);
impl_all_tuple_opt!(A, B, C);

// ---- Entity-prefixed tuples: Query<(Entity, &A, …)> ------------------
//
// A parallel family beside impl_one_combo! / impl_all_tuple!, for shapes
// whose FIRST element is `Entity` followed by 1..=3 components. It reuses
// every per-flag helper (decl_type!, item_type!, state_type!,
// init_storage!, access_call!, build_non_driver_fetch!, fetch_non_driver!,
// drive_iter!) unchanged — only the impl target, the `Item` type, and the
// yielded tuple gain a leading `Entity`.
//
// `Entity` has no storage and cannot drive a join: the FIRST COMPONENT
// drives (its storage is walked) and `Entity` rides the entity id the
// driver already threads in every `(entity, item)` pair. The closure emits
// `(entity, (entity, item_first, rest..))` — the outer entity is the
// trait-threaded key (the filter consumes it, `Query::iter` strips it),
// the inner is the id the caller named. `Entity` adds nothing to `State`
// and nothing to `collect_access` — the generated `collect_access` still
// reports every *component* access, so the self-conflict check fires as
// usual; only `Entity` itself is invisible to it.
//
// The "rest" is `*` (zero-or-more), so a single component — `(Entity, &A)`
// — is expressible; its `State` is then a 1-tuple `(Option<Ref<…>>,)`. The
// all-R arm is listed first so all-read shapes also get `ReadOnlyQueryData`
// (first-match-wins: any `&mut` falls through to the mixed arm, which emits
// only `QueryData`).

// NOTE the `*` (zero-or-more) on the "rest", where `impl_one_combo!` uses
// `+`: the entity family must cover arity 1 — `(Entity, &A)`, which has a
// first component and *no* rest — whereas the plain family delegates its
// arity-1 case to the `&T` / `&mut T` impls and so never needs zero-rest.
macro_rules! impl_one_combo_entity {
    // All-R: also emit ReadOnly.
    (R $First:ident, $(R $Rest:ident,)*) => {
        impl_one_combo_entity!(@gen R $First, $(R $Rest,)*);
        impl_one_combo_entity!(@readonly $First $(, $Rest)*);
    };
    // Mixed (at least one W): only QueryData.
    ($first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)*) => {
        impl_one_combo_entity!(@gen $first_flag $First, $($rest_flag $Rest,)*);
    };

    (@gen $first_flag:ident $First:ident, $($rest_flag:ident $Rest:ident,)*) => {
        #[allow(unsafe_code)]
        impl<$First: Component, $($Rest: Component),*>
            QueryData
            for (Entity, decl_type!($first_flag $First, '_), $(decl_type!($rest_flag $Rest, '_),)*)
        {
            type Item<'w>
                = (Entity, item_type!($first_flag $First, 'w), $(item_type!($rest_flag $Rest, 'w),)*)
            where
                Self: 'w;
            // No `Entity` in State — it rides the driver's threaded id.
            type State<'w>
                = (state_type!($first_flag $First, 'w), $(state_type!($rest_flag $Rest, 'w),)*)
            where
                Self: 'w;

            fn init_state<'w>(world: &'w World) -> Self::State<'w>
            where
                Self: 'w,
            {
                (init_storage!($first_flag $First, world), $(init_storage!($rest_flag $Rest, world),)*)
            }

            #[allow(non_snake_case)]
            fn iter<'s, 'w>(
                state: &'s mut Self::State<'w>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)*) = state;
                $(let $Rest = build_non_driver_fetch!($rest_flag, $Rest);)*
                let driver: Box<dyn Iterator<Item = (Entity, item_type!($first_flag $First, 's))> + 's> =
                    drive_iter!($first_flag, $First);
                Box::new(counted!(driver).filter_map(move |(entity, item_first)| {
                    Some((
                        entity,
                        (
                            entity,
                            item_first,
                            $(fetch_non_driver!($rest_flag, $Rest, entity),)*
                        ),
                    ))
                }))
            }

            fn collect_access(access: &mut QueryAccess) {
                access_call!($first_flag $First, access);
                $(access_call!($rest_flag $Rest, access);)*
            }

            #[allow(non_snake_case)]
            fn min_data_candidate<'w>(state: &Self::State<'w>) -> Option<(usize, usize)>
            where
                Self: 'w,
            {
                let ($First, $($Rest,)*) = state;
                // Components only — `Entity` (global index 0) offers no
                // candidate. Local array index `i` maps to global index
                // `i + 1`; first minimum wins ties.
                let pops = [len_of!($First), $(len_of!($Rest),)*];
                let mut best = (1usize, pops[0]);
                for (i, &p) in pops.iter().enumerate().skip(1) {
                    if p < best.1 {
                        best = (i + 1, p);
                    }
                }
                Some(best)
            }

            #[allow(non_snake_case)]
            fn drive<'s, 'w>(
                state: &'s mut Self::State<'w>,
                driver: DriveSource<'s>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                // First *component* (global index 1) is the natural driver.
                if let DriveSource::Data(1) = driver {
                    return Self::iter(state);
                }
                let ($First, $($Rest,)*) = state;
                // `build_elem!` yields `(entity slice, fetch handle)` per
                // element; shadow each ident with that pair (so `$First.1` is
                // now the lookup handle, not the `Option<Ref<…>>` state).
                let $First = build_elem!($first_flag, $First);
                $(let $Rest = build_elem!($rest_flag, $Rest);)*
                // Slot 0 is `Entity` (no storage) — an empty placeholder keeps
                // the array indexed by the global element index.
                let slices = [&[][..], $First.0, $($Rest.0,)*];
                let di: DriverIter<'s> = match driver {
                    DriveSource::Data(k) => DriverIter::new(slices[k]),
                    DriveSource::External(di) => di,
                };
                Box::new(counted!(di).filter_map(move |entity| {
                    Some((
                        entity,
                        (
                            entity,
                            fetch_non_driver!($first_flag, $First.1, entity),
                            $(fetch_non_driver!($rest_flag, $Rest.1, entity),)*
                        ),
                    ))
                }))
            }
        }
    };

    (@readonly $First:ident $(, $Rest:ident)*) => {
        impl<$First: Component, $($Rest: Component),*>
            ReadOnlyQueryData for (Entity, &$First, $(&$Rest,)*)
        {
            #[allow(non_snake_case)]
            fn iter_ref<'s, 'w>(
                state: &'s Self::State<'w>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)*) = state;
                let driver: Box<dyn Iterator<Item = (Entity, &'s $First)> + 's> = match $First {
                    Some(s) => Box::new(s.iter()),
                    None => Box::new(std::iter::empty()),
                };
                Box::new(counted!(driver).filter_map(move |(entity, item_first)| {
                    Some((
                        entity,
                        (
                            entity,
                            item_first,
                            $($Rest.as_ref()?.get(entity)?,)*
                        ),
                    ))
                }))
            }

            #[allow(non_snake_case)]
            fn lookup<'s, 'w>(
                state: &'s Self::State<'w>,
                entity: Entity,
            ) -> Option<Self::Item<'s>>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                let ($First, $($Rest,)*) = state;
                Some((
                    entity,
                    $First.as_ref()?.get(entity)?,
                    $($Rest.as_ref()?.get(entity)?,)*
                ))
            }

            #[allow(non_snake_case)]
            fn drive_ref<'s, 'w>(
                state: &'s Self::State<'w>,
                driver: DriveSource<'s>,
            ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
            where
                Self: 's,
                Self: 'w,
                'w: 's,
            {
                if let DriveSource::Data(1) = driver {
                    return Self::iter_ref(state);
                }
                let di: DriverIter<'s> = match driver {
                    DriveSource::Data(k) => {
                        let ($First, $($Rest,)*) = state;
                        let slices = [&[][..], slice_of_read!($First), $(slice_of_read!($Rest),)*];
                        DriverIter::new(slices[k])
                    }
                    DriveSource::External(di) => di,
                };
                Box::new(counted!(di).filter_map(move |entity| {
                    Self::lookup(state, entity).map(|item| (entity, item))
                }))
            }
        }
    };
}

macro_rules! impl_all_tuple_entity_cartesian {
    (@start [$($acc:tt)*]) => {
        impl_one_combo_entity!($($acc)*);
    };
    (@start [$($acc:tt)*] $Head:ident, $($Tail:ident,)*) => {
        impl_all_tuple_entity_cartesian!(@start [$($acc)* R $Head,] $($Tail,)*);
        impl_all_tuple_entity_cartesian!(@start [$($acc)* W $Head,] $($Tail,)*);
    };
}

/// Emits every `QueryData` impl (and the `ReadOnlyQueryData` impl for the
/// all-read combination) for `(Entity, …)` at *every* `&` / `&mut`
/// combination of the trailing components.
///
/// One line per trailing arity. `impl_all_tuple_entity!(A, B)` generates
/// 2^2 = 4 impls: `(Entity, &A, &B)`, `(Entity, &mut A, &B)`,
/// `(Entity, &A, &mut B)`, `(Entity, &mut A, &mut B)`. A single ident —
/// `impl_all_tuple_entity!(A)` — is allowed and yields the `(Entity, &A)` /
/// `(Entity, &mut A)` pair. Extending past `(Entity, &A, &B, &C)` is one
/// more line, at the usual doubling monomorphisation cost.
macro_rules! impl_all_tuple_entity {
    ($A:ident $(, $B:ident)*) => {
        impl_all_tuple_entity_cartesian!(@start [] $A, $($B,)*);
    };
}

impl_all_tuple_entity!(A); // (Entity, &A) / (Entity, &mut A)
impl_all_tuple_entity!(A, B); // (Entity, &A, &B) + all &/&mut combos
impl_all_tuple_entity!(A, B, C); // (Entity, &A, &B, &C) + all combos

// -------- Query<'w, D> --------

/// System parameter that walks entities matching a data shape `D`.
///
/// Holds the [`QueryData::State`] for the duration of iteration so the
/// underlying `RefCell` borrows on each storage live as long as the
/// query does. Two `Query<&A>` over the same `A` in one system coexist
/// (shared borrows stack); two `Query<&mut A>` over the same `A` panic
/// on the second fetch (the `RefCell` "already borrowed" rule). Until
/// the M4 scheduler hoists conflict detection to registration time,
/// the runtime panic is the safety net.
///
/// # Iteration shape
///
/// `iter()` / `iter_mut()` yield `D::Item<'_>` directly — no
/// `(Entity, …)` prefix. A system that needs the entity asks for it
/// explicitly by naming `Entity` in the shape — `Query<(Entity, &T)>`,
/// or `Query<Entity>` for the id alone (see the [`Entity`] impl). See
/// the module-level docs for the *path A* vs *path B* design note.
///
/// # Filters
///
/// The second generic `F: QueryFilter` defaults to `()` (always true),
/// so `Query<D>` means `Query<D, ()>` and every existing call site is
/// unchanged. A non-default `F` narrows which entities iterate without
/// changing the yielded item: `Query<&Position, With<Powered>>` still
/// yields `&Position`. See the `With` / `Without` / `And` / `Or` filters.
///
/// # Examples
///
/// Single-component read — yields `&Position`, not `(Entity, &Position)`:
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
///
/// let mut world = World::new();
/// world.spawn().insert(Position { x: 1.0, y: 2.0 });
/// world.spawn().insert(Position { x: 3.0, y: 4.0 });
///
/// let q = Query::<&Position>::from_world(&world);
/// assert_eq!(q.iter().count(), 2);
/// let xs: Vec<f32> = q.iter().map(|p| p.x).collect();
/// assert!(xs.contains(&1.0));
/// assert!(xs.contains(&3.0));
/// ```
///
/// Two-component join — yields `(&mut Position, &Velocity)`:
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
///
/// let mut world = World::new();
/// world.spawn()
///     .insert(Position { x: 0.0, y: 0.0 })
///     .insert(Velocity { x: 1.0, y: 0.5 });
/// // Lonely Position with no Velocity — must be skipped by the join.
/// world.spawn().insert(Position { x: 99.0, y: 99.0 });
///
/// {
///     let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
///     let mut touched = 0;
///     for (mut pos, vel) in q.iter_mut() {
///         pos.x += vel.x;
///         pos.y += vel.y;
///         touched += 1;
///     }
///     assert_eq!(touched, 1);
/// }
///
/// // The lonely Position is untouched; the joined entity moved.
/// let xs: Vec<f32> = Query::<&Position>::from_world(&world)
///     .iter()
///     .map(|p| p.x)
///     .collect();
/// assert!(xs.contains(&1.0));
/// assert!(xs.contains(&99.0));
/// ```
pub struct Query<'w, D: QueryData + 'w, F: QueryFilter + 'w = ()> {
    /// The data shape's storage borrows, held for the query's lifetime.
    state: D::State<'w>,
    /// The filter's storage borrows + tick baselines, fetched once at
    /// [`from_world`](Self::from_world) (issue #65 moved this here from a
    /// per-`iter` fetch so a filter-driven loop can borrow its candidate
    /// slice at query lifetime). Shared with `state` against the `RefCell`
    /// rules — `With<A>` + `&mut A` is caught earlier by the self-conflict
    /// check; the only behavior shift is that the nonsensical
    /// `Query<&mut T, Without<T>>` now panics here at construction rather
    /// than at first iteration.
    filter_state: F::State<'w>,
    /// The driver chosen once from the data + filter candidate populations.
    /// Replayed on every `iter` / `iter_mut`; never recomputed per call.
    plan: DriverPlan,
    /// The `Or`-union driver, materialized **once** at
    /// [`from_world`](Self::from_world) and owned for the query's lifetime, so
    /// `iter` / `iter_mut` borrow a slice of it rather than re-sorting and
    /// re-allocating the union on every call. `None` unless the filter both
    /// drives *and* has no single-storage candidate (i.e. it is an `Or`).
    /// Frozen at construction, like the `Query<Entity>` live snapshot.
    materialized_driver: Option<Vec<Entity>>,
}

/// Builds the entity stream for a filter-driven query: a borrowed slice into a
/// single-storage candidate (`With`/`Changed`/`Added`/`And`), or into the
/// `Or` union materialized once at `from_world` and passed here as
/// `materialized`. Only reached when [`QueryFilter::candidate_len`] was
/// `Some`, so one branch always applies.
fn build_filter_driver<'s, F: QueryFilter>(
    filter_state: &'s F::State<'_>,
    materialized: Option<&'s [Entity]>,
) -> DriverIter<'s> {
    if let Some(slice) = F::candidate_slice(filter_state) {
        DriverIter::new(slice)
    } else {
        // No single-storage candidate ⇒ an `Or`, whose union was materialized
        // at construction (its `candidate_len` was `Some`, so it must exist).
        DriverIter::new(materialized.expect("Or driver materialized at from_world"))
    }
}

impl<'w, D: QueryData + 'w, F: QueryFilter + 'w> Query<'w, D, F> {
    /// Fetches a `Query` directly from a [`World`]. Convenience for
    /// tests and doc examples; system fns receive their `Query` from
    /// the runner via [`SystemParam::fetch`].
    ///
    /// Runs [`QueryAccess::assert_no_self_conflict`] on the combined
    /// data-shape *and* filter access set *before* touching any storage,
    /// so shapes like `(&mut A, &A)` or `(&mut A, &mut A)` panic with a
    /// precise message naming the offending component instead of the
    /// [`RefCell`]'s "already borrowed". Filters fold their access in via
    /// [`QueryFilter::collect_access`] — notably `With<A>` reports a read
    /// of `A`, so `Query<&mut A, With<A>>` is itself a self-conflict.
    ///
    /// [`RefCell`]: std::cell::RefCell
    ///
    /// # Panics
    ///
    /// - Panics if the combined access names the same component twice
    ///   with at least one `&mut` — within the data shape
    ///   (`(&mut A, &A)`, `(&mut A, &mut A)`, the reversed `(&A, &mut A)`)
    ///   or across data and filter (`&mut A` data + `With<A>`). The panic
    ///   originates from [`QueryAccess::assert_no_self_conflict`].
    /// - Panics when `D` contains a `&mut T` and that storage is already
    ///   borrowed (shared or exclusive) — the `RefCell` rule, fired
    ///   from a second concurrent query over the same storage in a
    ///   different `Query` value.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// world.spawn().insert(Health(100));
    ///
    /// let q = Query::<&Health>::from_world(&world);
    /// assert_eq!(q.iter().count(), 1);
    /// ```
    #[must_use]
    pub fn from_world(world: &'w World) -> Self {
        // Order matters: check before borrow. `init_state` calls
        // `world.storage_mut::<T>()` per `&mut T` in the shape, and
        // `(&mut A, &A)` would otherwise surface as the `RefCell`'s
        // generic "already borrowed" rather than this module's
        // specific "query has conflicting access to component `A`"
        // diagnostic.
        let mut access = QueryAccess::default();
        D::collect_access(&mut access);
        F::collect_access(&mut access);
        access.assert_no_self_conflict();

        let state = D::init_state(world);
        let filter_state = F::init_state(world);

        // Freeze the driver once: the smallest candidate across the data
        // shape and the filter leads the loop. A tie favors the data shape
        // (earlier in `Query<D, F>` than the filter); within the data shape
        // the earliest element wins (see `min_data_candidate`). Reading each
        // population is O(1); the choice never depends on a per-`iter` lookup.
        let plan = match (
            D::min_data_candidate(&state),
            F::candidate_len(&filter_state),
        ) {
            // Nothing names a candidate (`Query<Entity>`, `…, Without<T>>`,
            // `…, ()>`): drive the live snapshot via the data shape's own
            // iter — the contract-exempt fallback.
            (None, None) => DriverPlan::LiveSet,
            (None, Some(_)) => DriverPlan::Filter,
            (Some((idx, _)), None) => DriverPlan::Data(idx),
            (Some((idx, data_pop)), Some(filter_pop)) => {
                if data_pop <= filter_pop {
                    DriverPlan::Data(idx)
                } else {
                    DriverPlan::Filter
                }
            }
        };

        // If the filter drives via a composite candidate (an `Or` union),
        // materialize it once here; `iter` / `iter_mut` then borrow it instead
        // of rebuilding the sorted, deduplicated union on every call.
        // Single-storage filters return `None` (they drive via a borrowed
        // slice), and so do non-filter plans.
        let materialized_driver = match plan {
            DriverPlan::Filter => F::candidate_materialize(&filter_state),
            DriverPlan::Data(_) | DriverPlan::LiveSet => None,
        };

        Self {
            state,
            filter_state,
            plan,
            materialized_driver,
        }
    }

    /// Exclusive iteration. Works for any `D: QueryData`, including
    /// `&mut T` and tuples containing a `&mut T`.
    ///
    /// Yields `D::Item<'_>` directly — no `(Entity, …)` prefix.
    /// Path B; see the module-level docs.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Velocity { x: f32, y: f32 }
    ///
    /// let mut world = World::new();
    /// world.spawn().insert(Velocity { x: 1.0, y: 0.0 });
    /// world.spawn().insert(Velocity { x: 0.0, y: 1.0 });
    ///
    /// {
    ///     // Scope the exclusive query so its `RefMut` drops before the
    ///     // shared read below — two outstanding borrows of the same
    ///     // storage cell (one mut, one shared) would panic.
    ///     let mut q = Query::<&mut Velocity>::from_world(&world);
    ///     for mut v in q.iter_mut() {
    ///         v.x *= 2.0;
    ///         v.y *= 2.0;
    ///     }
    /// }
    ///
    /// let sums: f32 = Query::<&Velocity>::from_world(&world)
    ///     .iter()
    ///     .map(|v| v.x + v.y)
    ///     .sum();
    /// assert!((sums - 4.0).abs() < f32::EPSILON);
    /// ```
    ///
    /// Multi-mut join — both elements mutable, distinct types:
    ///
    /// ```
    /// use spark_ecs::{Component, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Position { x: f32, y: f32 }
    /// #[derive(Component)]
    /// struct Velocity { x: f32, y: f32 }
    ///
    /// let mut world = World::new();
    /// world.spawn()
    ///     .insert(Position { x: 0.0, y: 0.0 })
    ///     .insert(Velocity { x: 1.0, y: 0.5 });
    ///
    /// {
    ///     let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
    ///     for (mut pos, mut vel) in q.iter_mut() {
    ///         pos.x += vel.x;
    ///         pos.y += vel.y;
    ///         // Both sides mutable — apply drag on the way out.
    ///         vel.x *= 0.9;
    ///         vel.y *= 0.9;
    ///     }
    /// }
    ///
    /// let (vx, vy) = Query::<&Velocity>::from_world(&world)
    ///     .iter()
    ///     .map(|v| (v.x, v.y))
    ///     .next()
    ///     .unwrap();
    /// assert!((vx - 0.9).abs() < 1e-6);
    /// assert!((vy - 0.45).abs() < 1e-6);
    /// ```
    pub fn iter_mut(&mut self) -> impl Iterator<Item = D::Item<'_>> + '_ {
        // The driver was chosen at construction; `state` (mut), `filter_state`
        // and `materialized_driver` (shared) are disjoint fields, so the
        // driver and the per-entity `matches` reject borrow them side by side.
        let filter_state = &self.filter_state;
        let materialized = self.materialized_driver.as_deref();
        let driven = match self.plan {
            DriverPlan::LiveSet => D::iter(&mut self.state),
            DriverPlan::Data(idx) => D::drive(&mut self.state, DriveSource::Data(idx)),
            DriverPlan::Filter => {
                let driver = build_filter_driver::<F>(filter_state, materialized);
                D::drive(&mut self.state, DriveSource::External(driver))
            }
        };
        // Path B — strip the entity that the trait threads internally for
        // join logic; the filter consumes it first. See module docs.
        driven
            .filter(move |(entity, _)| F::matches(*entity, filter_state))
            .map(|(_entity, item)| item)
    }
}

impl<'w, D: ReadOnlyQueryData + 'w, F: QueryFilter + 'w> Query<'w, D, F> {
    /// Shared iteration. Available only for `D: ReadOnlyQueryData`
    /// (no `&mut T` anywhere in the shape).
    ///
    /// Yields `D::Item<'_>` directly — no `(Entity, …)` prefix.
    /// Path B; see the module-level docs.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Position { x: f32, y: f32 }
    /// #[derive(Component)]
    /// struct Velocity { x: f32, y: f32 }
    ///
    /// let mut world = World::new();
    /// world.spawn()
    ///     .insert(Position { x: 0.0, y: 0.0 })
    ///     .insert(Velocity { x: 1.0, y: 0.0 });
    ///
    /// let q = Query::<(&Position, &Velocity)>::from_world(&world);
    /// for (pos, vel) in q.iter() {
    ///     assert_eq!(pos.x + vel.x, 1.0);
    ///     assert_eq!(pos.y + vel.y, 0.0);
    /// }
    /// ```
    ///
    /// `.iter()` is gated on [`ReadOnlyQueryData`]. A query containing
    /// `&mut` does not implement that supertrait, so the call is a
    /// **compile-time** error — not a runtime check:
    ///
    /// ```compile_fail
    /// use spark_ecs::{Component, Query, World};
    ///
    /// // `Position` *is* a component here — so `insert` compiles and the
    /// // failure lands squarely on `q.iter()` below (the bound this
    /// // example is about), not on an undecorated-component error.
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// world.spawn().insert(Position(0.0, 0.0));
    /// let q = Query::<&mut Position>::from_world(&world);
    /// // error[E0599]: the method `iter` exists for struct
    /// // `Query<&mut Position>`, but its trait bounds were not
    /// // satisfied: `&mut Position: ReadOnlyQueryData`.
    /// for _ in q.iter() {}
    /// ```
    ///
    /// The gate applies to entity-prefixed shapes too: only all-read
    /// `(Entity, &A, …)` tuples are [`ReadOnlyQueryData`], so a tuple with
    /// a `&mut` element cannot use `.iter()` (use `.iter_mut()`):
    ///
    /// ```compile_fail
    /// use spark_ecs::{Component, Entity, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// world.spawn().insert(Position(0.0, 0.0));
    /// let q = Query::<(Entity, &mut Position)>::from_world(&world);
    /// // error: `(Entity, &mut Position): ReadOnlyQueryData` is not satisfied.
    /// for _ in q.iter() {}
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = D::Item<'_>> + '_ {
        // Path B — see `Query::iter_mut`. Shared mirror: every borrow here is
        // shared, so the driver slice and the `matches` reject coexist freely.
        let filter_state = &self.filter_state;
        let materialized = self.materialized_driver.as_deref();
        let driven = match self.plan {
            DriverPlan::LiveSet => D::iter_ref(&self.state),
            DriverPlan::Data(idx) => D::drive_ref(&self.state, DriveSource::Data(idx)),
            DriverPlan::Filter => {
                let driver = build_filter_driver::<F>(filter_state, materialized);
                D::drive_ref(&self.state, DriveSource::External(driver))
            }
        };
        driven
            .filter(move |(entity, _)| F::matches(*entity, filter_state))
            .map(|(_entity, item)| item)
    }
}

impl<D: QueryData, F: QueryFilter> SystemParam for Query<'_, D, F> {
    type Item<'w>
        = Query<'w, D, F>
    where
        Self: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        Self: 'w,
    {
        Query::from_world(world)
    }
    fn collect_access(access: &mut Access) {
        // Replay the data shape and filter through the *same*
        // `collect_access` calls that `Query::from_world` uses for the
        // per-query self-conflict check (`&T`→read, `&mut T`→write,
        // `With<T>`→read, `Without<T>`→nothing) — but record them in the
        // system's component set instead of a throwaway one.
        D::collect_access(access.components_mut());
        F::collect_access(access.components_mut());
    }
}

/// `for x in &q` — shared-iteration sugar over [`Query::iter`].
///
/// `for x in &q` desugars to `IntoIterator::into_iter(&q)`, which calls
/// [`Query::iter`]. It therefore carries the same [`ReadOnlyQueryData`]
/// gate: a shape containing `&mut T` is rejected at compile time, never
/// walked through a shared borrow. Yielded items match `iter` exactly —
/// the data shape, no `Entity` prefix (path B; see the module docs).
///
/// Boxes the iterator — use [`Query::iter`] directly for hot paths or
/// adapter chains (`q.iter().map(…)`); the crate README covers the cost.
/// By-value `IntoIterator for Query` is deliberately absent — consuming
/// the query would drop its [`Ref`] storage guards mid-walk.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
///
/// let mut world = World::new();
/// world.spawn()
///     .insert(Position { x: 0.0, y: 0.0 })
///     .insert(Velocity { x: 1.0, y: 0.5 });
///
/// let q = Query::<(&Position, &Velocity)>::from_world(&world);
/// for (pos, vel) in &q {
///     assert_eq!(pos.x + vel.x, 1.0);
/// }
/// ```
///
/// The gate is compile-time, mirroring [`Query::iter`]: a `&mut` shape
/// cannot be iterated through `&q`.
///
/// ```compile_fail
/// use spark_ecs::{Component, Query, World};
///
/// // `Position` is a component, so `insert` compiles and the failure
/// // lands on `for _ in &q` — the `ReadOnlyQueryData` bound this
/// // example is about.
/// #[derive(Component)]
/// struct Position(f32, f32);
///
/// let mut world = World::new();
/// world.spawn().insert(Position(0.0, 0.0));
/// let q = Query::<&mut Position>::from_world(&world);
/// // error[E0277]: `&mut Position: ReadOnlyQueryData` is not satisfied.
/// for _ in &q {}
/// ```
impl<'q, 'w, D: ReadOnlyQueryData + 'w, F: QueryFilter> IntoIterator for &'q Query<'w, D, F> {
    type Item = D::Item<'q>;
    type IntoIter = Box<dyn Iterator<Item = D::Item<'q>> + 'q>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter())
    }
}

/// `for x in &mut q` — exclusive-iteration sugar over [`Query::iter_mut`].
///
/// `for x in &mut q` desugars to `IntoIterator::into_iter(&mut q)`, which
/// calls [`Query::iter_mut`]. It works for any `D: QueryData`, including
/// `&mut T` shapes, and yields the data shape directly (path B).
///
/// Boxes the iterator — use [`Query::iter_mut`] directly for hot paths or
/// adapter chains; the crate README covers the cost.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
///
/// let mut world = World::new();
/// world.spawn()
///     .insert(Position { x: 0.0, y: 0.0 })
///     .insert(Velocity { x: 1.0, y: 0.5 });
///
/// {
///     let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
///     for (mut pos, vel) in &mut q {
///         pos.x += vel.x;
///         pos.y += vel.y;
///     }
/// }
///
/// let (x, y) = Query::<&Position>::from_world(&world)
///     .iter()
///     .map(|p| (p.x, p.y))
///     .next()
///     .unwrap();
/// assert!((x - 1.0).abs() < f32::EPSILON);
/// assert!((y - 0.5).abs() < f32::EPSILON);
/// ```
impl<'q, 'w, D: QueryData + 'w, F: QueryFilter> IntoIterator for &'q mut Query<'w, D, F> {
    type Item = D::Item<'q>;
    type IntoIter = Box<dyn Iterator<Item = D::Item<'q>> + 'q>;
    fn into_iter(self) -> Self::IntoIter {
        Box::new(self.iter_mut())
    }
}

#[cfg(test)]
#[allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    reason = "test fns live next to their assertions; system fns take \
              `Query` by value to match how plugins write systems."
)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use super::*;
    use crate::Commands;
    use crate::Component;
    use crate::filter::{And, Or, With, Without};
    use crate::system::IntoSystem;

    // Integer fields keep unit tests free of `clippy::float_cmp`
    // assertions. Doc tests stay with the canonical `f32` flavour to
    // read like real engine code.
    #[derive(Debug, PartialEq, Component)]
    struct Position(i32, i32);

    #[derive(Debug, PartialEq, Component)]
    struct Velocity(i32, i32);

    #[derive(Debug, PartialEq, Component)]
    struct Marker;

    fn world_with_three_movers() -> (World, [Entity; 3]) {
        let mut world = World::new();
        let a = world
            .spawn()
            .insert(Position(0, 0))
            .insert(Velocity(1, 0))
            .id();
        let b = world
            .spawn()
            .insert(Position(10, 10))
            .insert(Velocity(0, 1))
            .id();
        let c = world
            .spawn()
            .insert(Position(20, 20))
            .insert(Velocity(1, 1))
            .id();
        (world, [a, b, c])
    }

    #[test]
    fn query_ref_single_component_walks_every_entity() {
        let (world, _) = world_with_three_movers();
        let q = Query::<&Position>::from_world(&world);
        assert_eq!(q.iter().count(), 3);
        let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
        assert_eq!(xs, vec![0, 10, 20]);
    }

    #[test]
    fn query_mut_single_component_mutates_in_place() {
        let (world, _) = world_with_three_movers();
        {
            let mut q = Query::<&mut Position>::from_world(&world);
            for mut p in q.iter_mut() {
                p.0 += 100;
            }
        }
        let after: Vec<i32> = Query::<&Position>::from_world(&world)
            .iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(after, vec![100, 110, 120]);
    }

    #[test]
    fn into_iter_ref_sugar_walks_every_entity() {
        let (world, _) = world_with_three_movers();
        let q = Query::<&Position>::from_world(&world);
        // `for … in &q` is the `IntoIterator for &Query` sugar — same
        // result as `q.iter()`, just without the explicit call.
        let mut xs = Vec::new();
        for p in &q {
            xs.push(p.0);
        }
        assert_eq!(xs, vec![0, 10, 20]);
    }

    #[test]
    fn into_iter_mut_sugar_mutates_in_place() {
        let (world, _) = world_with_three_movers();
        {
            let mut q = Query::<&mut Position>::from_world(&world);
            // `for … in &mut q` is the `IntoIterator for &mut Query` sugar.
            for mut p in &mut q {
                p.0 += 100;
            }
        }
        let after: Vec<i32> = Query::<&Position>::from_world(&world)
            .iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(after, vec![100, 110, 120]);
    }

    #[test]
    fn query_two_tuple_yields_intersection_only() {
        let mut world = World::new();
        // Has both Position and Velocity — in the join.
        let _movers = [
            world
                .spawn()
                .insert(Position(0, 0))
                .insert(Velocity(1, 0))
                .id(),
            world
                .spawn()
                .insert(Position(5, 5))
                .insert(Velocity(0, 1))
                .id(),
        ];
        // Has only Position — must be skipped.
        let _lonely = world.spawn().insert(Position(99, 99)).id();
        // Has only Velocity — must be skipped.
        let _phantom = world.spawn().insert(Velocity(7, 7)).id();

        // Path B: the iterator yields data only, no `Entity`. Identify
        // each yielded pair by its values (every entity has unique
        // spawn data in this test).
        let q = Query::<(&Position, &Velocity)>::from_world(&world);
        let pairs: Vec<(Position, Velocity)> = q
            .iter()
            .map(|(p, v)| (Position(p.0, p.1), Velocity(v.0, v.1)))
            .collect();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.contains(&(Position(0, 0), Velocity(1, 0))));
        assert!(pairs.contains(&(Position(5, 5), Velocity(0, 1))));
        // `lonely`'s Position(99, 99) is unjoined — its values must
        // not appear in the join output.
        assert!(!pairs.iter().any(|(p, _)| p == &Position(99, 99)));
    }

    #[test]
    fn query_two_tuple_mut_drives_and_writes_through() {
        // The canonical movement example. Position and Velocity have equal
        // populations here (3 each), so the tie breaks to the first element:
        // Position (mut) drives, Velocity (shared) is sparse-looked-up. With
        // unequal populations the smaller would drive — see `driver_cost_tests`.
        let (world, entities) = world_with_three_movers();
        {
            let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
            for (mut pos, vel) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
            }
        }
        assert_eq!(*world.get::<Position>(entities[0]).unwrap(), Position(1, 0));
        assert_eq!(
            *world.get::<Position>(entities[1]).unwrap(),
            Position(10, 11)
        );
        assert_eq!(
            *world.get::<Position>(entities[2]).unwrap(),
            Position(21, 21)
        );
    }

    #[test]
    fn query_two_tuple_skips_entity_missing_second_component() {
        // E0 has both. E1 only has Position. The join must skip E1
        // even though Position drives the walk.
        let mut world = World::new();
        let e0 = world
            .spawn()
            .insert(Position(1, 1))
            .insert(Velocity(2, 2))
            .id();
        let e1 = world.spawn().insert(Position(99, 99)).id();
        {
            let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
            for (mut pos, vel) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
            }
        }
        assert_eq!(*world.get::<Position>(e0).unwrap(), Position(3, 3));
        assert_eq!(*world.get::<Position>(e1).unwrap(), Position(99, 99));
    }

    #[test]
    fn query_for_unknown_component_yields_empty_iter() {
        // No entity has ever held a Marker → the storage doesn't even
        // exist. The query must compile and produce an empty iterator,
        // not panic.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));

        let q = Query::<&Marker>::from_world(&world);
        assert_eq!(q.iter().count(), 0);

        let mut q_mut = Query::<&mut Marker>::from_world(&world);
        assert_eq!(q_mut.iter_mut().count(), 0);
    }

    #[test]
    fn two_shared_queries_over_same_type_coexist() {
        let (world, _) = world_with_three_movers();
        let q_a = Query::<&Position>::from_world(&world);
        let q_b = Query::<&Position>::from_world(&world);
        assert_eq!(q_a.iter().count(), q_b.iter().count());
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn two_mut_queries_over_same_type_panic_on_second_fetch() {
        let (world, _) = world_with_three_movers();
        let _q_a = Query::<&mut Position>::from_world(&world);
        let _q_b = Query::<&mut Position>::from_world(&world);
    }

    // Distinct unit-struct components for the higher-arity tuple tests.
    // Plain `i32` newtypes so equality checks stay clippy-clean.
    //
    // These are independent test fixtures — *not* related to the
    // `$first_flag $First, ...` macro variables in `impl_all_tuple!`.
    #[derive(Debug, PartialEq, Component)]
    struct A(i32);
    #[derive(Debug, PartialEq, Component)]
    struct B(i32);
    #[derive(Debug, PartialEq, Component)]
    struct C(i32);
    #[derive(Debug, PartialEq, Component)]
    struct D(i32);
    #[derive(Debug, PartialEq, Component)]
    struct E(i32);

    #[test]
    fn query_three_tuple_joins_three_storages() {
        let mut world = World::new();
        // E0 has A, B, C — should be yielded.
        world.spawn().insert(A(1)).insert(B(2)).insert(C(3));
        // E1 has A and B but no C — must be skipped.
        world.spawn().insert(A(10)).insert(B(20));
        // E2 has all three plus an extra D — should be yielded.
        world
            .spawn()
            .insert(A(100))
            .insert(B(200))
            .insert(C(300))
            .insert(D(400));

        // Path B: iterator yields data only — no `Entity` in the pair.
        let q = Query::<(&A, &B, &C)>::from_world(&world);
        let mut yielded = Vec::new();
        for (a, b, c) in q.iter() {
            yielded.push((a.0, b.0, c.0));
        }
        assert_eq!(yielded.len(), 2);
        assert!(yielded.contains(&(1, 2, 3)));
        assert!(yielded.contains(&(100, 200, 300)));
        // E1's values (A=10) must never appear — it was missing C.
        assert!(yielded.iter().all(|(a, _, _)| *a != 10));
    }

    #[test]
    fn query_four_tuple_joins_four_storages() {
        let mut world = World::new();
        // E0 has all four — yielded.
        world
            .spawn()
            .insert(A(1))
            .insert(B(2))
            .insert(C(3))
            .insert(D(4));
        // E1 missing D — must be skipped.
        world.spawn().insert(A(10)).insert(B(20)).insert(C(30));
        // E2 has all four — yielded.
        world
            .spawn()
            .insert(A(100))
            .insert(B(200))
            .insert(C(300))
            .insert(D(400));

        let q = Query::<(&A, &B, &C, &D)>::from_world(&world);
        let mut yielded = Vec::new();
        for (a, b, c, d) in q.iter() {
            yielded.push((a.0, b.0, c.0, d.0));
        }
        assert_eq!(yielded.len(), 2);
        assert!(yielded.contains(&(1, 2, 3, 4)));
        assert!(yielded.contains(&(100, 200, 300, 400)));
    }

    #[test]
    fn query_three_tuple_with_mut_driver_writes_through() {
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(0, 0))
            .insert(Velocity(2, 3))
            .insert(Marker)
            .id();
        // Missing Marker — skipped by the join even though Position
        // drives.
        world
            .spawn()
            .insert(Position(100, 100))
            .insert(Velocity(7, 7));
        {
            let mut q = Query::<(&mut Position, &Velocity, &Marker)>::from_world(&world);
            for (mut pos, vel, _marker) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
            }
        }
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(2, 3));
    }

    #[test]
    fn query_as_system_param_via_into_system() {
        // Smoke-test that the SystemParam impl wires up automatically
        // through #9's `IntoSystem` machinery. No explicit fetch call;
        // the system fn declares `Query<…>` and the runner builds it.
        let (world, entities) = world_with_three_movers();
        fn step(mut q: Query<(&mut Position, &Velocity)>) {
            for (mut pos, vel) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
            }
        }
        let mut sys = IntoSystem::into_system(step);
        sys(&world);
        sys(&world);
        assert_eq!(*world.get::<Position>(entities[0]).unwrap(), Position(2, 0));
        assert_eq!(
            *world.get::<Position>(entities[1]).unwrap(),
            Position(10, 12)
        );
        assert_eq!(
            *world.get::<Position>(entities[2]).unwrap(),
            Position(22, 22)
        );
    }

    // -------- Arity-2 multi-mut: (&mut A, &mut B) --------

    #[test]
    fn query_two_mut_tuple_writes_through_both_sides() {
        let (world, entities) = world_with_three_movers();
        {
            let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
            for (mut pos, mut vel) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
                // Mutate B too — proves the second slot really is `&mut`.
                vel.0 *= 2;
                vel.1 *= 2;
            }
        }
        assert_eq!(*world.get::<Position>(entities[0]).unwrap(), Position(1, 0));
        assert_eq!(
            *world.get::<Position>(entities[1]).unwrap(),
            Position(10, 11)
        );
        assert_eq!(
            *world.get::<Position>(entities[2]).unwrap(),
            Position(21, 21)
        );
        assert_eq!(*world.get::<Velocity>(entities[0]).unwrap(), Velocity(2, 0));
        assert_eq!(*world.get::<Velocity>(entities[1]).unwrap(), Velocity(0, 2));
        assert_eq!(*world.get::<Velocity>(entities[2]).unwrap(), Velocity(2, 2));
    }

    #[test]
    fn query_two_mut_tuple_skips_entity_missing_second_component() {
        let mut world = World::new();
        let e0 = world
            .spawn()
            .insert(Position(1, 1))
            .insert(Velocity(2, 2))
            .id();
        let e1 = world.spawn().insert(Position(99, 99)).id();
        {
            let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
            for (mut pos, mut vel) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
                vel.0 = -1;
                vel.1 = -1;
            }
        }
        assert_eq!(*world.get::<Position>(e0).unwrap(), Position(3, 3));
        assert_eq!(*world.get::<Velocity>(e0).unwrap(), Velocity(-1, -1));
        assert_eq!(*world.get::<Position>(e1).unwrap(), Position(99, 99));
        assert!(world.get::<Velocity>(e1).is_none());
    }

    #[test]
    fn query_two_mut_tuple_empty_when_either_storage_absent() {
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
        assert_eq!(q.iter_mut().count(), 0);
    }

    // -------- Self-conflict detection --------

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_two_mut_tuple_same_type_panics_with_named_component() {
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(&mut Position, &mut Position)>::from_world(&world);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_mut_plus_read_same_type_panics_with_named_component() {
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(&mut Position, &Position)>::from_world(&world);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_arity_three_self_conflict_panics() {
        // Arity-3 macro must propagate `collect_access` to every
        // element. A silent miss would slip past the conflict check
        // and double-borrow the driver storage cell.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0)).insert(Velocity(0, 0));
        let _q = Query::<(&mut Position, &Velocity, &Position)>::from_world(&world);
    }

    #[test]
    fn query_self_conflict_panic_originates_before_refcell_borrow() {
        // The conflict shapes must panic from
        // `QueryAccess::assert_no_self_conflict`, not the `RefCell`
        // borrow inside `init_state`. A future refactor that
        // accidentally reordered the check to run after `init_state`
        // would surface as "already borrowed" instead.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));

        for kind in ["write_write", "write_read", "read_write"] {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
                "write_write" => {
                    let _q = Query::<(&mut Position, &mut Position)>::from_world(&world);
                }
                "write_read" => {
                    let _q = Query::<(&mut Position, &Position)>::from_world(&world);
                }
                "read_write" => {
                    let _q = Query::<(&Position, &mut Position)>::from_world(&world);
                }
                _ => unreachable!(),
            }));
            let payload = result.expect_err("expected panic");
            let msg = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&'static str>().copied())
                .unwrap_or("");
            assert!(
                msg.contains("conflicting access to component"),
                "{kind}: wrong panic message: {msg}"
            );
            assert!(
                !msg.contains("already borrowed"),
                "{kind}: panic came from RefCell, not the self-conflict check: {msg}"
            );
        }
    }

    #[test]
    fn query_two_mut_tuple_collect_access_is_conflict_free() {
        // Two writes of distinct types — no self-conflict.
        let mut access = QueryAccess::default();
        <(&mut Position, &mut Velocity) as QueryData>::collect_access(&mut access);
        access.assert_no_self_conflict();
    }

    // -------- Optional fetch: Option<&T> / Option<&mut T> (issue #70) -----

    /// A join with an optional second element keeps every row the required
    /// element drives — present rows carry `Some`, absent rows carry `None`.
    #[test]
    fn query_optional_ref_yields_some_when_present_none_when_absent() {
        let mut world = World::new();
        let both = world
            .spawn()
            .insert(Position(1, 0))
            .insert(Velocity(2, 0))
            .id();
        let lonely = world.spawn().insert(Position(3, 0)).id(); // no Velocity

        let q = Query::<(&Position, Option<&Velocity>)>::from_world(&world);
        let rows: Vec<(i32, Option<i32>)> = q.iter().map(|(p, v)| (p.0, v.map(|v| v.0))).collect();

        assert_eq!(rows.len(), 2); // both rows kept — Option never gates
        assert!(rows.contains(&(1, Some(2)))); // `both` carries Velocity
        assert!(rows.contains(&(3, None))); // `lonely` is still yielded
        let _ = (both, lonely);
    }

    /// Standing alone, `Query<Option<&T>>` visits every live entity, with
    /// or without `T` — the same no-candidate / live-set path as
    /// `Query<Entity>`.
    #[test]
    fn query_optional_standalone_visits_every_live_entity() {
        let mut world = World::new();
        world.spawn().insert(Velocity(2, 0));
        world.spawn().insert(Position(1, 0)); // no Velocity
        world.spawn(); // no components at all

        let q = Query::<Option<&Velocity>>::from_world(&world);
        assert_eq!(q.iter().count(), 3); // every live entity, with or without Velocity
        assert_eq!(q.iter().flatten().count(), 1); // exactly one carries Velocity
    }

    /// Standalone mutable mirror: `Query<Option<&mut T>>` visits every live
    /// entity (snapshot-driven) and writes through the present ones — a
    /// distinct branch from the tuple-trailing `Option<&mut T>` cases.
    #[test]
    fn query_optional_standalone_mut_visits_all_and_writes_through() {
        let mut world = World::new();
        let has_v = world.spawn().insert(Velocity(5, 0)).id();
        world.spawn().insert(Position(1, 0)); // no Velocity
        world.spawn(); // no components

        {
            let mut q = Query::<Option<&mut Velocity>>::from_world(&world);
            assert_eq!(q.iter_mut().count(), 3); // every live entity
            for mut vel in q.iter_mut().flatten() {
                vel.0 = 99; // write-through on the present one only
            }
        }
        assert_eq!(world.get::<Velocity>(has_v).unwrap().0, 99);
    }

    /// `Option<&mut T>` writes through on entities that have `T`, and leaves
    /// the rest untouched while still yielding them.
    #[test]
    fn query_optional_mut_writes_through_present_only() {
        let mut world = World::new();
        let poisoned = world
            .spawn()
            .insert(Position(100, 0))
            .insert(Velocity(5, 0))
            .id();
        let clean = world.spawn().insert(Position(50, 0)).id(); // no Velocity

        let mut yielded = 0;
        {
            let mut q = Query::<(&mut Position, Option<&mut Velocity>)>::from_world(&world);
            for (mut pos, vel) in q.iter_mut() {
                yielded += 1;
                if let Some(mut v) = vel {
                    pos.0 -= v.0; // apply…
                    v.0 = 0; // …and consume
                }
            }
        }
        assert_eq!(yielded, 2); // both rows visited
        assert_eq!(world.get::<Position>(poisoned).unwrap().0, 95); // 100 - 5
        assert_eq!(world.get::<Velocity>(poisoned).unwrap().0, 0); // consumed
        assert_eq!(world.get::<Position>(clean).unwrap().0, 50); // untouched
    }

    /// `Option<&T>` is read-only, so an all-read optional shape implements
    /// `ReadOnlyQueryData` and `.iter()` is available.
    #[test]
    fn query_optional_read_shape_is_read_only() {
        fn assert_read_only<D: ReadOnlyQueryData>() {}
        assert_read_only::<(&Position, Option<&Velocity>)>();
        assert_read_only::<Option<&Position>>();
    }

    /// The optional reports its access, so `(&A, Option<&mut A>)` trips the
    /// per-query self-conflict check — a write+read conflict on `Position`,
    /// caught at construction before any storage is borrowed. (Issue #70 pins
    /// `(Option<&mut A>, &A)`; optional-first doesn't compile under the
    /// required-first rule, so this is the equivalent reachable shape.)
    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_optional_mut_self_conflict_panics() {
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(&Position, Option<&mut Position>)>::from_world(&world);
    }

    /// Driver and trailing optional both *write* the same component
    /// (`(&mut A, Option<&mut A>)`) — the write+write branch of the conflict
    /// check, distinct from the write+read case above; both reach it through
    /// the `O`/`OW` arms of `access_call!`.
    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_optional_mut_driver_and_opt_both_write_same_type_panics() {
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(&mut Position, Option<&mut Position>)>::from_world(&world);
    }

    /// Arity-3 with a trailing optional: the required intersection drives,
    /// the optional rides along as `Some`/`None`.
    #[test]
    fn query_optional_arity_three_required_intersection_plus_optional() {
        let mut world = World::new();
        // Has Position + Velocity + Marker.
        world
            .spawn()
            .insert(Position(1, 0))
            .insert(Velocity(1, 0))
            .insert(Marker);
        // Has Position + Velocity, no Marker.
        world.spawn().insert(Position(2, 0)).insert(Velocity(2, 0));
        // Has only Position — excluded (Velocity is required).
        world.spawn().insert(Position(3, 0));

        let q = Query::<(&Position, &Velocity, Option<&Marker>)>::from_world(&world);
        let rows: Vec<(i32, bool)> = q.iter().map(|(p, _v, m)| (p.0, m.is_some())).collect();

        assert_eq!(rows.len(), 2); // only the Position∩Velocity intersection
        assert!(rows.contains(&(1, true))); // Marker present
        assert!(rows.contains(&(2, false))); // Marker absent — still yielded
        assert!(!rows.iter().any(|(x, _)| *x == 3)); // no Velocity → excluded
    }

    /// The optional's *storage* never existing (no entity ever had it) is a
    /// distinct branch from "this entity lacks it": `init_state` returns
    /// `None`, so every lookup yields `None` — without panicking.
    #[test]
    fn query_optional_ref_absent_storage_yields_all_none() {
        let mut world = World::new();
        world.spawn().insert(Position(1, 0));
        world.spawn().insert(Position(2, 0));
        // Velocity storage was never created.
        let q = Query::<(&Position, Option<&Velocity>)>::from_world(&world);
        let rows: Vec<(i32, bool)> = q.iter().map(|(p, v)| (p.0, v.is_some())).collect();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(_, has_v)| !has_v)); // None for every row
    }

    /// Same absent-storage branch for the mutable optional (`view` is `None`).
    #[test]
    fn query_optional_mut_absent_storage_yields_all_none() {
        let mut world = World::new();
        world.spawn().insert(Position(10, 0));
        // Velocity storage was never created.
        let mut q = Query::<(&mut Position, Option<&mut Velocity>)>::from_world(&world);
        let mut count = 0;
        for (pos, vel) in q.iter_mut() {
            assert_eq!(pos.0, 10); // the required driver still yields its value
            assert!(vel.is_none());
            count += 1;
        }
        assert_eq!(count, 1);
    }

    /// Standalone `Query<Option<&T>>` on an empty world drives an empty
    /// snapshot — zero rows, no panic.
    #[test]
    fn query_optional_standalone_on_empty_world_yields_nothing() {
        let world = World::new();
        let q = Query::<Option<&Velocity>>::from_world(&world);
        assert_eq!(q.iter().count(), 0);
    }

    /// An optional composes with a filter: the filter narrows the set, the
    /// optional rides along on whatever survives.
    #[test]
    fn query_optional_with_filter_narrows_by_filter_optional_rides_along() {
        let mut world = World::new();
        // Marker + Velocity → kept, Velocity Some.
        world
            .spawn()
            .insert(Position(1, 0))
            .insert(Velocity(10, 0))
            .insert(Marker);
        // Marker, no Velocity → kept, Velocity None.
        world.spawn().insert(Position(2, 0)).insert(Marker);
        // Velocity but no Marker → excluded by the filter.
        world.spawn().insert(Position(3, 0)).insert(Velocity(30, 0));

        let q = Query::<(&Position, Option<&Velocity>), With<Marker>>::from_world(&world);
        let rows: Vec<(i32, Option<i32>)> = q.iter().map(|(p, v)| (p.0, v.map(|v| v.0))).collect();

        assert_eq!(rows.len(), 2); // only the two Marker-holders
        assert!(rows.contains(&(1, Some(10))));
        assert!(rows.contains(&(2, None)));
        assert!(!rows.iter().any(|(x, _)| *x == 3)); // no Marker → excluded
    }

    /// `Without` offers no candidate, so the data element drives (Data(0)) and
    /// the filter narrows per entity via `matches` — distinct from the `With`
    /// case where the filter can drive. The optional still rides along.
    #[test]
    fn query_optional_without_filter_data_drives_optional_rides() {
        let mut world = World::new();
        // No Marker → kept; Velocity Some.
        world.spawn().insert(Position(1, 0)).insert(Velocity(10, 0));
        // No Marker, no Velocity → kept; Velocity None.
        world.spawn().insert(Position(2, 0));
        // Has Marker → excluded by Without<Marker>.
        world
            .spawn()
            .insert(Position(3, 0))
            .insert(Velocity(30, 0))
            .insert(Marker);

        let q = Query::<(&Position, Option<&Velocity>), Without<Marker>>::from_world(&world);
        let rows: Vec<(i32, Option<i32>)> = q.iter().map(|(p, v)| (p.0, v.map(|v| v.0))).collect();

        assert_eq!(rows.len(), 2); // the two Marker-free entities
        assert!(rows.contains(&(1, Some(10))));
        assert!(rows.contains(&(2, None)));
        assert!(!rows.iter().any(|(x, _)| *x == 3)); // has Marker → excluded
    }

    /// An `Or` filter that wins driver selection feeds entities through the
    /// `DriveSource::External` path. This pins that the optional resolves to
    /// the correct `Some`/`None` *value* under external drive — the existing
    /// driver-cost test only counts steps, not values.
    #[test]
    fn query_optional_or_filter_external_drive_resolves_values() {
        let mut world = World::new();
        // Matches via the Velocity arm only → Marker None.
        world.spawn().insert(Position(1, 0)).insert(Velocity(1, 0));
        // Matches via the Marker arm only → Marker Some.
        world.spawn().insert(Position(2, 0)).insert(Marker);
        // Matches both arms → Marker Some, visited once.
        world
            .spawn()
            .insert(Position(3, 0))
            .insert(Velocity(3, 0))
            .insert(Marker);
        // Matches neither → excluded.
        world.spawn().insert(Position(4, 0));

        let q =
            Query::<(&Position, Option<&Marker>), Or<(With<Velocity>, With<Marker>)>>::from_world(
                &world,
            );
        let rows: Vec<(i32, bool)> = q.iter().map(|(p, m)| (p.0, m.is_some())).collect();

        assert_eq!(rows.len(), 3); // the Or union, deduplicated
        assert!(rows.contains(&(1, false))); // Velocity arm — Marker absent
        assert!(rows.contains(&(2, true))); // Marker arm — Marker present
        assert!(rows.contains(&(3, true))); // both arms — present, once
        assert!(!rows.iter().any(|(x, _)| *x == 4));
    }

    /// Mut driver + read optional (`(&mut A, Option<&B>)`) — a different macro
    /// path from `(&mut A, Option<&mut B>)`. The driver writes through; the
    /// optional read rides along.
    #[test]
    fn query_optional_read_with_mut_driver_writes_through() {
        let mut world = World::new();
        let mover = world
            .spawn()
            .insert(Position(5, 0))
            .insert(Velocity(3, 0))
            .id();
        let still = world.spawn().insert(Position(9, 0)).id(); // no Velocity

        {
            let mut q = Query::<(&mut Position, Option<&Velocity>)>::from_world(&world);
            for (mut pos, vel) in q.iter_mut() {
                if let Some(v) = vel {
                    pos.0 += v.0;
                }
            }
        }
        assert_eq!(world.get::<Position>(mover).unwrap().0, 8); // 5 + 3
        assert_eq!(world.get::<Position>(still).unwrap().0, 9); // untouched
    }

    /// Arity-3 with TWO trailing optionals — both `O` positions exercised
    /// together; the required first element drives all rows.
    #[test]
    fn query_optional_arity_three_two_trailing_optionals() {
        let mut world = World::new();
        world
            .spawn()
            .insert(Position(1, 0))
            .insert(Velocity(10, 0))
            .insert(Marker); // both Some
        world.spawn().insert(Position(2, 0)).insert(Velocity(20, 0)); // Velocity Some, Marker None
        world.spawn().insert(Position(3, 0)); // both None

        let q = Query::<(&Position, Option<&Velocity>, Option<&Marker>)>::from_world(&world);
        let rows: Vec<(i32, Option<i32>, bool)> = q
            .iter()
            .map(|(p, v, m)| (p.0, v.map(|v| v.0), m.is_some()))
            .collect();

        assert_eq!(rows.len(), 3); // all three — Position is the required driver
        assert!(rows.contains(&(1, Some(10), true)));
        assert!(rows.contains(&(2, Some(20), false)));
        assert!(rows.contains(&(3, None, false)));
    }

    /// Writing through `Option<&mut T>` marks the component changed (via the
    /// same `Mut` guard as `&mut T`); a present-but-unwritten row does not.
    #[test]
    fn query_optional_mut_marks_changed_only_when_written() {
        let mut world = World::new();
        let written = world
            .spawn()
            .insert(Position(1, 0))
            .insert(Velocity(5, 0))
            .id();
        let untouched = world
            .spawn()
            .insert(Position(2, 0))
            .insert(Velocity(7, 0))
            .id();
        // Advance Velocity's clock past both inserts (a third Velocity bumps
        // it), so a write later stamps a STRICTLY higher tick. Without this the
        // write would land on the same tick as the last insert and the
        // assertion below would pass only by coincidence of insertion order.
        world.spawn().insert(Velocity(0, 0));
        // Capture the pre-write marks (after the bump).
        let before_written = world
            .storage::<Velocity>()
            .unwrap()
            .changed_tick_for(written);
        let before_untouched = world
            .storage::<Velocity>()
            .unwrap()
            .changed_tick_for(untouched);

        {
            let mut q = Query::<(&Position, Option<&mut Velocity>)>::from_world(&world);
            for (pos, vel) in q.iter_mut() {
                if pos.0 == 1
                    && let Some(mut v) = vel
                {
                    v.0 = 99; // DerefMut → marks Velocity changed for `written`
                }
                // `untouched`'s Velocity is Some but never written through.
            }
        }
        let velo = world.storage::<Velocity>().unwrap();
        assert!(velo.changed_tick_for(written) > before_written); // marked
        assert_eq!(velo.changed_tick_for(untouched), before_untouched); // untouched
    }

    /// Arity-3 with TWO trailing *mutable* optionals — exercises the
    /// `build_elem!(OW, …)` + `fetch_non_driver!(OW, …)` `DenseMut` lookup for
    /// two optional positions at once, with write-through on the present one.
    #[test]
    fn query_optional_arity_three_two_trailing_mut_optionals() {
        let mut world = World::new();
        let both = world
            .spawn()
            .insert(Position(1, 0))
            .insert(Velocity(10, 0))
            .insert(Marker)
            .id();
        let vel_only = world
            .spawn()
            .insert(Position(2, 0))
            .insert(Velocity(20, 0))
            .id();
        let neither = world.spawn().insert(Position(3, 0)).id();

        let mut markers_seen = 0;
        {
            let mut q =
                Query::<(&mut Position, Option<&mut Velocity>, Option<&mut Marker>)>::from_world(
                    &world,
                );
            for (mut pos, vel, marker) in q.iter_mut() {
                pos.0 += 100; // required driver writes through
                if marker.is_some() {
                    markers_seen += 1;
                }
                if let Some(mut v) = vel {
                    v.0 += 1; // OW write-through
                }
            }
        }
        assert_eq!(markers_seen, 1); // only `both` has a Marker
        assert_eq!(world.get::<Position>(both).unwrap().0, 101); // driver wrote all 3
        assert_eq!(world.get::<Position>(vel_only).unwrap().0, 102);
        assert_eq!(world.get::<Position>(neither).unwrap().0, 103);
        assert_eq!(world.get::<Velocity>(both).unwrap().0, 11); // OW wrote the present ones
        assert_eq!(world.get::<Velocity>(vel_only).unwrap().0, 21);
    }

    // -------- Wider multi-mut + mut-not-first (the unified macro) --------

    #[test]
    fn query_arity_three_multi_mut_writes_through_all_sides() {
        // `(&mut A, &mut B, &mut C)` — three storages, all mutable.
        // Driver A's safe `iter_mut`; B and C looked up per entity via
        // their own `DenseMut` views. Each entity touched once by the
        // driver, so each view's `get` is called at most once per
        // entity.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(1, 2))
            .insert(Velocity(10, 20))
            .insert(Marker)
            .id();
        {
            let mut q = Query::<(&mut Position, &mut Velocity, &mut Marker)>::from_world(&world);
            for (mut pos, mut vel, _marker) in q.iter_mut() {
                pos.0 += vel.0;
                pos.1 += vel.1;
                vel.0 = -5;
                vel.1 = -5;
            }
        }
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(11, 22));
        assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(-5, -5));
    }

    #[test]
    fn query_arity_four_multi_mut_writes_through_all_sides() {
        // Arity 4, all mutable. Same logic as arity 3.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(A(1))
            .insert(B(2))
            .insert(C(3))
            .insert(D(4))
            .id();
        {
            let mut q = Query::<(&mut A, &mut B, &mut C, &mut D)>::from_world(&world);
            for (mut a, mut b, mut c, mut d) in q.iter_mut() {
                a.0 += 100;
                b.0 += 100;
                c.0 += 100;
                d.0 += 100;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 101);
        assert_eq!(world.get::<B>(e).unwrap().0, 102);
        assert_eq!(world.get::<C>(e).unwrap().0, 103);
        assert_eq!(world.get::<D>(e).unwrap().0, 104);
    }

    #[test]
    fn query_arity_five_mixed_writes_through_mut_positions() {
        // Arity-5 smoke test: confirms `impl_all_tuple!(A, B, C, D, E)`
        // expands cleanly and that a mixed combination at the new
        // arity behaves like the lower arities (driver A is read,
        // muts at B / D, reads at C / E).
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(A(1))
            .insert(B(2))
            .insert(C(3))
            .insert(D(4))
            .insert(E(5))
            .id();
        {
            let mut q = Query::<(&A, &mut B, &C, &mut D, &E)>::from_world(&world);
            for (a, mut b, c, mut d, e_item) in q.iter_mut() {
                b.0 = a.0 + c.0 + e_item.0; // 1 + 3 + 5 = 9
                d.0 = a.0 + c.0 + e_item.0; // 9
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 1); // unchanged
        assert_eq!(world.get::<B>(e).unwrap().0, 9);
        assert_eq!(world.get::<C>(e).unwrap().0, 3); // unchanged
        assert_eq!(world.get::<D>(e).unwrap().0, 9);
        assert_eq!(world.get::<E>(e).unwrap().0, 5); // unchanged
    }

    #[test]
    fn query_read_driver_with_mut_non_driver_writes_through() {
        // `(&A, &mut B)` — read driver, mut non-driver. Previously
        // deferred ("write as `(&mut B, &A)` instead"); now ships.
        // Driver A's safe `iter`; B looked up per entity via `DenseMut`.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(7, 7))
            .insert(Velocity(1, 2))
            .id();
        {
            let mut q = Query::<(&Position, &mut Velocity)>::from_world(&world);
            for (pos, mut vel) in q.iter_mut() {
                vel.0 += pos.0;
                vel.1 += pos.1;
            }
        }
        assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(8, 9));
        // Position untouched.
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(7, 7));
    }

    #[test]
    fn query_mixed_mut_arity_three_writes_only_through_mut_positions() {
        // `(&A, &mut B, &C)` — only B is mutable. Driver A reads, C
        // reads, B writes.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(2, 3))
            .insert(Velocity(10, 10))
            .insert(Marker)
            .id();
        {
            let mut q = Query::<(&Position, &mut Velocity, &Marker)>::from_world(&world);
            for (pos, mut vel, _marker) in q.iter_mut() {
                vel.0 += pos.0;
                vel.1 += pos.1;
            }
        }
        assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(12, 13));
        // Position, Marker untouched.
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(2, 3));
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_arity_three_multi_mut_self_conflict_panics() {
        // `(&mut A, &mut A, &mut B)` — A written twice. Caught by the
        // self-conflict check at `Query::from_world` time.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0)).insert(Velocity(0, 0));
        let _q = Query::<(&mut Position, &mut Position, &mut Velocity)>::from_world(&world);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_mut_not_first_self_conflict_panics() {
        // `(&A, &mut A)` — reversed-order conflict (read driver, mut
        // non-driver, same component). Now reachable as a query shape
        // since the unified macro covers it; previously the access-
        // level test was the only coverage of the reversed direction.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(&Position, &mut Position)>::from_world(&world);
    }

    // -------- Coverage for previously-untested mixed shape combos --------

    #[test]
    fn query_arity_three_mixed_combinations_write_only_through_mut_positions() {
        // Exercises the four arity-3 combinations not covered by the
        // dedicated tests above: `(&A, &B, &mut C)`,
        // `(&mut A, &mut B, &C)`, `(&mut A, &B, &mut C)`, and
        // `(&A, &mut B, &mut C)`. Each block mutates only the `&mut`
        // positions and verifies non-`&mut` positions stayed put.
        let mut world = World::new();
        let e = world.spawn().insert(A(1)).insert(B(2)).insert(C(3)).id();

        // (&A, &B, &mut C): only C is mutable.
        {
            let mut q = Query::<(&A, &B, &mut C)>::from_world(&world);
            for (a, b, mut c) in q.iter_mut() {
                c.0 = a.0 + b.0;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 1);
        assert_eq!(world.get::<B>(e).unwrap().0, 2);
        assert_eq!(world.get::<C>(e).unwrap().0, 3); // 1 + 2

        // (&mut A, &mut B, &C): A and B mutable, C read.
        {
            let mut q = Query::<(&mut A, &mut B, &C)>::from_world(&world);
            for (mut a, mut b, c) in q.iter_mut() {
                a.0 = c.0 * 10;
                b.0 = c.0 * 20;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 30);
        assert_eq!(world.get::<B>(e).unwrap().0, 60);
        assert_eq!(world.get::<C>(e).unwrap().0, 3); // unchanged

        // (&mut A, &B, &mut C): muts at outer positions.
        {
            let mut q = Query::<(&mut A, &B, &mut C)>::from_world(&world);
            for (mut a, b, mut c) in q.iter_mut() {
                a.0 = b.0 + 100;
                c.0 = b.0 + 200;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 160);
        assert_eq!(world.get::<B>(e).unwrap().0, 60); // unchanged
        assert_eq!(world.get::<C>(e).unwrap().0, 260);

        // (&A, &mut B, &mut C): read driver, two muts.
        {
            let mut q = Query::<(&A, &mut B, &mut C)>::from_world(&world);
            for (a, mut b, mut c) in q.iter_mut() {
                b.0 = a.0 + 1;
                c.0 = a.0 + 2;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 160); // unchanged
        assert_eq!(world.get::<B>(e).unwrap().0, 161);
        assert_eq!(world.get::<C>(e).unwrap().0, 162);
    }

    #[test]
    fn query_arity_four_mixed_combinations_write_only_through_mut_positions() {
        // Four representative mixed combinations out of the 14
        // arity-4 mixes not covered elsewhere. Each block mutates
        // only the `&mut` positions and verifies non-`&mut` positions
        // stayed put.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(A(1))
            .insert(B(2))
            .insert(C(3))
            .insert(D(4))
            .id();

        // (&mut A, &mut B, &C, &D): muts at first two positions.
        {
            let mut q = Query::<(&mut A, &mut B, &C, &D)>::from_world(&world);
            for (mut a, mut b, c, d) in q.iter_mut() {
                a.0 = c.0 + d.0;
                b.0 = c.0 * d.0;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 7);
        assert_eq!(world.get::<B>(e).unwrap().0, 12);
        assert_eq!(world.get::<C>(e).unwrap().0, 3);
        assert_eq!(world.get::<D>(e).unwrap().0, 4);

        // (&A, &mut B, &mut C, &mut D): read driver, three muts.
        {
            let mut q = Query::<(&A, &mut B, &mut C, &mut D)>::from_world(&world);
            for (a, mut b, mut c, mut d) in q.iter_mut() {
                b.0 = a.0 + 100;
                c.0 = a.0 + 200;
                d.0 = a.0 + 300;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 7); // unchanged
        assert_eq!(world.get::<B>(e).unwrap().0, 107);
        assert_eq!(world.get::<C>(e).unwrap().0, 207);
        assert_eq!(world.get::<D>(e).unwrap().0, 307);

        // (&mut A, &B, &mut C, &mut D): muts at positions 0, 2, 3.
        {
            let mut q = Query::<(&mut A, &B, &mut C, &mut D)>::from_world(&world);
            for (mut a, b, mut c, mut d) in q.iter_mut() {
                a.0 = b.0 + 1000;
                c.0 = b.0 + 2000;
                d.0 = b.0 + 3000;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 1107);
        assert_eq!(world.get::<B>(e).unwrap().0, 107); // unchanged
        assert_eq!(world.get::<C>(e).unwrap().0, 2107);
        assert_eq!(world.get::<D>(e).unwrap().0, 3107);

        // (&A, &mut B, &C, &mut D): alternating mut/read.
        {
            let mut q = Query::<(&A, &mut B, &C, &mut D)>::from_world(&world);
            for (a, mut b, c, mut d) in q.iter_mut() {
                b.0 = a.0 - c.0;
                d.0 = a.0 - c.0;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 1107); // unchanged
        assert_eq!(world.get::<B>(e).unwrap().0, -1000);
        assert_eq!(world.get::<C>(e).unwrap().0, 2107); // unchanged
        assert_eq!(world.get::<D>(e).unwrap().0, -1000);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_arity_four_self_conflict_panics() {
        // Arity-4 macro propagates `collect_access` to every position.
        // A regression that stopped emitting one of the access calls
        // would let `(&mut A, &B, &C, &mut A)` slip past the check
        // and double-borrow A's storage cell.
        let mut world = World::new();
        world
            .spawn()
            .insert(A(0))
            .insert(B(0))
            .insert(C(0))
            .insert(D(0));
        let _q = Query::<(&mut A, &B, &C, &mut A)>::from_world(&world);
    }

    // -------- DenseMut direct coverage --------

    #[test]
    #[allow(unsafe_code, reason = "exercises DenseMut::get's generation check")]
    fn dense_mut_get_rejects_stale_handle_via_generation_check() {
        // Through the `World` API the generation check never fires —
        // despawn cascades clean every storage. The check is defense
        // in depth for direct callers; exercise it here by
        // constructing mismatched arrays by hand.
        use crate::entity::EntityAllocator;
        let mut alloc = EntityAllocator::new();
        let live = alloc.allocate();
        let mut dense = vec![Position(7, 7)];
        let mut changed = vec![0_u32];
        let sparse = vec![Some(0_u32)];
        let entity_index = vec![live];
        let view = DenseMut::<Position>::new(&mut dense, &mut changed, &sparse, &entity_index, 1);
        // SAFETY: single call per entity — no aliasing.
        let live_ref = unsafe { view.get(live) };
        assert!(live_ref.is_some());

        // Manufacture a stale handle: same `index`, different
        // `generation`. Simulates the corruption a direct
        // `ComponentStorage` user could produce by bypassing despawn.
        alloc.destroy(live);
        let fresh = alloc.allocate();
        assert_eq!(live.index, fresh.index);
        assert_ne!(live, fresh);
        let view = DenseMut::<Position>::new(&mut dense, &mut changed, &sparse, &entity_index, 1);
        // SAFETY: distinct entity from the call above (different
        // generation); single call.
        let stale_ref = unsafe { view.get(fresh) };
        assert!(
            stale_ref.is_none(),
            "generation check should reject the stale handle"
        );
    }

    #[test]
    fn dense_join_aliasing_stress_writes_each_slot_once() {
        // Many-entity `(&mut A, &mut B)` join: the non-driver `&mut B` is
        // fetched per entity through `DenseMut::get`, the crate's only
        // `unsafe fn`. Writing through *both* handles on every iteration
        // exercises the "each dense slot is handed out at most once"
        // contract at scale — the property the crate-scoped Miri job
        // (`cargo +nightly miri test -p spark-ecs`) machine-checks against
        // raw-pointer aliasing. A double-borrow of one slot, or an
        // off-by-one in the dense-index lookup the `query/` split touches,
        // surfaces here as a Miri error or a wrong post-condition rather
        // than as silent UB.
        let mut world = World::new();
        let mut ids = Vec::new();
        for i in 0..256 {
            ids.push(world.spawn().insert(A(i)).insert(B(i * 10)).id());
        }

        let mut q = Query::<(&mut A, &mut B)>::from_world(&world);
        let mut visited = 0usize;
        for (mut a, mut b) in q.iter_mut() {
            a.0 += 1;
            b.0 += 1;
            visited += 1;
        }
        // Release the query's exclusive storage borrows before reading back.
        drop(q);
        assert_eq!(visited, ids.len(), "every A∩B entity visited exactly once");

        // Each slot written exactly once: A(i) → i + 1, B(i * 10) → i * 10 + 1.
        // A second write to any slot (the aliasing bug this guards) would
        // show up as `+ 2` here.
        for (i, &e) in ids.iter().enumerate() {
            let i = i32::try_from(i).unwrap();
            assert_eq!(world.get::<A>(e).unwrap().0, i + 1);
            assert_eq!(world.get::<B>(e).unwrap().0, i * 10 + 1);
        }
    }

    // -------- Query filters: Query<D, F> --------

    #[test]
    fn query_with_filter_yields_only_entities_having_the_marker() {
        let mut world = World::new();
        world.spawn().insert(Position(1, 1)).insert(Marker);
        world.spawn().insert(Position(2, 2)); // no Marker
        world.spawn().insert(Position(3, 3)).insert(Marker);

        let q = Query::<&Position, With<Marker>>::from_world(&world);
        let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
        assert_eq!(xs.len(), 2);
        assert!(xs.contains(&1));
        assert!(xs.contains(&3));
        assert!(!xs.contains(&2));
    }

    #[test]
    fn query_without_filter_excludes_entities_having_the_marker() {
        let mut world = World::new();
        world.spawn().insert(Position(1, 1)).insert(Marker);
        world.spawn().insert(Position(2, 2)); // no Marker
        let q = Query::<&Position, Without<Marker>>::from_world(&world);
        let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
        assert_eq!(xs, vec![2]);
    }

    #[test]
    fn query_and_filter_requires_all_branches() {
        let mut world = World::new();
        // Velocity present, Marker absent — matches.
        world.spawn().insert(Position(1, 1)).insert(Velocity(0, 0));
        // Velocity present but Marker present — excluded by Without.
        world
            .spawn()
            .insert(Position(2, 2))
            .insert(Velocity(0, 0))
            .insert(Marker);
        // Velocity absent — excluded by With.
        world.spawn().insert(Position(3, 3));

        let q = Query::<&Position, And<(With<Velocity>, Without<Marker>)>>::from_world(&world);
        let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
        assert_eq!(xs, vec![1]);
    }

    #[test]
    fn query_or_filter_matches_any_branch() {
        let mut world = World::new();
        world.spawn().insert(Position(1, 1)).insert(Velocity(0, 0)); // Velocity
        world.spawn().insert(Position(2, 2)).insert(Marker); // Marker
        world.spawn().insert(Position(3, 3)); // neither — excluded

        let q = Query::<&Position, Or<(With<Velocity>, With<Marker>)>>::from_world(&world);
        let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
        assert_eq!(xs.len(), 2);
        assert!(xs.contains(&1));
        assert!(xs.contains(&2));
    }

    #[test]
    fn query_nested_and_of_or_filter_composes() {
        // Filter: With<Velocity> AND (With<Marker> OR With<A>).
        let mut world = World::new();
        // Velocity + Marker — matches.
        world
            .spawn()
            .insert(Position(1, 1))
            .insert(Velocity(0, 0))
            .insert(Marker);
        // Velocity + A — matches the Or via its other branch.
        world
            .spawn()
            .insert(Position(2, 2))
            .insert(Velocity(0, 0))
            .insert(A(0));
        // Velocity only — neither Or branch matches.
        world.spawn().insert(Position(3, 3)).insert(Velocity(0, 0));
        // Marker only, no Velocity — fails the outer And.
        world.spawn().insert(Position(4, 4)).insert(Marker);

        let q = Query::<&Position, And<(With<Velocity>, Or<(With<Marker>, With<A>)>)>>::from_world(
            &world,
        );
        let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
        assert_eq!(xs.len(), 2);
        assert!(xs.contains(&1));
        assert!(xs.contains(&2));
    }

    #[test]
    fn query_filter_applies_to_iter_mut() {
        let mut world = World::new();
        let marked = world.spawn().insert(Position(1, 1)).insert(Marker).id();
        let plain = world.spawn().insert(Position(2, 2)).id();
        {
            let mut q = Query::<&mut Position, With<Marker>>::from_world(&world);
            for mut p in q.iter_mut() {
                p.0 += 100;
            }
        }
        assert_eq!(world.get::<Position>(marked).unwrap().0, 101);
        assert_eq!(world.get::<Position>(plain).unwrap().0, 2); // untouched
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_mut_data_with_same_component_filter_panics() {
        // `With<Position>` reports a read of Position; combined with the
        // `&mut Position` data shape that's a write+read self-conflict,
        // caught at `from_world` before any borrow.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<&mut Position, With<Position>>::from_world(&world);
    }

    #[test]
    fn query_mut_data_without_same_component_does_not_conflict() {
        // `Without<Position>` reports no access, so `&mut Position` data
        // + `Without<Position>` passes the self-conflict check. With no
        // Position entities the driver is empty and `matches` is never
        // called — it iterates cleanly to zero rather than panicking.
        let mut world = World::new();
        world.spawn().insert(Velocity(0, 0));
        let mut q = Query::<&mut Position, Without<Position>>::from_world(&world);
        assert_eq!(q.iter_mut().count(), 0);
    }

    #[test]
    #[should_panic(expected = "already mutably borrowed")]
    fn query_mut_data_without_same_component_panics_at_from_world() {
        // The flip side of the test above. When `Position`'s storage is
        // *non-empty*, `from_world` first takes a `RefMut` on its cell
        // (`D::init_state`), then `Without<Position>::init_state` tries a
        // shared borrow of the same cell — issue #65 moved filter-state
        // fetching into `from_world`, so the `RefCell` "already mutably
        // borrowed" panic now fires at *construction*, before iteration.
        // The query is nonsensical (it could never yield anything), but the
        // failure mode is exactly what `Without`'s no-access decision implies.
        //
        // REGRESSION GUARD — there is deliberately **no** `iter`/`iter_mut`
        // call below: the panic must come from `from_world` itself. If a
        // future refactor moves filter-state fetching back to a per-iter
        // local, construction stops borrowing the cell, this body no longer
        // panics, and `#[should_panic]` fails loudly — pinning the panic
        // *point*, not just the message. Do not add an `iter` call here.
        let mut world = World::new();
        world.spawn().insert(Position(1, 1));
        let _q = Query::<&mut Position, Without<Position>>::from_world(&world);
    }

    #[test]
    fn filtered_query_wires_up_as_system_param() {
        // The `F` generic threads through `IntoSystem` like any other
        // part of the query type — the runner builds it via `fetch`.
        let mut world = World::new();
        world.spawn().insert(Position(1, 1)).insert(Marker);
        world.spawn().insert(Position(2, 2));
        fn bump_marked(mut q: Query<&mut Position, With<Marker>>) {
            for mut p in q.iter_mut() {
                p.0 += 10;
            }
        }
        let mut sys = IntoSystem::into_system(bump_marked);
        sys(&world);
        let xs: Vec<i32> = Query::<&Position>::from_world(&world)
            .iter()
            .map(|p| p.0)
            .collect();
        assert!(xs.contains(&11)); // marked entity moved
        assert!(xs.contains(&2)); // unmarked entity untouched
    }

    // -------- change detection: precise `Mut` marking --------

    #[test]
    fn join_does_not_overmark_driver_for_unjoined_entities() {
        // The headline fix: in `Query<(&mut Position, &mut Velocity)>` the
        // driver (Position) visits *every* Position entity, but the join
        // drops those lacking Velocity. With `Mut`, a dropped entity's
        // Position is never `DerefMut`'d, so it is NOT marked changed.
        let mut world = World::new();
        let both = world
            .spawn()
            .insert(Position(1, 1))
            .insert(Velocity(1, 1))
            .id(); // Position.changed = 2 (clock 1→2)
        let pos_only = world.spawn().insert(Position(5, 5)).id(); // changed = 3
        let _bump = world.spawn().insert(Position(0, 0)).id(); // clock → 4
        {
            let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
            for (mut p, mut v) in q.iter_mut() {
                p.0 += 1; // marks Position for the joined entity only
                v.0 += 1;
            }
        }
        let pos = world.storage::<Position>().unwrap();
        // `both` was written → marked at Position's clock (4).
        assert_eq!(pos.changed_tick_for(both), Some(4));
        // `pos_only` was visited by the driver but the join dropped it and
        // the body never wrote it → its mark stays at its insert tick (3).
        assert_eq!(pos.changed_tick_for(pos_only), Some(3));
    }

    #[test]
    fn read_only_iteration_marks_nothing() {
        let mut world = World::new();
        let e = world.spawn().insert(Position(7, 7)).id(); // changed = 2
        {
            let q = Query::<&Position>::from_world(&world);
            assert_eq!(q.iter().count(), 1);
        }
        // The read path never takes a `Mut`, so nothing is marked.
        assert_eq!(
            world.storage::<Position>().unwrap().changed_tick_for(e),
            Some(2)
        );
    }

    #[test]
    fn multi_mut_tuple_marks_only_the_written_component() {
        // `Query<(&mut Position, &mut Velocity)>`, body writes Position
        // (DerefMut) but only reads Velocity (Deref). Run through
        // `run_system` so BOTH clocks advance to 3 first — proving that
        // advancing the clock is *not* what marks: only the `DerefMut`
        // on the non-driver `Velocity` (via `DenseMut::get` → `Mut`) would,
        // and it never happens. Velocity stays at its insert tick.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(1, 1)) // Position clock → 2
            .insert(Velocity(2, 2)) // Velocity clock → 2
            .id();
        let mut access = Access::new();
        access.components_mut().add_write::<Position>();
        access.components_mut().add_write::<Velocity>();
        let mut last_seen = Vec::new();
        world.run_system(&access, &mut last_seen, &mut |w| {
            let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(w);
            for (mut pos, vel) in q.iter_mut() {
                pos.0 += vel.0; // DerefMut on pos; Deref-only on vel
            }
        });
        // Both clocks advanced 2 → 3; only the written component is stamped.
        assert_eq!(
            world.storage::<Position>().unwrap().changed_tick_for(e),
            Some(3) // written
        );
        assert_eq!(
            world.storage::<Velocity>().unwrap().changed_tick_for(e),
            Some(2) // read-only → stays at its insert tick
        );
    }

    // -------- entity-as-data: Query<Entity> / Query<(Entity, …)> --------

    #[test]
    fn query_entity_yields_every_live_entity_including_componentless() {
        let mut world = World::new();
        let a = world.spawn().insert(Position(0, 0)).id();
        let b = world.spawn().id(); // no components at all
        let c = world.spawn().insert(Velocity(1, 1)).id();

        let ids: Vec<Entity> = Query::<Entity>::from_world(&world).iter().collect();
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&a));
        assert!(ids.contains(&b)); // yielded despite holding no components
        assert!(ids.contains(&c));
    }

    #[test]
    fn query_entity_excludes_already_despawned_entity() {
        let (mut world, [a, b, c]) = world_with_three_movers();
        world.despawn(b);
        let ids: Vec<Entity> = Query::<Entity>::from_world(&world).iter().collect();
        assert_eq!(ids, vec![a, c]); // slot-index order; b's slot is free
    }

    #[test]
    fn query_entity_with_filter_keeps_only_matching() {
        let mut world = World::new();
        let m1 = world.spawn().insert(Position(1, 1)).insert(Marker).id();
        let _plain = world.spawn().insert(Position(2, 2)).id(); // no Marker
        let m2 = world.spawn().insert(Marker).id(); // Marker, no Position

        // Entity drives off the live snapshot, then `With<Marker>` filters
        // per entity — correct even though the marker isn't in the item.
        let ids: Vec<Entity> = Query::<Entity, With<Marker>>::from_world(&world)
            .iter()
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&m1));
        assert!(ids.contains(&m2));
    }

    #[test]
    fn query_entity_position_tuple_yields_id_and_component() {
        let (world, [a, b, c]) = world_with_three_movers();
        let mut pairs: Vec<(Entity, i32)> = Query::<(Entity, &Position)>::from_world(&world)
            .iter()
            .map(|(e, p)| (e, p.0))
            .collect();
        pairs.sort_by_key(|(_, x)| *x);
        assert_eq!(pairs, vec![(a, 0), (b, 10), (c, 20)]);
    }

    #[test]
    fn query_entity_mut_tuple_writes_through_and_reports_id() {
        let (world, [a, _, _]) = world_with_three_movers();
        {
            let mut q = Query::<(Entity, &mut Position)>::from_world(&world);
            for (e, mut p) in q.iter_mut() {
                if e == a {
                    p.0 = 999;
                }
            }
        }
        assert_eq!(world.get::<Position>(a).unwrap().0, 999);
    }

    #[test]
    fn query_entity_arity_two_tuple_yields_id_and_both_components() {
        let (world, _) = world_with_three_movers();
        let rows: Vec<(Entity, i32, i32)> =
            Query::<(Entity, &Position, &Velocity)>::from_world(&world)
                .iter()
                .map(|(e, p, v)| (e, p.0, v.0))
                .collect();
        assert_eq!(rows.len(), 3);
        // The id in each row owns the components it's paired with.
        for (e, px, _) in rows {
            assert_eq!(world.get::<Position>(e).unwrap().0, px);
        }
    }

    #[test]
    fn query_entity_tuple_skips_entity_missing_a_component() {
        let mut world = World::new();
        let full = world
            .spawn()
            .insert(Position(1, 1))
            .insert(Velocity(2, 2))
            .id();
        let _pos_only = world.spawn().insert(Position(3, 3)).id();
        // Position drives; the Velocity-less entity is dropped by the join.
        let ids: Vec<Entity> = Query::<(Entity, &Position, &Velocity)>::from_world(&world)
            .iter()
            .map(|(e, _, _)| e)
            .collect();
        assert_eq!(ids, vec![full]);
    }

    #[test]
    fn query_entity_mixed_mut_tuple_writes_only_through_mut() {
        let (world, _) = world_with_three_movers();
        {
            // Position drives (mut), Velocity is a read non-driver.
            let mut q = Query::<(Entity, &mut Position, &Velocity)>::from_world(&world);
            for (_e, mut p, v) in q.iter_mut() {
                p.0 += v.0;
            }
        }
        let xs: Vec<i32> = Query::<&Position>::from_world(&world)
            .iter()
            .map(|p| p.0)
            .collect();
        // a: 0+1, b: 10+0, c: 20+1
        assert_eq!(xs, vec![1, 10, 21]);
    }

    #[test]
    fn query_entity_tuple_as_system_param_via_into_system() {
        let (world, [a, _, _]) = world_with_three_movers();
        fn step(mut q: Query<(Entity, &mut Position)>) {
            for (_e, mut p) in q.iter_mut() {
                p.0 += 1;
            }
        }
        let mut sys = IntoSystem::into_system(step);
        sys(&world);
        assert_eq!(world.get::<Position>(a).unwrap().0, 1);
    }

    #[test]
    fn query_entity_tuple_for_in_ref_sugar_yields_id_and_component() {
        let (world, _) = world_with_three_movers();
        let q = Query::<(Entity, &Position)>::from_world(&world);
        let mut count = 0;
        for (e, p) in &q {
            // The yielded id owns the yielded component.
            assert_eq!(world.get::<Position>(e).unwrap().0, p.0);
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_entity_tuple_self_conflict_panics_naming_component() {
        // Entity is invisible to the conflict check; the write+read of
        // Position still collides and the panic names Position.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(Entity, &mut Position, &Position)>::from_world(&world);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_entity_mut_with_same_component_filter_panics() {
        // `With<Position>` reports a read; the `&mut Position` element
        // writes it. Entity contributes nothing, so the conflict stands.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(Entity, &mut Position), With<Position>>::from_world(&world);
    }

    #[test]
    fn query_entity_coexists_with_commands_and_snapshot_excludes_new_spawn() {
        // The snapshot (taken when the query is fetched, before the body
        // runs) is what lets `Query<Entity>` and `Commands` share a
        // signature: the Vec releases the allocator borrow, so the later
        // `cmd.spawn()`'s `borrow_mut` doesn't panic. The spawned id
        // postdates the snapshot, so it is not yielded this frame.
        //
        // `IntoSystem` requires a `'static` body, so the observed state is
        // shared into the closure via owned `Rc` clones rather than borrows.
        let mut world = World::new();
        let pre = world.spawn().id();

        let seen = Rc::new(RefCell::new(Vec::<Entity>::new()));
        let spawned = Rc::new(RefCell::new(None));
        {
            let seen = Rc::clone(&seen);
            let spawned = Rc::clone(&spawned);
            let observe = move |q: Query<Entity>, mut cmd: Commands| {
                let fresh = cmd.spawn().id(); // allocated synchronously…
                *spawned.borrow_mut() = Some(fresh);
                for id in q.iter() {
                    // …but the query walks the construction-time snapshot.
                    seen.borrow_mut().push(id);
                }
            };
            let mut sys = IntoSystem::into_system(observe);
            sys(&world); // no "already borrowed" panic
        }
        world.flush_commands();

        let seen = seen.borrow();
        let spawned = spawned.borrow().unwrap();
        assert_eq!(*seen, vec![pre]);
        assert!(!seen.contains(&spawned));
        assert!(world.is_alive(spawned)); // it really was created
    }

    #[test]
    fn query_entity_snapshot_still_yields_commands_despawned_entity() {
        // `Commands::despawn` is deferred to the next flush, so the doomed
        // entity stays alive — and in the snapshot — for the rest of the
        // frame, and is still yielded.
        let mut world = World::new();
        let doomed = world.spawn().id();
        let other = world.spawn().id();

        let seen = Rc::new(RefCell::new(Vec::<Entity>::new()));
        {
            let seen = Rc::clone(&seen);
            let observe = move |q: Query<Entity>, mut cmd: Commands| {
                cmd.despawn(doomed);
                for id in q.iter() {
                    seen.borrow_mut().push(id);
                }
            };
            let mut sys = IntoSystem::into_system(observe);
            sys(&world);
        }
        {
            let captured = seen.borrow();
            assert!(captured.contains(&doomed)); // deferred → still yielded
            assert!(captured.contains(&other));
        }
        assert!(world.is_alive(doomed)); // not gone until flush

        world.flush_commands();
        assert!(!world.is_alive(doomed));
    }

    #[test]
    fn query_entity_on_empty_world_yields_nothing() {
        // The snapshot path must cope with zero live entities — an empty
        // `Vec`, not a panic.
        let world = World::new();
        assert_eq!(Query::<Entity>::from_world(&world).iter().count(), 0);
    }

    #[test]
    fn query_entity_fully_mut_tuple_writes_through_both_components() {
        // `(Entity, &mut A, &mut B)` — the all-mut entity tuple. The
        // non-driver `&mut B` goes through `DenseMut` under an entity
        // prefix, a path no other entity test exercises.
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(1, 2))
            .insert(Velocity(10, 20))
            .id();
        {
            let mut q = Query::<(Entity, &mut Position, &mut Velocity)>::from_world(&world);
            for (id, mut p, mut v) in q.iter_mut() {
                assert_eq!(id, e);
                p.0 += v.0; // 1 + 10 = 11
                v.0 = 0;
            }
        }
        assert_eq!(world.get::<Position>(e).unwrap().0, 11);
        assert_eq!(world.get::<Velocity>(e).unwrap().0, 0);
    }

    #[test]
    fn query_entity_arity_three_component_read_tuple_joins_and_skips() {
        // `(Entity, &A, &B, &C)` — the widest entity tuple, and the only
        // shape that exercises `impl_all_tuple_entity!(A, B, C)`'s
        // `ReadOnlyQueryData` arm. Also drives the `for … in &q` sugar on a
        // 3-component entity tuple (its `iter_ref` body).
        let mut world = World::new();
        let full = world.spawn().insert(A(1)).insert(B(2)).insert(C(3)).id();
        world.spawn().insert(A(9)).insert(B(9)); // no C — join must skip it

        let rows: Vec<(Entity, i32, i32, i32)> = Query::<(Entity, &A, &B, &C)>::from_world(&world)
            .iter()
            .map(|(id, a, b, c)| (id, a.0, b.0, c.0))
            .collect();
        assert_eq!(rows, vec![(full, 1, 2, 3)]);

        // `&q` sugar over the same shape — id owns the components it's with.
        let q = Query::<(Entity, &A, &B, &C)>::from_world(&world);
        let mut count = 0;
        for (id, a, _b, _c) in &q {
            assert_eq!(world.get::<A>(id).unwrap().0, a.0);
            count += 1;
        }
        assert_eq!(count, 1);
    }

    #[test]
    fn query_entity_tuple_for_in_mut_sugar_writes_through() {
        // `for … in &mut q` over an entity tuple — the `IntoIterator for
        // &mut Query` path (`iter`), distinct from the `&q` (`iter_ref`)
        // path the read tests cover.
        let (world, _) = world_with_three_movers();
        {
            let mut q = Query::<(Entity, &mut Position)>::from_world(&world);
            for (id, mut p) in &mut q {
                assert!(world.is_alive(id)); // the threaded id is a live handle
                p.0 += 1;
            }
        }
        let xs: Vec<i32> = Query::<&Position>::from_world(&world)
            .iter()
            .map(|p| p.0)
            .collect();
        assert_eq!(xs, vec![1, 11, 21]);
    }

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn query_entity_double_mut_same_type_panics() {
        // Write+write of the same component, behind an Entity prefix: the
        // entity-prefixed `collect_access` must still report both writes so
        // the self-conflict check fires (proves no `access_call!` was
        // dropped in the entity macro expansion).
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let _q = Query::<(Entity, &mut Position, &mut Position)>::from_world(&world);
    }

    #[test]
    fn query_entity_tuple_without_filter_excludes_marked_entities() {
        // Filter threading through an entity tuple: the outer (threaded) id
        // the filter tests must be the driver's entity, so `Without<Marker>`
        // drops the marked one and the surviving id/value pair is correct.
        let mut world = World::new();
        let plain = world.spawn().insert(Position(1, 1)).id();
        let _marked = world.spawn().insert(Position(2, 2)).insert(Marker).id();

        let rows: Vec<(Entity, i32)> =
            Query::<(Entity, &Position), Without<Marker>>::from_world(&world)
                .iter()
                .map(|(e, p)| (e, p.0))
                .collect();
        assert_eq!(rows, vec![(plain, 1)]);
    }

    #[test]
    fn query_entity_read_tuple_iter_and_iter_mut_agree() {
        // `.iter()` on an all-read entity tuple takes the `@readonly`
        // `iter_ref` path; `.iter_mut()` takes the `@gen` `iter` path. They
        // are distinct macro expansions and must yield identical pairs — the
        // only test that exercises `@gen` for an *all-read* entity tuple.
        let (world, [a, b, c]) = world_with_three_movers();

        let via_iter: Vec<(Entity, i32)> = Query::<(Entity, &Position)>::from_world(&world)
            .iter()
            .map(|(e, p)| (e, p.0))
            .collect();
        let mut q = Query::<(Entity, &Position)>::from_world(&world);
        let via_iter_mut: Vec<(Entity, i32)> = q.iter_mut().map(|(e, p)| (e, p.0)).collect();

        assert_eq!(via_iter, via_iter_mut);
        assert_eq!(via_iter, vec![(a, 0), (b, 10), (c, 20)]);
    }

    #[test]
    fn query_entity_tuple_excludes_immediately_despawned_entity() {
        // `world.despawn` (immediate, unlike `Commands::despawn`) removes
        // components before freeing the slot, so a tuple query built after it
        // drives off a storage that no longer holds the despawned entity.
        // Pins that despawn-ordering invariant for the entity-tuple path.
        let (mut world, [a, b, c]) = world_with_three_movers();
        world.despawn(b);
        let pairs: Vec<(Entity, i32)> = Query::<(Entity, &Position)>::from_world(&world)
            .iter()
            .map(|(e, p)| (e, p.0))
            .collect();
        assert_eq!(pairs, vec![(a, 0), (c, 20)]); // b gone (swap-removed)
    }

    #[test]
    fn query_entity_tuple_for_unknown_component_yields_empty() {
        // `Marker` storage was never created — the entity-prefixed driver
        // must reach the `None` arm of `drive_iter!` and yield empty, not
        // panic (a distinct macro path from the join-skip case).
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        let q = Query::<(Entity, &Marker)>::from_world(&world);
        assert_eq!(q.iter().count(), 0);
        let mut q_mut = Query::<(Entity, &mut Marker)>::from_world(&world);
        assert_eq!(q_mut.iter_mut().count(), 0);
    }

    #[test]
    fn query_entity_after_respawn_yields_new_generation_not_stale() {
        // The snapshot must carry each slot's *current* generation: after a
        // despawn+respawn into the same slot, `Query<Entity>` yields the new
        // handle and not the stale one.
        let mut world = World::new();
        let a = world.spawn().id(); // slot 0, gen 0
        world.despawn(a);
        let b = world.spawn().id(); // slot 0 reused, gen 1
        assert_eq!(a.index, b.index);
        assert_ne!(a, b);

        let ids: Vec<Entity> = Query::<Entity>::from_world(&world).iter().collect();
        assert_eq!(ids, vec![b]);
        assert!(!ids.contains(&a)); // stale handle excluded
    }
}

#[cfg(test)]
#[allow(
    clippy::items_after_statements,
    reason = "test components sit beside the assertions that use them."
)]
mod driver_cost_tests {
    //! Deterministic cost-contract checks for issue #65: every shape drives a
    //! number of *driver steps* proportional to its smallest candidate, not to
    //! the live set or to which element was written first. Counting is exact
    //! (a `#[cfg(test)]` per-advance counter), so these assertions are
    //! noise-free — wall-clock benchmarks are deferred to #63.

    use super::*;
    use crate::filter::{Added, And, Changed, Or, With, Without};

    #[derive(Component)]
    struct Big;
    #[derive(Component)]
    struct Small;
    #[derive(Component)]
    struct One;
    /// Never inserted — its storage is absent (population 0).
    #[derive(Component)]
    struct Phantom;
    /// A valued component, for tests that assert *written values*, not just
    /// driver-step counts.
    #[derive(Component)]
    struct Val(i32);
    #[derive(Component)]
    struct Tag;

    // 10_000 entities hold `Big`; the first 50 of those also hold `Small`; a
    // single separate entity holds only `One`. So the populations are
    // distinct — Big 10_000, Small 50, One 1 — and the live set is 10_001.
    const BIG: usize = 10_000;
    const SMALL: usize = 50;
    const LIVE: usize = BIG + 1;

    fn world() -> World {
        let mut w = World::new();
        for i in 0..BIG {
            let e = w.spawn().insert(Big).id();
            if i < SMALL {
                w.insert(e, Small);
            }
        }
        w.spawn().insert(One);
        w
    }

    /// Runs `body` (which must fully consume a query iterator) and returns how
    /// many driver advances it cost.
    fn steps(body: impl FnOnce()) -> usize {
        let _ = take_driver_steps(); // clear any residue
        body();
        take_driver_steps()
    }

    // ---- the contract matrix: driver steps ∝ smallest candidate ----

    #[test]
    fn entity_with_filter_drives_filter_population() {
        let w = world();
        let n = steps(|| {
            let q = Query::<Entity, With<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL); // result correctness
        });
        assert_eq!(n, SMALL); // not LIVE
    }

    #[test]
    fn ref_with_filter_drives_smaller_of_data_and_filter() {
        let w = world();
        // Filter (50) beats data (10_000): the filter leads.
        let n = steps(|| {
            let q = Query::<&Big, With<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn tuple_drives_smallest_component_even_when_not_first() {
        let w = world();
        // `Small` is the *second* element yet drives — the headline win.
        let n = steps(|| {
            let q = Query::<(&Big, &Small)>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL); // not BIG
    }

    #[test]
    fn tuple_first_element_smallest_keeps_direct_path() {
        let w = world();
        // `Small` first: today's direct path, same step count.
        let n = steps(|| {
            let q = Query::<(&Small, &Big)>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn entity_prefixed_tuple_drives_smallest_component() {
        let w = world();
        let n = steps(|| {
            let q = Query::<(Entity, &Big, &Small)>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn optional_element_offers_no_candidate_and_never_drives() {
        let w = world();
        // `Small` (50) is the smaller storage, but as `Option<&Small>` it
        // narrows nothing — it must NOT drive, or the 9_950 `Big`-only
        // entities would be wrongly dropped. `Big` drives all 10_000 rows.
        let n = steps(|| {
            let q = Query::<(&Big, Option<&Small>)>::from_world(&w);
            assert_eq!(q.iter().count(), BIG); // every Big entity, not just 50
        });
        assert_eq!(n, BIG); // drives Big, never the optional Small
    }

    #[test]
    fn required_drives_with_optional_riding_along() {
        let w = world();
        // `Small` required drives (50); the huge `Big` rides as `Option`.
        let n = steps(|| {
            let q = Query::<(&Small, Option<&Big>)>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL); // not BIG, not LIVE
    }

    #[test]
    fn optional_at_arity_three_never_drives_smallest_required_does() {
        let w = world();
        // `Small` (50) is the smallest *required* element and drives even from
        // the second position; `Option<&One>` offers no candidate. Cost is 50,
        // not BIG, not LIVE — and this exercises the `drive_ref(Data(k>0))`
        // slice path with an optional sitting in the slices array.
        let n = steps(|| {
            let q = Query::<(&Big, &Small, Option<&One>)>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn non_first_driver_resolves_optional_values_correctly() {
        // Value-correctness companion to the step-count test above: when a
        // non-first required element drives (`Small` at index 1), the optional
        // at index 2 must still resolve per the driven entities.
        let mut w = world();
        // One entity carries Big + Small + One, so its optional yields `Some`.
        w.spawn().insert(Big).insert(Small).insert(One);
        let q = Query::<(&Big, &Small, Option<&One>)>::from_world(&w);
        let (mut some, mut none) = (0usize, 0usize);
        for (_b, _s, one) in q.iter() {
            if one.is_some() {
                some += 1;
            } else {
                none += 1;
            }
        }
        assert_eq!(some, 1); // exactly the entity we just added
        assert_eq!(none, SMALL); // the original 50 Small-holders lack One
    }

    #[test]
    fn filter_drives_optional_bearing_data_shape() {
        let w = world();
        // `With<Small>` (50) beats the `&Big` data candidate (10_000), so the
        // FILTER drives even though the data shape carries an optional. The
        // `Option<&One>` rides along (always `None` here) and never gates.
        let n = steps(|| {
            let q = Query::<(&Big, Option<&One>), With<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL); // filter-driven, not the 10_000 Big data set
    }

    #[test]
    fn mut_tuple_drives_smallest_component() {
        let w = world();
        // `&mut Big` first, `&Small` second and smaller — the smaller leads,
        // `Big` is reached through the `DenseMut` lookup.
        let n = steps(|| {
            let mut q = Query::<(&mut Big, &Small)>::from_world(&w);
            assert_eq!(q.iter_mut().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn and_drives_smallest_arm() {
        let w = world();
        // min(|Small| 50, |One| 1) = 1. No entity has both, so 0 results —
        // but the driver still costs only the smallest arm.
        let n = steps(|| {
            let q = Query::<Entity, And<(With<Small>, With<One>)>>::from_world(&w);
            assert_eq!(q.iter().count(), 0);
        });
        assert_eq!(n, 1);
    }

    #[test]
    fn and_with_data_drives_smallest_candidate_across_all() {
        let w = world();
        // min over &Big (10_000), Small (50), One (1) = 1.
        let n = steps(|| {
            let q = Query::<&Big, And<(With<Small>, With<One>)>>::from_world(&w);
            let _ = q.iter().count();
        });
        assert_eq!(n, 1);
    }

    #[test]
    fn shallow_or_drives_deduplicated_union() {
        let w = world();
        // Small (50) and One (1) are disjoint → union is 51.
        let n = steps(|| {
            let q = Query::<Entity, Or<(With<Small>, With<One>)>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL + 1);
        });
        assert_eq!(n, SMALL + 1);
    }

    #[test]
    fn changed_drives_component_population_never_live_set() {
        let w = world();
        // No system has run → baseline 0 → all 50 `Small` count as changed.
        // Driver is Small (50), never the live set, even paired with &Big.
        let n = steps(|| {
            let q = Query::<&Big, Changed<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn added_drives_component_population() {
        let w = world();
        let n = steps(|| {
            let q = Query::<&Big, Added<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    // ---- exempt shapes: no candidate ⇒ live-set drive (by necessity) ----

    #[test]
    fn standalone_entity_drives_live_set() {
        let w = world();
        let n = steps(|| {
            let q = Query::<Entity>::from_world(&w);
            assert_eq!(q.iter().count(), LIVE);
        });
        assert_eq!(n, LIVE);
    }

    #[test]
    fn without_only_falls_back_to_live_set() {
        let w = world();
        // `Without` enumerates no candidate; with nothing positive elsewhere
        // it drives the live set and rejects per entity (exempt by design).
        let n = steps(|| {
            let q = Query::<Entity, Without<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), LIVE - SMALL);
        });
        assert_eq!(n, LIVE);
    }

    // ---- natural form ≤ best hand-rewrite (same driver steps) ----

    #[test]
    fn natural_form_matches_best_hand_rewrite() {
        let w = world();
        // `Query<Entity, With<Small>>` is the natural form; `Query<(Entity,
        // &Small)>` is the hand-rewrite a user reaches for today. Equal cost.
        let natural = steps(|| {
            let _ = Query::<Entity, With<Small>>::from_world(&w).iter().count();
        });
        let rewrite = steps(|| {
            let _ = Query::<(Entity, &Small)>::from_world(&w).iter().count();
        });
        assert_eq!(natural, rewrite);
        assert_eq!(natural, SMALL);
    }

    // ---- correctness: reordering the driver doesn't change the result set ----

    #[test]
    fn driver_choice_preserves_result_set() {
        let w = world();
        let forward: usize = Query::<(&Big, &Small)>::from_world(&w).iter().count();
        let reversed: usize = Query::<(&Small, &Big)>::from_world(&w).iter().count();
        assert_eq!(forward, reversed);
        assert_eq!(forward, SMALL);
    }

    // ---- value correctness through a filter-/reorder-chosen driver ----

    // Six `Val` entities; indices 0 and 3 also carry `Tag`. So `Tag` (2)
    // drives over `Val` (6) — exercising the `&mut` filter-driven path
    // (`DenseMut` reached via an *external* filter driver, not a non-first
    // data element) and asserting the *written values*, not just the count.
    fn world_six_vals() -> (World, Vec<Entity>) {
        let mut w = World::new();
        let mut ids = Vec::new();
        for i in 0..6 {
            let e = w.spawn().insert(Val(i)).id();
            if i % 3 == 0 {
                w.insert(e, Tag);
            }
            ids.push(e);
        }
        (w, ids)
    }

    #[test]
    fn filter_driven_mut_writes_only_matching_values() {
        let (w, ids) = world_six_vals();
        let n = steps(|| {
            let mut q = Query::<&mut Val, With<Tag>>::from_world(&w);
            for mut v in q.iter_mut() {
                v.0 += 100;
            }
        });
        assert_eq!(n, 2); // filter (Tag) drives, not all six Vals
        let vals: Vec<i32> = ids.iter().map(|&e| w.get::<Val>(e).unwrap().0).collect();
        assert_eq!(vals, vec![100, 1, 2, 103, 4, 5]); // only the Tag-holders bumped
    }

    #[test]
    fn entity_prefixed_mut_drives_smallest_and_writes_through() {
        let (w, ids) = world_six_vals();
        let n = steps(|| {
            let mut q = Query::<(Entity, &mut Val, &Tag)>::from_world(&w);
            let mut seen = Vec::new();
            for (e, mut v, _tag) in q.iter_mut() {
                v.0 += 100;
                seen.push(e);
            }
            assert_eq!(seen.len(), 2);
        });
        assert_eq!(n, 2); // `Tag` (global index 2) drives the exclusive path
        let vals: Vec<i32> = ids.iter().map(|&e| w.get::<Val>(e).unwrap().0).collect();
        assert_eq!(vals, vec![100, 1, 2, 103, 4, 5]);
    }

    // ---- edge cases in the candidate machinery ----

    #[test]
    fn or_with_absent_arm_drives_only_the_present_arm() {
        let w = world();
        // `Phantom`'s storage is absent ⇒ its `candidate_slice` is `Some(&[])`,
        // so the union is `Small (50) ∪ ∅` = 50 — never the live set.
        let n = steps(|| {
            let q = Query::<Entity, Or<(With<Small>, With<Phantom>)>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL);
    }

    #[test]
    fn and_with_negative_arm_drives_the_positive_arm() {
        let w = world();
        // `Without<Big>` offers no candidate (it `flatten`s out of
        // `And::candidate_slice`), so the positive `With<Small>` arm (50)
        // drives. Every Small-holder also has Big, so `matches` rejects all
        // 50 — 0 results, but the driver cost is the smallest positive arm.
        let n = steps(|| {
            let q = Query::<Entity, And<(With<Small>, Without<Big>)>>::from_world(&w);
            assert_eq!(q.iter().count(), 0);
        });
        assert_eq!(n, SMALL); // drove Small's 50, not the live set
    }

    #[test]
    fn equal_population_tie_breaks_to_earliest_element() {
        // `L` and `R` cover the same 3 entities but in opposite dense order
        // (`R` inserted in reverse). Equal populations ⇒ the tie breaks to the
        // *first* element, so the yielded order is the first element's order.
        #[derive(Component)]
        struct L(i32);
        #[derive(Component)]
        struct R;

        let mut w = World::new();
        let e0 = w.spawn().insert(L(0)).id();
        let e1 = w.spawn().insert(L(1)).id();
        let e2 = w.spawn().insert(L(2)).id();
        w.insert(e2, R);
        w.insert(e1, R);
        w.insert(e0, R); // R's dense order is [e2, e1, e0]

        // `(&L, &R)`: tie → L (first) drives → L's dense order [0, 1, 2].
        let forward: Vec<i32> = Query::<(&L, &R)>::from_world(&w)
            .iter()
            .map(|(l, _)| l.0)
            .collect();
        assert_eq!(forward, vec![0, 1, 2]);

        // `(&R, &L)`: tie → R (first) drives → R's dense order [2, 1, 0].
        let reversed: Vec<i32> = Query::<(&R, &L)>::from_world(&w)
            .iter()
            .map(|(_, l)| l.0)
            .collect();
        assert_eq!(reversed, vec![2, 1, 0]);
    }

    // ---- the frozen plan replays correctly across multiple iterations ----

    #[test]
    fn or_filter_plan_replays_identically_on_second_iter() {
        let w = world();
        // The `Or` union is materialized once at construction; a second
        // `iter()` must borrow the same cached `Vec` and yield the same set
        // at the same cost — not rebuild it (cycle-1 perf fix) nor drift.
        let q = Query::<Entity, Or<(With<Small>, With<One>)>>::from_world(&w);
        let first: Vec<Entity> = q.iter().collect();
        let _ = take_driver_steps();
        let second: Vec<Entity> = q.iter().collect();
        let n2 = take_driver_steps();
        assert_eq!(first, second);
        assert_eq!(n2, SMALL + 1);
    }

    #[test]
    fn non_first_data_driver_replays_on_second_iter_mut() {
        let w = world();
        // `Small` (2nd, smaller) drives; `Big` is reached via `DenseMut` from
        // the still-live `RefMut`. A second `iter_mut()` re-borrows through
        // the same guard and costs the same.
        let mut q = Query::<(&mut Big, &Small)>::from_world(&w);
        let first = q.iter_mut().count();
        let _ = take_driver_steps();
        let second = q.iter_mut().count();
        let n2 = take_driver_steps();
        assert_eq!(first, SMALL);
        assert_eq!(second, SMALL);
        assert_eq!(n2, SMALL);
    }

    #[test]
    fn or_three_arms_drives_union_of_all_three() {
        // Exercises the arity-3 `candidate_len` sum and `candidate_materialize`
        // union (three disjoint arms: Small 50, One 1, Trio 3).
        #[derive(Component)]
        struct Trio;
        let mut w = world();
        for _ in 0..3 {
            w.spawn().insert(Trio);
        }
        let n = steps(|| {
            let q = Query::<Entity, Or<(With<Small>, With<One>, With<Trio>)>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL + 1 + 3);
        });
        assert_eq!(n, SMALL + 1 + 3);
    }

    #[test]
    fn multi_mut_tuple_drives_smaller_and_writes_both() {
        // `(&mut Val, &mut Val2)` where `Val2` is smaller: it drives off its
        // own slice and *both* elements are written through `DenseMut`.
        #[derive(Component)]
        struct Val2(i32);
        let mut w = World::new();
        let mut ids = Vec::new();
        for i in 0..6 {
            let e = w.spawn().insert(Val(i)).id();
            if i % 3 == 0 {
                w.insert(e, Val2(i * 10)); // idx 0, 3 carry Val2
            }
            ids.push(e);
        }
        let n = steps(|| {
            let mut q = Query::<(&mut Val, &mut Val2)>::from_world(&w);
            for (mut v, mut v2) in q.iter_mut() {
                v.0 += 1;
                v2.0 += 100;
            }
        });
        assert_eq!(n, 2); // Val2 (2) drives, not Val (6)
        // The two joined entities had both written; the rest are untouched.
        assert_eq!(w.get::<Val>(ids[0]).unwrap().0, 1);
        assert_eq!(w.get::<Val2>(ids[0]).unwrap().0, 100);
        assert_eq!(w.get::<Val>(ids[3]).unwrap().0, 4);
        assert_eq!(w.get::<Val2>(ids[3]).unwrap().0, 130);
        assert_eq!(w.get::<Val>(ids[1]).unwrap().0, 1); // Val(1), no Val2 → unchanged
        assert_eq!(w.get::<Val>(ids[2]).unwrap().0, 2); // unchanged
    }

    #[test]
    fn or_mut_with_overlapping_arms_visits_each_entity_once() {
        // Soundness pin: an entity in BOTH `Or` arms must be visited once,
        // never twice — `Or::candidate_materialize`'s dedup is what keeps
        // `DenseMut::get` from handing out two aliasing `&mut OD` to the same
        // dense slot. If the dedup regressed, this would over-count steps and
        // double-increment (or be UB under Miri).
        #[derive(Component)]
        struct OA;
        #[derive(Component)]
        struct OB;
        #[derive(Component)]
        struct OD(i32);

        let mut w = World::new();
        let e0 = w.spawn().insert(OA).insert(OB).insert(OD(1)).id(); // in BOTH arms
        let e1 = w.spawn().insert(OA).insert(OD(2)).id();
        let e2 = w.spawn().insert(OB).insert(OD(3)).id();
        // Extra OD-only entities so OD's population (8) exceeds the `Or`
        // candidate sum (|OA| + |OB| = 4) — forcing the `Or` union to be the
        // driver, which is the path under test. Each is paired with its
        // expected (untouched) value so the assertion needs no `usize as i32`.
        let others: Vec<(Entity, i32)> = (0..5)
            .map(|v| (w.spawn().insert(OD(10 + v)).id(), 10 + v))
            .collect();

        let n = steps(|| {
            let mut q = Query::<&mut OD, Or<(With<OA>, With<OB>)>>::from_world(&w);
            for mut d in q.iter_mut() {
                d.0 += 100;
            }
        });
        // Union {e0,e1} ∪ {e0,e2} dedups to {e0,e1,e2} = 3 driver steps — e0
        // appears once, so its `&mut OD` is handed out exactly once.
        assert_eq!(n, 3);
        assert_eq!(w.get::<OD>(e0).unwrap().0, 101); // visited exactly once
        assert_eq!(w.get::<OD>(e1).unwrap().0, 102);
        assert_eq!(w.get::<OD>(e2).unwrap().0, 103);
        for (e, expected) in others {
            assert_eq!(w.get::<OD>(e).unwrap().0, expected); // excluded, untouched
        }
    }

    #[test]
    fn changed_driver_steps_equal_candidate_while_matches_trims() {
        // `Changed<Small>` drives `Small`'s full population (the candidate),
        // and the per-entity tick check trims the result *below* that — the
        // step count tracks the candidate, the result tracks the matches.
        use crate::Access;
        use crate::storage::AnyStorage;
        use std::any::TypeId;

        let mut w = World::new();
        let mut small_ids = Vec::new();
        for _ in 0..5 {
            small_ids.push(w.spawn().insert(Big).insert(Small).id());
        }
        for _ in 0..10 {
            w.spawn().insert(Big); // Big 15, Small 5 → Changed<Small> drives
        }

        // Baseline = Small's clock now: every current holder looks "seen".
        let baseline = w.storage::<Small>().unwrap().current_tick();
        let small_tid = TypeId::of::<Small>();

        // Re-insert Small on 2 → their `changed_tick` advances past baseline.
        w.insert(small_ids[0], Small);
        w.insert(small_ids[1], Small);

        let mut results = 0;
        let mut driver_steps = 0;
        let access = Access::new();
        let mut last_seen = vec![(small_tid, baseline)];
        w.run_system(&access, &mut last_seen, &mut |world| {
            let _ = take_driver_steps();
            let q = Query::<&Big, Changed<Small>>::from_world(world);
            results = q.iter().count();
            driver_steps = take_driver_steps();
        });
        assert_eq!(results, 2); // only the 2 re-inserted Smalls
        assert_eq!(driver_steps, 5); // but the driver walked all 5 Small, never Big's 15
    }

    // ---- `Without` as a per-entity reject within a driven query ----

    #[test]
    fn without_rejects_within_data_driven_query() {
        let w = world();
        // `Without<Small>` offers no candidate, so the positive `&Big`
        // (10_000) drives and `Without` rejects the 50 Small-holders per
        // entity. The exclusion shrinks the *result*, not the driver.
        let n = steps(|| {
            let q = Query::<&Big, Without<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), BIG - SMALL);
        });
        assert_eq!(n, BIG); // driver is the data element; `Without` can't shrink it
    }

    #[test]
    fn entity_prefixed_without_rejects_per_entity() {
        let w = world();
        // `Query<(Entity, &Big), Without<Small>>`: `Entity` and `Without`
        // offer no candidate, so `&Big` drives and the id rides along while
        // `Without<Small>` rejects per entity.
        let n = steps(|| {
            let q = Query::<(Entity, &Big), Without<Small>>::from_world(&w);
            assert_eq!(q.iter().count(), BIG - SMALL);
        });
        assert_eq!(n, BIG);
    }

    #[test]
    fn and_changed_with_without_drives_the_changed_arm() {
        let w = world();
        // `And<(Changed<Small>, Without<One>)>`: `Changed<Small>` surfaces
        // Small's 50 entities; `Without<One>` surfaces nothing — so the `And`
        // drives the change-filter arm (50), never the 10_000-element `&Big`.
        // No system ran, so baseline 0 makes every Small "changed"; none of
        // the Small-holders carry `One`, so all 50 survive both arms.
        let n = steps(|| {
            let q = Query::<&Big, And<(Changed<Small>, Without<One>)>>::from_world(&w);
            assert_eq!(q.iter().count(), SMALL);
        });
        assert_eq!(n, SMALL); // the Changed arm drives, not the data element
    }
}
