//! Declarative code generation for the tuple [`QueryData`] /
//! [`ReadOnlyQueryData`] impls (arity 2+), kept in one file because
//! `macro_rules!` is textually scoped — the helper macros, the three leaf
//! generators, the Cartesian drivers, and their invocations must all sit
//! together. Phase 2 (issue #80) fronts this file with a *variant
//! manifest* table.
//!
//! The generated impls reuse the driver runtime
//! ([`DriveSource`](super::DriveSource) / [`DriverIter`](super::DriverIter)),
//! [`DenseMut`](super::dense_mut::DenseMut), and the `counted!` test
//! harness — all from the parent [`query`](super) module, reachable here by
//! textual macro scope and ancestor access.

use std::cell::{Ref, RefMut};

use crate::Component;
use crate::access::QueryAccess;
use crate::entity::Entity;
use crate::storage::{ComponentStorage, Mut};
use crate::world::World;

use super::dense_mut::DenseMut;
use super::{DriveSource, DriverIter, QueryData, ReadOnlyQueryData};

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
