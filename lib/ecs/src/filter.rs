//! Query filters — narrow *which* entities a [`Query`] yields without
//! changing *what* it yields.
//!
//! A [`QueryData`] shape answers "which components do I read/write, and
//! what do I get per entity". A [`QueryFilter`] answers a separate
//! question: "of the entities that shape could match, which ones do I
//! actually want". It contributes a presence/absence predicate and
//! nothing to the yielded item — `for … in &q` still binds exactly
//! `D::Item`, never the filter.
//!
//! That separation is why filters live in the **second** generic of
//! [`Query<'w, D, F>`](crate::Query): `Query<&Position, With<Powered>>`
//! reads `Position` and yields `&Position`, but only for entities that
//! also carry a `Powered` marker. `F` defaults to `()`, the always-true
//! filter, so every existing `Query<D>` keeps its meaning.
//!
//! # The matching mechanism
//!
//! [`Query::from_world`](crate::Query::from_world) calls
//! [`QueryFilter::init_state`] **once at query construction** — taking the
//! storage borrow(s) and tick baseline(s) the filter needs — and stores the
//! result in the [`Query`]. Each [`Query::iter`](crate::Query::iter) /
//! `iter_mut` then wraps the driver in a `.filter(…)` that calls
//! [`QueryFilter::matches`] per candidate entity against that pre-fetched
//! `&Self::State`. So `World::storage::<T>()` is borrowed once at
//! construction — not per `iter` call and not per entity — and the borrow
//! lives for the query's lifetime (issue #65 moved it here from a per-`iter`
//! fetch so a filter that *drives* can borrow its candidate slice at query
//! lifetime; see [`QueryFilter::candidate_slice`]). The filter rides on top
//! of the safe iteration path — no new `unsafe`, no change to the join
//! machinery. That single borrow is the chokepoint the M4
//! `RefCell → UnsafeCell` swap will target.
//!
//! # Access reporting (and the `With` / `Without` asymmetry)
//!
//! [`QueryFilter::collect_access`] feeds the same [`QueryAccess`] set the
//! data shape fills, *before* the per-query self-conflict check. The two
//! presence filters report differently, by deliberate decision (issue
//! #31):
//!
//! - [`With<T>`] reports a **read** of `T`. In a sparse set the presence
//!   check touches `T`'s storage, so a `&mut T` data shape combined with
//!   `With<T>` is a genuine self-conflict and panics cleanly at
//!   [`Query::from_world`].
//! - [`Without<T>`] reports **nothing**. It is a pure archetype-style
//!   exclusion. The cost: a nonsensical `Query<&mut T, Without<T>>`
//!   (mutate `T` on entities that lack `T` — always empty) is *not* caught
//!   by the self-conflict check; if `T`'s storage is non-empty it surfaces
//!   at construction instead — `Without<T>::init_state` (now run from
//!   `Query::from_world`, see *The matching mechanism*) takes a shared
//!   borrow of `T` while the `&mut T` data shape already holds it
//!   exclusively, so the `RefCell`'s "already mutably borrowed" fires at
//!   `from_world`, before any iteration. That query is meaningless anyway.
//!
//! [`And`] / [`Or`] report the **union** of their children's access —
//! conservative on purpose, since the scheduler needs the worst case
//! even though any single entity may have matched via only one branch.
//!
//! # Change detection ([`Changed<T>`] / [`Added<T>`])
//!
//! These read the per-component change-detection clock. **Each component
//! type owns its own clock** ([`ComponentStorage::current_tick`]): it
//! advances when `T` is inserted, and once before any system that
//! declares a write of `T`. Every entity's slot records the tick it was
//! last written (`changed_tick`) and first attached (`added_tick`). A
//! system carries one *baseline* per component it touches — the tick that
//! component's clock read when it last ran — which the scheduler parks on
//! the world via [`World::baseline_for`](crate::World::baseline_for).
//! [`init_state`](QueryFilter::init_state) reads it once into the filter's
//! [`State`](QueryFilter::State); per-entity [`matches`](QueryFilter::matches)
//! then compares against that, never touching the world directly.
//!
//! [`Changed<T>`] passes when `changed_tick` is newer than the baseline;
//! [`Added<T>`] when `added_tick` is (a one-shot, since `added_tick` never
//! moves after the attach). "Newer than" is a **wrapping-aware relative-age**
//! comparison, not a plain `tick > baseline`: the clock is `wrapping_add`, so
//! comparing distances from the storage's current tick (`is_changed_since`)
//! stays correct across a wrap that a strict `>` would miss. Both report a
//! **read** of `T`, like [`With<T>`], so `Query<&mut T, Changed<T>>` is a
//! self-conflict (use `&T`, or detect a *different* component).
//!
//! Marking is **precise**: a `Query<&mut T>` yields [`Mut<T>`](crate::Mut),
//! which stamps `changed_tick` only on an actual write (`DerefMut`). An
//! entity merely visited by a tuple's driver, dropped by the join, or
//! excluded by a filter is never stamped. And because each component's
//! clock starts at 1 while a fresh system's baseline is 0, a component
//! attached before any system ran is seen on that system's first run.
//!
//! [`ComponentStorage::current_tick`]: crate::ComponentStorage
//! [`Mut<T>`]: crate::Mut
//! [`Query`]: crate::Query
//! [`QueryData`]: crate::QueryData
//! [`QueryData::init_state`]: crate::QueryData::init_state
//! [`Query::from_world`]: crate::Query::from_world

use std::cell::Ref;
use std::marker::PhantomData;

use crate::Component;
use crate::access::QueryAccess;
use crate::entity::Entity;
use crate::storage::ComponentStorage;
use crate::world::World;

/// A predicate over entities that gates a [`Query`] without widening its
/// item.
///
/// Implementors answer two questions: [`matches`](Self::matches) — does
/// this entity pass — and [`collect_access`](Self::collect_access) —
/// which components does the predicate inspect. The crate ships impls for
/// the always-true `()`, the presence pair [`With<T>`] / [`Without<T>`],
/// and the combinators [`And<(…)>`] / [`Or<(…)>`] over tuples of filters
/// (which nest freely, since each combinator is itself a `QueryFilter`).
///
/// # Examples
///
/// `QueryFilter` is rarely named directly — it appears as the second
/// generic of [`Query`]. Naming it is useful in generic helpers:
///
/// ```
/// use spark_ecs::{Component, QueryFilter, With};
///
/// fn _accepts<F: QueryFilter>() {}
/// #[derive(Component)]
/// struct Powered;
/// _accepts::<()>();
/// _accepts::<With<Powered>>();
/// ```
///
/// [`Query`]: crate::Query
/// [`And<(…)>`]: And
/// [`Or<(…)>`]: Or
pub trait QueryFilter {
    /// Per-query state fetched **once** before iteration — the storage
    /// borrow(s) and tick baseline(s) the filter needs — so the per-entity
    /// [`matches`](Self::matches) is a cheap field read instead of a fresh
    /// `HashMap` lookup + `RefCell` borrow each time. Mirrors
    /// [`QueryData::State`](crate::QueryData::State).
    type State<'w>
    where
        Self: 'w;

    /// Fetches the filter's [`State`](Self::State) from the world, once, at
    /// query construction ([`Query::from_world`](crate::Query::from_world)) —
    /// not per `iter` call. The borrow guards live for the query's lifetime.
    fn init_state(world: &World) -> Self::State<'_>;

    /// Returns `true` if `entity` passes this filter, reading only the
    /// pre-fetched `state`. Called once per candidate entity; performs no
    /// mutation and touches the world only through `state`.
    fn matches(entity: Entity, state: &Self::State<'_>) -> bool;

    /// Reports the components this filter inspects into `access`.
    ///
    /// Runs at [`Query::from_world`](crate::Query::from_world) time
    /// alongside the data shape's own access, so the self-conflict check
    /// and (later) the scheduler see one combined set. See the
    /// module-level docs for why [`With`] reports a read but [`Without`]
    /// reports nothing.
    fn collect_access(access: &mut QueryAccess);

    /// The dense entity list this filter can *drive* iteration off of, or
    /// `None` if it offers no single-storage candidate.
    ///
    /// This is the heart of issue #65's driver selection: a filter that
    /// names a concrete component already holds that component's storage,
    /// so it can *lead* the loop over just that component's entities
    /// instead of only rejecting members of a larger driver. [`With<T>`],
    /// [`Changed<T>`] and [`Added<T>`] return `T`'s entities (an **empty**
    /// slice when `T`'s storage is absent — the candidate is empty, not
    /// missing, so the query yields nothing rather than scanning the live
    /// set). [`And`] returns its smallest arm's slice. [`Without`], `()`
    /// and [`Or`] have no single-storage candidate and return `None`
    /// (`Or` surfaces a composite one via
    /// [`candidate_materialize`](Self::candidate_materialize)).
    ///
    /// The default returns `None`.
    fn candidate_slice<'s>(_state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        None
    }

    /// Population of this filter's candidate set, or `None` if it offers
    /// none. Read once at [`Query::from_world`](crate::Query::from_world)
    /// to pick the smallest driver across the whole query; O(1) (a `len()`
    /// on a packed array).
    ///
    /// The default derives from [`candidate_slice`](Self::candidate_slice).
    /// [`Or`] overrides it: its candidate is the *union* of its arms, sized
    /// as the sum of arm populations (an upper bound — exact enough to
    /// choose a driver, and `None` unless **every** arm offers a
    /// [`candidate_slice`](Self::candidate_slice) so the union can be built —
    /// `Without` / `()` / nested `Or` arms do not, an `And` arm does).
    ///
    /// **Contract:** if you override this to return `Some`, then
    /// [`candidate_slice`](Self::candidate_slice) **or**
    /// [`candidate_materialize`](Self::candidate_materialize) must also return
    /// `Some` — the query relies on a `Some` length meaning "I can produce a
    /// driver". The default upholds this trivially.
    fn candidate_len(state: &Self::State<'_>) -> Option<usize> {
        Self::candidate_slice(state).map(<[Entity]>::len)
    }

    /// Materialized composite candidate for a filter whose driver is *not*
    /// a single storage — today only [`Or`], whose driver is the
    /// deduplicated union of its arms. Returns `None` for every
    /// single-storage filter (they drive via
    /// [`candidate_slice`](Self::candidate_slice)).
    ///
    /// The union is sorted by `(index, generation)` and deduplicated, so an
    /// entity in two arms yields once and the order is a deterministic
    /// function of the inputs. The default returns `None`.
    fn candidate_materialize(_state: &Self::State<'_>) -> Option<Vec<Entity>> {
        None
    }
}

/// The always-true filter — the default `F` for `Query<'w, D>`.
///
/// Matches every entity and reports no access, so `Query<D>` and
/// `Query<D, ()>` are the same query.
impl QueryFilter for () {
    type State<'w> = ();

    fn init_state(_world: &World) -> Self::State<'_> {}

    fn matches(_entity: Entity, _state: &Self::State<'_>) -> bool {
        true
    }

    fn collect_access(_access: &mut QueryAccess) {}
}

/// Keeps only entities that **have** a `T` component (without fetching
/// it).
///
/// Zero-sized: the type parameter exists purely so the [`QueryFilter`]
/// impl can dispatch on `T`. Reports a **read** of `T` into the access
/// model — see the module-level docs.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
/// use spark_ecs::With;
///
/// #[derive(Component)]
/// struct Plant { output_mw: f32 }
/// #[derive(Component)]
/// struct Operational;          // marker — zero-sized
///
/// let mut world = World::new();
/// world.spawn().insert(Plant { output_mw: 4.0 }).insert(Operational);
/// world.spawn().insert(Plant { output_mw: 9.0 });   // offline
///
/// // Sum output of operational plants only.
/// let supply: f32 = Query::<&Plant, With<Operational>>::from_world(&world)
///     .iter()
///     .map(|p| p.output_mw)
///     .sum();
/// assert_eq!(supply, 4.0);
/// ```
pub struct With<T>(PhantomData<T>);

impl<T: Component> QueryFilter for With<T> {
    type State<'w> = Option<Ref<'w, ComponentStorage<T>>>;

    fn init_state(world: &World) -> Self::State<'_> {
        world.storage::<T>()
    }

    fn matches(entity: Entity, state: &Self::State<'_>) -> bool {
        state
            .as_ref()
            .is_some_and(|storage| storage.contains(entity))
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }

    fn candidate_slice<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        // Some(empty) when `T`'s storage is absent: the candidate exists and
        // is empty (nobody has `T`), so the query drives nothing rather than
        // falling back to the live set and rejecting everyone.
        Some(state.as_ref().map_or(&[], |storage| storage.entities()))
    }
}

/// Keeps only entities that **lack** a `T` component.
///
/// The mirror image of [`With<T>`]. Reports **no** access (a pure
/// presence-absence test), so it composes with a `&mut U` data shape for
/// any `U ≠ T` without tripping the self-conflict check.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
/// use spark_ecs::Without;
///
/// #[derive(Component)]
/// struct Worker { id: u32 }
/// #[derive(Component)]
/// struct CurrentJob;           // marker — zero-sized
///
/// let mut world = World::new();
/// world.spawn().insert(Worker { id: 1 }).insert(CurrentJob);
/// world.spawn().insert(Worker { id: 2 });   // idle
///
/// // Idle workers only.
/// let idle: Vec<u32> = Query::<&Worker, Without<CurrentJob>>::from_world(&world)
///     .iter()
///     .map(|w| w.id)
///     .collect();
/// assert_eq!(idle, vec![2]);
/// ```
pub struct Without<T>(PhantomData<T>);

impl<T: Component> QueryFilter for Without<T> {
    type State<'w> = Option<Ref<'w, ComponentStorage<T>>>;

    fn init_state(world: &World) -> Self::State<'_> {
        world.storage::<T>()
    }

    fn matches(entity: Entity, state: &Self::State<'_>) -> bool {
        // No storage for `T` → no entity has it → everyone passes.
        state
            .as_ref()
            .is_none_or(|storage| !storage.contains(entity))
    }

    fn collect_access(_access: &mut QueryAccess) {
        // Pure exclusion — reports no access. See module docs for why.
    }
}

/// Keeps only entities whose `T` was **written** since the calling system
/// last ran (insert, overwrite, or a `&mut T` write through [`Mut`]).
///
/// Zero-sized, like [`With<T>`]. Each component type carries its own
/// change-detection clock; this compares the entity's `changed_tick`
/// against [`World::baseline_for::<T>`](crate::World::baseline_for) — the
/// tick `T`'s clock read when this system last ran — with the wrapping-aware
/// relative-age test (`is_changed_since`), correct even when the clock has
/// wrapped past the baseline. Reports a **read** of `T`, so pairing it with
/// a `&mut T` data shape is a self-conflict (read with `&T`, or filter on a
/// different component).
///
/// Iterating a `Query<&mut T>` marks an entity changed only when the body
/// actually writes it (via [`Mut`]'s `DerefMut`), so `Changed<T>` is
/// precise — no over-marking of merely-visited or filtered-out entities.
///
/// [`Mut`]: crate::Mut
///
/// # Examples
///
/// ```
/// use spark_ecs::{Changed, Component, Query, World};
///
/// #[derive(Component)]
/// struct Health(u32);
///
/// let mut world = World::new();
/// world.spawn().insert(Health(100)); // Health's clock advances on insert
///
/// // With no baseline parked (system never ran → baseline 0), the fresh
/// // insert counts as changed.
/// let changed = Query::<&Health, Changed<Health>>::from_world(&world)
///     .iter()
///     .count();
/// assert_eq!(changed, 1);
/// ```
pub struct Changed<T>(PhantomData<T>);

/// True iff a write stamped at `tick` is newer than the `baseline`
/// observation, measured relative to `current` — this component's clock
/// when the filter was built. Shared by [`Changed`] and [`Added`].
///
/// # Why relative-age, not `tick > baseline`
///
/// The tick clock is `wrapping_add` ([`ComponentStorage::insert`]), so a
/// plain `tick > baseline` breaks the instant the clock wraps past a parked
/// baseline — a fresh write stamped post-wrap reads as *older*. Comparing
/// **distances from now** is wrapping-correct: `current - tick` is how long
/// ago the write happened, `current - baseline` how long ago the last
/// observation was, and the write is newer exactly when its distance is the
/// smaller. The strict `<` preserves the `tick == baseline` edge as *not
/// changed* (a write exactly at the baseline observation is not "since" it).
///
/// # Window
///
/// `current` is a true upper bound on both `tick` and `baseline` (the clock
/// only ever advances), so the wrapping subtractions are exact for any
/// baseline staleness below a full `u32` lap — 2³² ticks — which the
/// single-threaded frame model never reaches. A known upper bound buys the
/// **full** range, wider than the conventional half-range (2³¹)
/// serial-arithmetic window. The sole residual false-negative is a complete
/// 2³²-tick lap with no intervening run of the system: `current` then wraps
/// back onto `baseline` and a same-tick change is indistinguishable from
/// none (it collapses into the `tick == baseline` edge above).
fn is_changed_since(current: u32, tick: u32, baseline: u32) -> bool {
    current.wrapping_sub(tick) < current.wrapping_sub(baseline)
}

impl<T: Component> QueryFilter for Changed<T> {
    /// The `T` storage borrow + the baseline tick, both fetched once.
    type State<'w> = (Option<Ref<'w, ComponentStorage<T>>>, u32);

    fn init_state(world: &World) -> Self::State<'_> {
        (world.storage::<T>(), world.baseline_for::<T>())
    }

    fn matches(entity: Entity, state: &Self::State<'_>) -> bool {
        let (storage, baseline) = state;
        storage.as_ref().is_some_and(|s| {
            s.changed_tick_for(entity)
                .is_some_and(|tick| is_changed_since(s.current_tick(), tick, *baseline))
        })
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }

    fn candidate_slice<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        // Only entities holding `T` can carry a change tick, so `T`'s
        // population is the ceiling — the per-entity tick check (`matches`)
        // then rejects the unchanged ones. Never the live set.
        Some(state.0.as_ref().map_or(&[], |s| s.entities()))
    }
}

/// Keeps only entities to which `T` was **newly attached** since the
/// calling system last ran — a one-shot signal, unlike the every-write
/// [`Changed<T>`].
///
/// Zero-sized, like [`With<T>`]. Compares the entity's `added_tick` (set
/// only on a fresh attach, never re-stamped by overwrite or `&mut`)
/// against [`World::baseline_for::<T>`](crate::World::baseline_for) with the
/// same wrapping-aware relative-age test as [`Changed<T>`]
/// (`is_changed_since`). Reports a **read** of `T`, same as [`With<T>`].
///
/// Because `added_tick` does not move on later writes, an entity matches
/// `Added<T>` for exactly the run after its `T` was attached and never
/// again (until the component is removed and re-added).
///
/// # Examples
///
/// ```
/// use spark_ecs::{Added, Component, Query, World};
///
/// #[derive(Component)]
/// struct Spawned;
///
/// let mut world = World::new();
/// world.spawn().insert(Spawned);
///
/// let added = Query::<&Spawned, Added<Spawned>>::from_world(&world)
///     .iter()
///     .count();
/// assert_eq!(added, 1);
/// ```
pub struct Added<T>(PhantomData<T>);

impl<T: Component> QueryFilter for Added<T> {
    /// The `T` storage borrow + the baseline tick, both fetched once.
    type State<'w> = (Option<Ref<'w, ComponentStorage<T>>>, u32);

    fn init_state(world: &World) -> Self::State<'_> {
        (world.storage::<T>(), world.baseline_for::<T>())
    }

    fn matches(entity: Entity, state: &Self::State<'_>) -> bool {
        let (storage, baseline) = state;
        storage.as_ref().is_some_and(|s| {
            s.added_tick_for(entity)
                .is_some_and(|tick| is_changed_since(s.current_tick(), tick, *baseline))
        })
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }

    fn candidate_slice<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
        // Same ceiling as `Changed<T>`: only `T`-holders carry an
        // `added_tick`, so `T`'s population bounds iteration; `matches`
        // then keeps only the freshly-attached ones.
        Some(state.0.as_ref().map_or(&[], |s| s.entities()))
    }
}

/// Conjunction — passes only when **every** filter in the tuple passes.
///
/// `F` is a tuple of [`QueryFilter`]s (arities 2–4 ship). Spelled
/// explicitly rather than as a bare tuple so it stays symmetric with
/// [`Or`] and unambiguous when the two nest — a deliberate divergence
/// from Bevy's implicit tuple-AND.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
/// use spark_ecs::{And, With, Without};
///
/// #[derive(Component)]
/// struct Plant;
/// #[derive(Component)]
/// struct Operational;
/// #[derive(Component)]
/// struct UnderMaintenance;
///
/// let mut world = World::new();
/// // Operational, not under maintenance — matches.
/// world.spawn().insert(Plant).insert(Operational);
/// // Operational *and* under maintenance — excluded.
/// world.spawn().insert(Plant).insert(Operational).insert(UnderMaintenance);
///
/// let n = Query::<&Plant, And<(With<Operational>, Without<UnderMaintenance>)>>::from_world(&world)
///     .iter()
///     .count();
/// assert_eq!(n, 1);
/// ```
pub struct And<F>(PhantomData<F>);

/// Disjunction — passes when **any** filter in the tuple passes.
///
/// `F` is a tuple of [`QueryFilter`]s (arities 2–4 ship). Access is the
/// conservative **union** of every branch's access, even though a given
/// entity may have matched through only one of them.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
/// use spark_ecs::{Or, With};
///
/// #[derive(Component)]
/// struct Building;
/// #[derive(Component)]
/// struct Powered;
/// #[derive(Component)]
/// struct Backup;
///
/// let mut world = World::new();
/// world.spawn().insert(Building).insert(Powered);
/// world.spawn().insert(Building).insert(Backup);
/// world.spawn().insert(Building);   // neither — excluded
///
/// // Buildings that have grid power *or* a backup source.
/// let n = Query::<&Building, Or<(With<Powered>, With<Backup>)>>::from_world(&world)
///     .iter()
///     .count();
/// assert_eq!(n, 2);
/// ```
pub struct Or<F>(PhantomData<F>);

/// Emits the [`QueryFilter`] impls for `And<(…)>` and `Or<(…)>` at one
/// tuple arity. `And` short-circuits on the first non-match (`&&`), `Or`
/// on the first match (`||`); both report the union of their children's
/// access. Adding an arity is one more invocation below.
//
// Codegen census: this is one of the crate's declarative-macro families;
// see the variant manifest at the top of `query/tuple_codegen.rs`.
macro_rules! impl_logical_filter {
    ($($F:ident),+) => {
        impl<$($F: QueryFilter),+> QueryFilter for And<($($F,)+)> {
            type State<'w>
                = ($($F::State<'w>,)+)
            where
                Self: 'w;

            fn init_state(world: &World) -> Self::State<'_> {
                ($($F::init_state(world),)+)
            }

            #[allow(non_snake_case)]
            fn matches(entity: Entity, state: &Self::State<'_>) -> bool {
                let ($($F,)+) = state;
                $($F::matches(entity, $F))&&+
            }

            fn collect_access(access: &mut QueryAccess) {
                $($F::collect_access(access);)+
            }

            // `And` matches the intersection, so its result is bounded by
            // every arm — the tightest arm bounds it. Drive the smallest
            // arm's candidate (ties → earliest arm, via `min_by_key`'s
            // first-minimum rule) and let `matches` reject the rest. Arms
            // offering no candidate (e.g. `Without`) drop out via `flatten`.
            #[allow(non_snake_case)]
            fn candidate_slice<'s>(state: &'s Self::State<'_>) -> Option<&'s [Entity]> {
                let ($($F,)+) = state;
                [$($F::candidate_slice($F),)+]
                    .into_iter()
                    .flatten()
                    .min_by_key(|s| s.len())
            }
        }

        impl<$($F: QueryFilter),+> QueryFilter for Or<($($F,)+)> {
            type State<'w>
                = ($($F::State<'w>,)+)
            where
                Self: 'w;

            fn init_state(world: &World) -> Self::State<'_> {
                ($($F::init_state(world),)+)
            }

            #[allow(non_snake_case)]
            fn matches(entity: Entity, state: &Self::State<'_>) -> bool {
                let ($($F,)+) = state;
                $($F::matches(entity, $F))||+
            }

            fn collect_access(access: &mut QueryAccess) {
                $($F::collect_access(access);)+
            }

            // `Or` drives the deduplicated *union* of its arms — so it can
            // surface a candidate only when **every** arm is itself a single
            // storage (the `?` on `candidate_slice` bails to `None` if any
            // arm is a `Without`, `()`, or nested `Or`). The length is the
            // sum of arm populations: an upper bound on `|⋃ arms|`, exact
            // enough to choose a driver.
            #[allow(non_snake_case)]
            fn candidate_len(state: &Self::State<'_>) -> Option<usize> {
                let ($($F,)+) = state;
                let mut total = 0usize;
                $(total += $F::candidate_slice($F)?.len();)+
                Some(total)
            }

            // Materializes the union: concatenate every arm's candidate, then
            // sort by `(index, generation)` and dedup so an entity in two
            // arms yields once and the order is deterministic. `matches`
            // re-checks each entity, so driving a superset (e.g. an `And`
            // arm's smallest-arm candidate) stays correct.
            #[allow(non_snake_case)]
            fn candidate_materialize(state: &Self::State<'_>) -> Option<Vec<Entity>> {
                let ($($F,)+) = state;
                let mut union: Vec<Entity> = Vec::new();
                $(union.extend_from_slice($F::candidate_slice($F)?);)+
                union.sort_unstable_by_key(|e| (e.index, e.generation));
                union.dedup();
                Some(union)
            }
        }
    };
}

impl_logical_filter!(F1, F2);
impl_logical_filter!(F1, F2, F3);
impl_logical_filter!(F1, F2, F3, F4);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::World;

    // ── wrapping-aware change detection (issue #80 Phase 4) ──────────────
    //
    // `is_changed_since(current, tick, baseline)` decides "is the write at
    // `tick` newer than the `baseline` observation, as seen from `current`".
    // Testing it directly pins the contract without driving a clock 2³² times.

    #[test]
    fn changed_since_basic_ordering() {
        assert!(is_changed_since(10, 10, 5), "fresh write, older baseline");
        assert!(
            !is_changed_since(10, 3, 5),
            "write strictly before baseline"
        );
    }

    #[test]
    fn changed_since_strict_edge_tick_equals_baseline_is_not_changed() {
        // A write stamped exactly at the baseline observation is NOT "since"
        // it — the strict `<` preserves this edge for every `current`.
        for current in [0u32, 1, 100, u32::MAX - 1, u32::MAX] {
            assert!(!is_changed_since(current, 42, 42), "current = {current}");
        }
    }

    #[test]
    fn changed_since_detects_across_a_wrap() {
        // The fix: clock wrapped to 0, write stamped at 0, baseline parked
        // just before the wrap. The old strict `tick > baseline`
        // (`0 > u32::MAX - 1`) missed it; the relative-age test
        // (`0 - 0 = 0 < 0 - (u32::MAX - 1) = 2`) detects it.
        assert!(is_changed_since(0, 0, u32::MAX - 1));
    }

    #[test]
    fn changed_since_window_is_the_full_u32_range_not_half() {
        // NOTE — deliberate deviation from issue #80's text, which pinned a
        // 2³¹ window ("detected at T + 2³¹ − 1, fail-mode at T + 2³¹"). That
        // is the conventional *half-range* serial-arithmetic bound, which
        // applies when ordering two ticks with no known reference. Here
        // `current` is a true upper bound on both `tick` and `baseline` (the
        // clock only advances), which buys the **full** range: a fresh write
        // is detected at every baseline staleness short of a complete lap —
        // including 2³¹, 2³¹ + 1, and 2³² − 1, where a half-range comparison
        // would already have flipped to a false-negative.
        let t = 100u32;
        let fresh = |staleness: u32| {
            let current = t.wrapping_add(staleness); // a fresh write at `current`
            is_changed_since(current, current, t)
        };
        assert!(fresh(1), "just after baseline");
        assert!(fresh(1 << 31), "2³¹ stale — the issue's claimed fail point");
        assert!(fresh((1 << 31) + 1), "2³¹ + 1 stale");
        assert!(fresh(u32::MAX), "2³² − 1 stale — the last detectable point");
    }

    #[test]
    fn changed_since_residual_false_negative_only_at_a_full_lap() {
        // The sole residual: a complete 2³²-tick lap with no intervening run
        // of the system wraps `current` back onto `baseline` (staleness 0),
        // and a same-tick write collapses into the `tick == baseline` edge.
        // This needs 2³² ticks between two runs of one system — unreachable
        // in the single-threaded frame model.
        let t = 100u32;
        let current = t; // a full lap has wrapped `current` back onto the baseline
        assert!(!is_changed_since(current, current, t));
    }

    #[derive(Component)]
    struct A;
    #[derive(Component)]
    struct B;
    #[derive(Component)]
    struct C;

    // Spawns three entities: one with A, one with A+B, one with A+C.
    // Returns their ids in that order.
    fn world_abc() -> (World, Entity, Entity, Entity) {
        let mut world = World::new();
        let only_a = world.spawn().insert(A).id();
        let a_and_b = world.spawn().insert(A).insert(B).id();
        let a_and_c = world.spawn().insert(A).insert(C).id();
        (world, only_a, a_and_b, a_and_c)
    }

    /// `F::matches` with its state fetched once — the exact shape
    /// `Query::iter` uses (`init_state` then `matches`). Presence filters
    /// need no parked baseline, so a bare `World` is enough.
    fn passes<F: QueryFilter>(world: &World, e: Entity) -> bool {
        F::matches(e, &F::init_state(world))
    }

    #[test]
    fn unit_filter_matches_everything() {
        let (world, only_a, a_and_b, _) = world_abc();
        assert!(passes::<()>(&world, only_a));
        assert!(passes::<()>(&world, a_and_b));
    }

    #[test]
    fn with_matches_only_entities_having_component() {
        let (world, only_a, a_and_b, _) = world_abc();
        assert!(passes::<With<B>>(&world, a_and_b));
        assert!(!passes::<With<B>>(&world, only_a));
    }

    #[test]
    fn with_unknown_component_matches_nobody() {
        // A component never inserted has no storage at all — `With` must
        // say "no match", not panic.
        let mut world = World::new();
        let e = world.spawn().insert(A).id();
        assert!(!passes::<With<B>>(&world, e));
    }

    #[test]
    fn without_matches_entities_lacking_component() {
        let (world, only_a, a_and_b, _) = world_abc();
        assert!(passes::<Without<B>>(&world, only_a));
        assert!(!passes::<Without<B>>(&world, a_and_b));
    }

    #[test]
    fn without_unknown_component_matches_everyone() {
        // No storage for `C` yet → every entity lacks it → all pass.
        let mut world = World::new();
        let e = world.spawn().insert(A).id();
        assert!(passes::<Without<C>>(&world, e));
    }

    #[test]
    fn and_requires_all_branches() {
        let (world, only_a, a_and_b, _) = world_abc();
        // A present AND B absent.
        assert!(passes::<And<(With<A>, Without<B>)>>(&world, only_a));
        assert!(!passes::<And<(With<A>, Without<B>)>>(&world, a_and_b));
    }

    #[test]
    fn or_requires_any_branch() {
        let (world, only_a, a_and_b, a_and_c) = world_abc();
        // Has B or C.
        assert!(passes::<Or<(With<B>, With<C>)>>(&world, a_and_b));
        assert!(passes::<Or<(With<B>, With<C>)>>(&world, a_and_c));
        assert!(!passes::<Or<(With<B>, With<C>)>>(&world, only_a));
    }

    #[test]
    fn nested_and_of_or_composes() {
        // With<A> AND (With<B> OR With<C>).
        type F = And<(With<A>, Or<(With<B>, With<C>)>)>;
        let (world, only_a, a_and_b, a_and_c) = world_abc();
        assert!(passes::<F>(&world, a_and_b));
        assert!(passes::<F>(&world, a_and_c));
        assert!(!passes::<F>(&world, only_a)); // has A, but neither B nor C
    }

    #[test]
    fn with_reports_a_read_of_its_component() {
        // `With<A>` must contribute a read of `A`; combined with a write
        // of `A` the self-conflict check fires.
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        With::<A>::collect_access(&mut access);
        let conflicted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            access.assert_no_self_conflict();
        }))
        .is_err();
        assert!(conflicted, "With<A> should report a read of A");
    }

    #[test]
    fn without_reports_no_access() {
        // `Without<A>` reports nothing, so even a write of `A` alongside
        // it is conflict-free at the access level.
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        Without::<A>::collect_access(&mut access);
        access.assert_no_self_conflict(); // must not panic
    }

    #[test]
    fn and_unions_child_access() {
        // `And<(With<A>, With<B>)>` reports reads of both A and B.
        let mut access = QueryAccess::default();
        access.add_write::<B>();
        And::<(With<A>, With<B>)>::collect_access(&mut access);
        let conflicted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            access.assert_no_self_conflict();
        }))
        .is_err();
        assert!(conflicted, "And should union child access (read of B)");
    }

    // -------- change-detection filters --------

    use crate::Access;
    use std::any::TypeId;

    /// Evaluates `F::matches(e)` with `baseline` parked as the running
    /// system's per-component baselines — exactly how the scheduler feeds
    /// `Changed`/`Added`. Empty access ⇒ no clock advance, no observe.
    fn matches_with<F: QueryFilter>(
        world: &mut World,
        e: Entity,
        baseline: &[(TypeId, u32)],
    ) -> bool {
        let access = Access::new();
        let mut last_seen = baseline.to_vec();
        let mut result = false;
        world.run_system(&access, &mut last_seen, &mut |w| {
            // `init_state` reads the parked baseline (and storage borrow),
            // exactly as `Query::iter` does, before per-entity `matches`.
            result = F::matches(e, &F::init_state(w));
        });
        result
    }

    #[test]
    fn changed_matches_when_clock_is_past_baseline() {
        let mut world = World::new();
        let e = world.spawn().insert(A).id(); // A's clock 1→2, changed = 2
        let aid = TypeId::of::<A>();
        assert!(matches_with::<Changed<A>>(&mut world, e, &[(aid, 1)])); // 2 > 1
        assert!(!matches_with::<Changed<A>>(&mut world, e, &[(aid, 2)])); // 2 > 2 false
    }

    #[test]
    fn added_is_one_shot_but_changed_repeats() {
        let mut world = World::new();
        let e = world.spawn().insert(A).id(); // added = changed = 2
        world.insert(e, A); // overwrite: clock → 3, changed = 3, added stays 2
        let aid = TypeId::of::<A>();
        // From baseline 2: Added (2 > 2) is false; Changed (3 > 2) is true.
        assert!(!matches_with::<Added<A>>(&mut world, e, &[(aid, 2)]));
        assert!(matches_with::<Changed<A>>(&mut world, e, &[(aid, 2)]));
    }

    #[test]
    fn change_filters_compose_with_combinators() {
        let (mut world, only_a, a_and_b, _a_and_c) = world_abc();
        let aid = TypeId::of::<A>();
        let bid = TypeId::of::<B>();
        let cid = TypeId::of::<C>();
        // Has A and B was changed since baseline 0.
        assert!(matches_with::<And<(With<A>, Changed<B>)>>(
            &mut world,
            a_and_b,
            &[(aid, 0), (bid, 0)]
        ));
        // only_a lacks B → the And fails.
        assert!(!matches_with::<And<(With<A>, Changed<B>)>>(
            &mut world,
            only_a,
            &[(aid, 0), (bid, 0)]
        ));
        // B or C added since baseline 0 — a_and_b has B.
        assert!(matches_with::<Or<(Added<B>, Added<C>)>>(
            &mut world,
            a_and_b,
            &[(bid, 0), (cid, 0)]
        ));
        assert!(!matches_with::<Or<(Added<B>, Added<C>)>>(
            &mut world,
            only_a,
            &[(bid, 0), (cid, 0)]
        ));
    }

    #[test]
    fn change_filters_on_unknown_component_match_nobody() {
        let mut world = World::new();
        let e = world.spawn().insert(A).id();
        assert!(!matches_with::<Changed<B>>(&mut world, e, &[]));
        assert!(!matches_with::<Added<B>>(&mut world, e, &[]));
    }
}
