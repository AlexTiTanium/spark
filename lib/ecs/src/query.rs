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
//! for it explicitly via the forthcoming `Query<(Entity, &T)>` shape
//! (entity-as-data follow-up).
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
//! regular [`QueryData`] element with an empty access set, and every
//! piece of machinery (`collect_access`, self-conflict, driver
//! selection) handles it uniformly without a special-case for the
//! always-pair. The internal `(Entity, Item)` thread is an
//! implementation detail of the join path.
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
//! Tuple queries drive the first element and look the rest up per
//! entity. **Every `&` / `&mut` combination** at arities 2–5 is
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
//! just for fewer entities. Iteration wraps the data driver in a
//! `.filter(…)` that calls [`QueryFilter::matches`] per candidate, and
//! [`Query::from_world`] folds [`QueryFilter::collect_access`] into the
//! same self-conflict check the data shape runs. See the [`filter`]
//! module for the filter set and the access-reporting rules.
//!
//! [`filter`]: crate::filter
//! [`QueryFilter::matches`]: crate::QueryFilter::matches
//! [`QueryFilter::collect_access`]: crate::QueryFilter::collect_access

use std::cell::{Ref, RefMut};
use std::marker::PhantomData;

use crate::Component;
use crate::access::{Access, QueryAccess};
use crate::entity::Entity;
use crate::filter::QueryFilter;
use crate::storage::ComponentStorage;
use crate::system::SystemParam;
use crate::world::World;

/// Type-level description of what a [`Query`] fetches.
///
/// Implementors define [`Item`](Self::Item) — the value yielded per
/// entity — and [`State`](Self::State) — the cached storage borrows the
/// [`Query`] holds across iteration. [`init_state`](Self::init_state)
/// builds the state from a `&World`; [`iter`](Self::iter) walks it. See
/// the module-level docs for the lifetime design.
///
/// # Examples
///
/// `QueryData` is rarely named in user code — the impls for `&T`,
/// `&mut T`, and tuples cover every query shape. Naming it is useful
/// when writing generic helpers; here we just confirm the impl exists:
///
/// ```
/// use spark_ecs::{Component, QueryData};
///
/// fn _accepts<D: QueryData>() {}
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// _accepts::<&Position>();
/// _accepts::<&mut Position>();
/// ```
pub trait QueryData {
    /// What this query yields per entity. `&T`, `&mut T`, or a tuple.
    type Item<'w>
    where
        Self: 'w;

    /// Cached storage borrows held by the [`Query`] for the duration
    /// of iteration. `Option<Ref<…>>` for `&T`, a tuple of states for
    /// joins.
    type State<'w>
    where
        Self: 'w;

    /// Builds the state from a `&World`. Called once per fetch.
    fn init_state<'w>(world: &'w World) -> Self::State<'w>
    where
        Self: 'w;

    /// Exclusive iteration over `(Entity, Item)` pairs. `&'s mut State`
    /// is what makes `&mut T` items expressible.
    fn iter<'s, 'w>(
        state: &'s mut Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's;

    /// Records which component types this data shape reads and writes
    /// into `access`. `&T` pushes a read, `&mut T` pushes a write,
    /// tuples push each element in shape order.
    ///
    /// Called by [`Query::from_world`] *before* [`init_state`](Self::init_state)
    /// so the per-query self-conflict check sees the access set before
    /// any storage is borrowed. The scheduler (roadmap item 3) reuses
    /// the same call to aggregate access at `SystemParam` level.
    fn collect_access(access: &mut QueryAccess);
}

/// [`QueryData`] that borrows nothing mutably — the marker that gates
/// shared iteration via [`Query::iter`].
///
/// `&T` implements this; `&mut T` deliberately does not. Read-only
/// tuples implement it when every element is itself read-only.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, ReadOnlyQueryData};
///
/// fn _accepts<D: ReadOnlyQueryData>() {}
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
/// _accepts::<&Position>();
/// _accepts::<(&Position, &Velocity)>();
/// // _accepts::<&mut Position>();    // would not compile — exclusive
/// ```
pub trait ReadOnlyQueryData: QueryData {
    /// Shared iteration over `(Entity, Item)` pairs from `&State`.
    fn iter_ref<'s, 'w>(
        state: &'s Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's;

    /// Per-entity lookup used by the read side of a tuple join.
    fn lookup<'s, 'w>(state: &'s Self::State<'w>, entity: Entity) -> Option<Self::Item<'s>>
    where
        Self: 's,
        Self: 'w,
        'w: 's;
}

// -------- &T --------

impl<T: Component> QueryData for &T {
    type Item<'w>
        = &'w T
    where
        Self: 'w;
    type State<'w>
        = Option<Ref<'w, ComponentStorage<T>>>
    where
        Self: 'w;

    fn init_state<'w>(world: &'w World) -> Self::State<'w>
    where
        Self: 'w,
    {
        world.storage::<T>()
    }

    fn iter<'s, 'w>(
        state: &'s mut Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match state {
            Some(storage) => Box::new(storage.iter()),
            None => Box::new(std::iter::empty()),
        }
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }
}

impl<T: Component> ReadOnlyQueryData for &T {
    fn iter_ref<'s, 'w>(
        state: &'s Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match state {
            Some(storage) => Box::new(storage.iter()),
            None => Box::new(std::iter::empty()),
        }
    }

    fn lookup<'s, 'w>(state: &'s Self::State<'w>, entity: Entity) -> Option<&'s T>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        state.as_ref().and_then(|s| s.get(entity))
    }
}

// -------- &mut T --------

impl<T: Component> QueryData for &mut T {
    type Item<'w>
        = &'w mut T
    where
        Self: 'w;
    type State<'w>
        = Option<RefMut<'w, ComponentStorage<T>>>
    where
        Self: 'w;

    fn init_state<'w>(world: &'w World) -> Self::State<'w>
    where
        Self: 'w,
    {
        world.storage_mut::<T>()
    }

    fn iter<'s, 'w>(
        state: &'s mut Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, &'s mut T)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match state {
            Some(storage) => Box::new(storage.iter_mut()),
            None => Box::new(std::iter::empty()),
        }
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_write::<T>();
    }
}

// No ReadOnlyQueryData for &mut T — that's the entire point of the split.

// -------- DenseMut: the single `unsafe fn` powering (D1, &mut T) --------

/// Mutable random-access view into one storage's `dense`, used by the
/// `(D1, &mut T)` arity-2 impl to hand out `&mut T` by entity.
///
/// The driver walks D1 with its normal `iter`; this view is the
/// random-access lookup for the second element, which needs a raw
/// `*mut T` + index (the price of leaving the borrow checker). The full
/// soundness argument lives on [`DenseMut::get`]'s `# Safety` block and
/// the module-level *Joins* docs.
///
/// `PhantomData<&'s mut [T]>` ties the view's lifetime to the
/// storage's exclusive borrow (so it cannot dangle) and gives `T` the
/// invariance that `&mut` requires.
pub(crate) struct DenseMut<'s, T> {
    ptr: *mut T,
    len: usize,
    sparse: &'s [Option<u32>],
    entity_index: &'s [Entity],
    _marker: PhantomData<&'s mut [T]>,
}

// The whole point of `DenseMut` is to host one carefully-contracted
// `unsafe fn` (and one inner `unsafe` block) — scope the workspace's
// `unsafe_code = "warn"` lint allowance to this impl rather than
// relaxing it crate-wide.
#[allow(unsafe_code)]
impl<'s, T> DenseMut<'s, T> {
    /// Builds a view from the storage's exclusively-borrowed `dense`
    /// slice plus shared borrows of its `sparse` table and
    /// `entity_index`. All three at lifetime `'s`; converting `dense`
    /// to a raw pointer consumes the unique borrow, and
    /// `PhantomData<&'s mut [T]>` keeps the lifetime tracked.
    fn new(dense: &'s mut [T], sparse: &'s [Option<u32>], entity_index: &'s [Entity]) -> Self {
        Self {
            ptr: dense.as_mut_ptr(),
            len: dense.len(),
            sparse,
            entity_index,
            _marker: PhantomData,
        }
    }

    /// Returns a mutable reference to `entity`'s component, or `None`
    /// if this storage has no entry for `entity` (or holds a stale
    /// handle to a recycled slot — same generation check
    /// [`ComponentStorage::get_mut`] uses).
    ///
    /// # Safety
    ///
    /// Across the whole lifetime `'s`, `get` must never be called twice
    /// with the same `entity`. Two `&mut T` to one dense slot would
    /// alias. See the module-level *Joins* docs for how the
    /// `(D1, &mut T)` arity-2 impl upholds this — structural driver
    /// shape plus [`QueryAccess::assert_no_self_conflict`] at query
    /// construction.
    ///
    /// Also assumes `ComponentStorage`'s sparse/dense parallel-array
    /// invariant: every `Some(idx)` in `sparse` points at an in-bounds
    /// `dense` slot. The `debug_assert!` catches violations in tests;
    /// release builds trust the invariant — or rather, the
    /// bounds-checked `entity_index[dense_idx]` access below would
    /// panic first, so reaching the `ptr.add` proves the index is in
    /// bounds.
    unsafe fn get(&self, entity: Entity) -> Option<&'s mut T> {
        let dense_idx = (*self.sparse.get(entity.index as usize)?)? as usize;
        debug_assert!(
            dense_idx < self.len,
            "sparse/dense desync — ComponentStorage swap_remove invariant violated"
        );
        // Generation check + implicit bounds check. A stale `Entity`
        // whose `index` collides with a different live tenant returns
        // `None`, matching `ComponentStorage::get_mut`. The Vec
        // indexing is bounds-checked in release too, so reaching the
        // `ptr.add` below proves `dense_idx < entity_index.len() ==
        // dense.len() == self.len`.
        if self.entity_index[dense_idx] != entity {
            return None;
        }
        // SAFETY: `dense_idx < self.len` (established by the
        // bounds-checked `entity_index[dense_idx]` above plus the
        // sparse/dense parallel-array invariant), and by this fn's
        // contract this entity is fetched at most once across `'s`, so
        // no other live `&mut` overlaps this slot.
        Some(unsafe { &mut *self.ptr.add(dense_idx) })
    }
}

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
// real need before extending.

// ---- Helper macros (file-private) -----------------------------------
//
// Each helper takes a flag token (`R` for read = `&T`, `W` for write =
// `&mut T`) followed by a type ident, and emits the right Rust syntax
// for that position. They keep `impl_one_combo!` body free of
// per-flag branching.

/// `&'w T` for `R`, `&'w mut T` for `W`.
macro_rules! item_type {
    (R $T:ident, $w:lifetime) => { &$w $T };
    (W $T:ident, $w:lifetime) => { &$w mut $T };
}

/// `Option<Ref<'w, ComponentStorage<T>>>` for `R`, `Option<RefMut<…>>`
/// for `W`. Matches the per-element borrow guard the `Query` holds for
/// the iteration's lifetime.
macro_rules! state_type {
    (R $T:ident, $w:lifetime) => { Option<Ref<$w, ComponentStorage<$T>>> };
    (W $T:ident, $w:lifetime) => { Option<RefMut<$w, ComponentStorage<$T>>> };
}

/// `world.storage::<T>()` for `R`, `world.storage_mut::<T>()` for `W`.
macro_rules! init_storage {
    (R $T:ident, $world:expr) => {
        $world.storage::<$T>()
    };
    (W $T:ident, $world:expr) => {
        $world.storage_mut::<$T>()
    };
}

/// `access.add_read::<T>()` for `R`, `access.add_write::<T>()` for `W`.
macro_rules! access_call {
    (R $T:ident, $access:expr) => {
        $access.add_read::<$T>();
    };
    (W $T:ident, $access:expr) => {
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
macro_rules! build_non_driver_fetch {
    (R, $state:ident) => {
        &*$state
    };
    (W, $state:ident) => {
        $state.as_mut().map(|refmut| {
            let (dense, sparse, entity_index) = refmut.split_for_join();
            DenseMut::new(dense, sparse, entity_index)
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
macro_rules! fetch_non_driver {
    (R, $fetch:expr, $entity:expr) => {
        $fetch.as_ref()?.get($entity)?
    };
    (W, $fetch:expr, $entity:expr) => {
        // SAFETY: driver visits each entity once, conflict check
        // guarantees disjoint storages. Module-level docs for details.
        unsafe { $fetch.as_ref()?.get($entity)? }
    };
}

/// Driver iterator over the first storage. `R` uses safe `.iter()`,
/// `W` uses safe `.iter_mut()` (both inherited from `ComponentStorage`).
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
            QueryData for (item_type!($first_flag $First, '_), $(item_type!($rest_flag $Rest, '_),)+)
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
                Box::new(driver.filter_map(move |(entity, item_first)| {
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
                Box::new(driver.filter_map(move |(entity, item_first)| {
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
/// explicitly via the (forthcoming) `Query<(Entity, &T)>` shape. See
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
///     for (pos, vel) in q.iter_mut() {
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
pub struct Query<'w, D: QueryData + 'w, F: QueryFilter = ()> {
    /// Retained for the per-entity filter check during iteration — see
    /// [`QueryFilter::matches`]. Shared, so it coexists with the
    /// `Ref`/`RefMut` storage guards in `state`.
    world: &'w World,
    state: D::State<'w>,
    _filter: PhantomData<F>,
}

impl<'w, D: QueryData + 'w, F: QueryFilter> Query<'w, D, F> {
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
        Self {
            world,
            state: D::init_state(world),
            _filter: PhantomData,
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
    ///     for v in q.iter_mut() {
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
    ///     for (pos, vel) in q.iter_mut() {
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
        // Copy the shared world handle out before borrowing `state`
        // mutably — disjoint fields, so this doesn't fight the `&mut`.
        let world = self.world;
        // Path B — strip the entity that the trait threads internally
        // for join logic; the filter consumes it first. See module docs.
        D::iter(&mut self.state)
            .filter(move |(entity, _)| F::matches(*entity, world))
            .map(|(_entity, item)| item)
    }
}

impl<'w, D: ReadOnlyQueryData + 'w, F: QueryFilter> Query<'w, D, F> {
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
    pub fn iter(&self) -> impl Iterator<Item = D::Item<'_>> + '_ {
        // Path B — see `Query::iter_mut` for the rationale.
        let world = self.world;
        D::iter_ref(&self.state)
            .filter(move |(entity, _)| F::matches(*entity, world))
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
///     for (pos, vel) in &mut q {
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
    use super::*;
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
            for p in q.iter_mut() {
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
            for p in &mut q {
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
    fn query_two_tuple_mut_drives_first_storage_and_writes_through() {
        // The canonical movement example. Drives Position (mut),
        // sparse-looks-up Velocity (shared).
        let (world, entities) = world_with_three_movers();
        {
            let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
            for (pos, vel) in q.iter_mut() {
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
            for (pos, vel) in q.iter_mut() {
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
    // `$D1, $D, ...` macro variables in `impl_query_data_tuple!`.
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
            for (pos, vel, _marker) in q.iter_mut() {
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
            for (pos, vel) in q.iter_mut() {
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
            for (pos, vel) in q.iter_mut() {
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
            for (pos, vel) in q.iter_mut() {
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
            for (pos, vel, _marker) in q.iter_mut() {
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
            for (a, b, c, d) in q.iter_mut() {
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
            for (a, b, c, d, e_item) in q.iter_mut() {
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
            for (pos, vel) in q.iter_mut() {
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
            for (pos, vel, _marker) in q.iter_mut() {
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
            for (a, b, c) in q.iter_mut() {
                c.0 = a.0 + b.0;
            }
        }
        assert_eq!(world.get::<A>(e).unwrap().0, 1);
        assert_eq!(world.get::<B>(e).unwrap().0, 2);
        assert_eq!(world.get::<C>(e).unwrap().0, 3); // 1 + 2

        // (&mut A, &mut B, &C): A and B mutable, C read.
        {
            let mut q = Query::<(&mut A, &mut B, &C)>::from_world(&world);
            for (a, b, c) in q.iter_mut() {
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
            for (a, b, c) in q.iter_mut() {
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
            for (a, b, c) in q.iter_mut() {
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
            for (a, b, c, d) in q.iter_mut() {
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
            for (a, b, c, d) in q.iter_mut() {
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
            for (a, b, c, d) in q.iter_mut() {
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
            for (a, b, c, d) in q.iter_mut() {
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
        let sparse = vec![Some(0_u32)];
        let entity_index = vec![live];
        let view = DenseMut::<Position>::new(&mut dense, &sparse, &entity_index);
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
        let view = DenseMut::<Position>::new(&mut dense, &sparse, &entity_index);
        // SAFETY: distinct entity from the call above (different
        // generation); single call.
        let stale_ref = unsafe { view.get(fresh) };
        assert!(
            stale_ref.is_none(),
            "generation check should reject the stale handle"
        );
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
            for p in q.iter_mut() {
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
    fn query_mut_data_without_same_component_panics_mid_iteration() {
        // The flip side of the test above. When `Position`'s storage is
        // *non-empty*, the `&mut Position` data shape holds a live
        // `RefMut` on its cell while `Without<Position>::matches`
        // re-borrows that same cell (shared) per entity — the documented
        // `RefCell` "already mutably borrowed" panic. The query is nonsensical
        // (it could never yield anything), but the failure mode is
        // exactly what `Without`'s no-access decision implies.
        let mut world = World::new();
        world.spawn().insert(Position(1, 1));
        let mut q = Query::<&mut Position, Without<Position>>::from_world(&world);
        let _ = q.iter_mut().count();
    }

    #[test]
    fn filtered_query_wires_up_as_system_param() {
        // The `F` generic threads through `IntoSystem` like any other
        // part of the query type — the runner builds it via `fetch`.
        let mut world = World::new();
        world.spawn().insert(Position(1, 1)).insert(Marker);
        world.spawn().insert(Position(2, 2));
        fn bump_marked(mut q: Query<&mut Position, With<Marker>>) {
            for p in q.iter_mut() {
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
}
