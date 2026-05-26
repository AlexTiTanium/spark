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
//! [`QueryFilter::matches`] takes the entity and a `&World` and returns a
//! bool. [`Query::iter`](crate::Query::iter) wraps the existing data
//! driver in a `.filter(…)` that calls it once per candidate entity, so
//! the filter rides on top of the safe iteration path — no new `unsafe`,
//! no change to the join machinery. The presence check goes through the
//! same `World::storage::<T>()` accessor [`QueryData::init_state`] uses,
//! so the M4 `RefCell → UnsafeCell` swap stays a single chokepoint.
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
//!   (mutate `T` on entities that lack `T` — always empty) is *not*
//!   caught at construction; if `T`'s storage is non-empty it surfaces
//!   later as the `RefCell`'s "already mutably borrowed" when `matches`
//!   re-borrows `T` mid-iteration. That query is meaningless anyway.
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
//! the world via [`World::baseline_for`](crate::World::baseline_for) for
//! the static [`matches`](QueryFilter::matches) to read.
//!
//! [`Changed<T>`] passes when `changed_tick > baseline`; [`Added<T>`]
//! when `added_tick > baseline` (a one-shot, since `added_tick` never
//! moves after the attach). Both report a **read** of `T`, like
//! [`With<T>`], so `Query<&mut T, Changed<T>>` is a self-conflict (use
//! `&T`, or detect a *different* component).
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

use std::marker::PhantomData;

use crate::Component;
use crate::access::QueryAccess;
use crate::entity::Entity;
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
    /// Returns `true` if `entity` passes this filter.
    ///
    /// Called once per candidate entity during iteration. Reads
    /// component presence through `world`; performs no mutation.
    fn matches(entity: Entity, world: &World) -> bool;

    /// Reports the components this filter inspects into `access`.
    ///
    /// Runs at [`Query::from_world`](crate::Query::from_world) time
    /// alongside the data shape's own access, so the self-conflict check
    /// and (later) the scheduler see one combined set. See the
    /// module-level docs for why [`With`] reports a read but [`Without`]
    /// reports nothing.
    fn collect_access(access: &mut QueryAccess);
}

/// The always-true filter — the default `F` for `Query<'w, D>`.
///
/// Matches every entity and reports no access, so `Query<D>` and
/// `Query<D, ()>` are the same query.
impl QueryFilter for () {
    fn matches(_entity: Entity, _world: &World) -> bool {
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
    fn matches(entity: Entity, world: &World) -> bool {
        world
            .storage::<T>()
            .is_some_and(|storage| storage.contains(entity))
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
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
    fn matches(entity: Entity, world: &World) -> bool {
        // No storage for `T` → no entity has it → everyone passes.
        world
            .storage::<T>()
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
/// tick `T`'s clock read when this system last ran — with a strict `>`.
/// Reports a **read** of `T`, so pairing it with a `&mut T` data shape is
/// a self-conflict (read with `&T`, or filter on a different component).
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

impl<T: Component> QueryFilter for Changed<T> {
    fn matches(entity: Entity, world: &World) -> bool {
        world.storage::<T>().is_some_and(|storage| {
            storage
                .changed_tick_for(entity)
                .is_some_and(|tick| tick > world.baseline_for::<T>())
        })
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }
}

/// Keeps only entities to which `T` was **newly attached** since the
/// calling system last ran — a one-shot signal, unlike the every-write
/// [`Changed<T>`].
///
/// Zero-sized, like [`With<T>`]. Compares the entity's `added_tick` (set
/// only on a fresh attach, never re-stamped by overwrite or `&mut`)
/// against [`World::baseline_for::<T>`](crate::World::baseline_for) with a
/// strict `>`. Reports a **read** of `T`, same as [`With<T>`].
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
    fn matches(entity: Entity, world: &World) -> bool {
        world.storage::<T>().is_some_and(|storage| {
            storage
                .added_tick_for(entity)
                .is_some_and(|tick| tick > world.baseline_for::<T>())
        })
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
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
macro_rules! impl_logical_filter {
    ($($F:ident),+) => {
        impl<$($F: QueryFilter),+> QueryFilter for And<($($F,)+)> {
            fn matches(entity: Entity, world: &World) -> bool {
                $($F::matches(entity, world))&&+
            }

            fn collect_access(access: &mut QueryAccess) {
                $($F::collect_access(access);)+
            }
        }

        impl<$($F: QueryFilter),+> QueryFilter for Or<($($F,)+)> {
            fn matches(entity: Entity, world: &World) -> bool {
                $($F::matches(entity, world))||+
            }

            fn collect_access(access: &mut QueryAccess) {
                $($F::collect_access(access);)+
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

    #[test]
    fn unit_filter_matches_everything() {
        let (world, only_a, a_and_b, _) = world_abc();
        assert!(<() as QueryFilter>::matches(only_a, &world));
        assert!(<() as QueryFilter>::matches(a_and_b, &world));
    }

    #[test]
    fn with_matches_only_entities_having_component() {
        let (world, only_a, a_and_b, _) = world_abc();
        assert!(With::<B>::matches(a_and_b, &world));
        assert!(!With::<B>::matches(only_a, &world));
    }

    #[test]
    fn with_unknown_component_matches_nobody() {
        // No entity ever held a `B`-less... rather, a component never
        // inserted has no storage at all — `With` must say "no match",
        // not panic.
        let mut world = World::new();
        let e = world.spawn().insert(A).id();
        assert!(!With::<B>::matches(e, &world));
    }

    #[test]
    fn without_matches_entities_lacking_component() {
        let (world, only_a, a_and_b, _) = world_abc();
        assert!(Without::<B>::matches(only_a, &world));
        assert!(!Without::<B>::matches(a_and_b, &world));
    }

    #[test]
    fn without_unknown_component_matches_everyone() {
        // No storage for `C` yet → every entity lacks it → all pass.
        let mut world = World::new();
        let e = world.spawn().insert(A).id();
        assert!(Without::<C>::matches(e, &world));
    }

    #[test]
    fn and_requires_all_branches() {
        let (world, only_a, a_and_b, _) = world_abc();
        // A present AND B absent.
        assert!(And::<(With<A>, Without<B>)>::matches(only_a, &world));
        assert!(!And::<(With<A>, Without<B>)>::matches(a_and_b, &world));
    }

    #[test]
    fn or_requires_any_branch() {
        let (world, only_a, a_and_b, a_and_c) = world_abc();
        // Has B or C.
        assert!(Or::<(With<B>, With<C>)>::matches(a_and_b, &world));
        assert!(Or::<(With<B>, With<C>)>::matches(a_and_c, &world));
        assert!(!Or::<(With<B>, With<C>)>::matches(only_a, &world));
    }

    #[test]
    fn nested_and_of_or_composes() {
        // With<A> AND (With<B> OR With<C>).
        type F = And<(With<A>, Or<(With<B>, With<C>)>)>;
        let (world, only_a, a_and_b, a_and_c) = world_abc();
        assert!(F::matches(a_and_b, &world));
        assert!(F::matches(a_and_c, &world));
        assert!(!F::matches(only_a, &world)); // has A, but neither B nor C
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
            result = F::matches(e, w);
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
