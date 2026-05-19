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
//! For `Query<(&mut A, &B)>` the tuple impl drives `A`'s storage and
//! sparse-looks-up `B` per entity. Only entities present in both
//! storages survive. The read side must be [`ReadOnlyQueryData`]:
//! looking up `&mut B` from a borrowed `RefMut` would need a lending
//! iterator, which std's `Iterator` cannot express in safe Rust. The
//! follow-up multi-mut issue adds `(&mut A, &mut B)` via a localised
//! `unsafe` block.

use std::cell::{Ref, RefMut};

use crate::entity::Entity;
use crate::storage::{Component, ComponentStorage};
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
/// use spark_ecs::QueryData;
///
/// fn _accepts<D: QueryData>() {}
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
/// use spark_ecs::ReadOnlyQueryData;
///
/// fn _accepts<D: ReadOnlyQueryData>() {}
/// struct Position { x: f32, y: f32 }
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
}

// No ReadOnlyQueryData for &mut T — that's the entire point of the split.

// -------- Tuple impls (arity 2, 3, 4) --------

/// Emits a `QueryData` and `ReadOnlyQueryData` impl pair for a tuple
/// arity ≥ 2.
///
/// **Driver convention.** `$D1` drives iteration; every other element
/// sparse-looks up per entity. That's why everything after `$D1` is
/// bounded by [`ReadOnlyQueryData`]: a `&mut T` lookup off a shared
/// `Ref<…>` would need a lending iterator, which `Iterator` cannot
/// express in safe Rust. Practical fallout: `(&mut A, &B, &C)` works,
/// `(&A, &mut B, …)` does not (re-order to put the mut side first),
/// and multi-mut tuples land with the dedicated multi-mut issue.
///
/// **Variable names shadow type names.** Each `$D` does double duty in
/// the expanded body — once as a type generic in paths like
/// `$D::lookup(…)`, once as a `let`-bound state variable. Rust resolves
/// type vs value by syntactic position; the `#[allow(non_snake_case)]`
/// quiets the lint that would otherwise fire on `D2`, `D3`, … . This
/// is the same shadow trick [`crate::IntoSystem`]'s arity macro uses.
macro_rules! impl_query_data_tuple {
    ($D1:ident $(, $D:ident)+) => {
        impl<$D1: QueryData $(, $D: ReadOnlyQueryData)+> QueryData for ($D1, $($D,)+) {
            type Item<'w>
                = ($D1::Item<'w>, $($D::Item<'w>,)+)
            where
                Self: 'w;
            type State<'w>
                = ($D1::State<'w>, $($D::State<'w>,)+)
            where
                Self: 'w;

            fn init_state<'w>(world: &'w World) -> Self::State<'w>
            where
                Self: 'w,
            {
                ($D1::init_state(world), $($D::init_state(world),)+)
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
                let ($D1, $($D,)+) = state;
                // Pre-reborrow each non-driver state as `&'s` *before*
                // moving into the closure. Inside the closure each call
                // would otherwise only see a transient `&$D::State<'_>`
                // with closure-body lifetime, not `'s` — and
                // `$D::lookup` would then yield items with that shorter
                // lifetime, breaking the iterator's `Item<'s>`.
                $(let $D: &'s $D::State<'w> = &*$D;)+
                Box::new($D1::iter($D1).filter_map(move |(entity, item_1)| {
                    Some((entity, (item_1, $($D::lookup($D, entity)?,)+)))
                }))
            }
        }

        impl<$D1: ReadOnlyQueryData $(, $D: ReadOnlyQueryData)+> ReadOnlyQueryData
            for ($D1, $($D,)+)
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
                let ($D1, $($D,)+) = state;
                Box::new($D1::iter_ref($D1).filter_map(move |(entity, item_1)| {
                    Some((entity, (item_1, $($D::lookup($D, entity)?,)+)))
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
                let ($D1, $($D,)+) = state;
                Some(($D1::lookup($D1, entity)?, $($D::lookup($D, entity)?,)+))
            }
        }
    };
}

impl_query_data_tuple!(D1, D2);
impl_query_data_tuple!(D1, D2, D3);
impl_query_data_tuple!(D1, D2, D3, D4);

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
/// # Examples
///
/// Single-component read — yields `&Position`, not `(Entity, &Position)`:
///
/// ```
/// use spark_ecs::{Query, World};
///
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
/// use spark_ecs::{Query, World};
///
/// struct Position { x: f32, y: f32 }
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
pub struct Query<'w, D: QueryData + 'w> {
    state: D::State<'w>,
}

impl<'w, D: QueryData + 'w> Query<'w, D> {
    /// Fetches a `Query` directly from a [`World`]. Convenience for
    /// tests and doc examples; system fns receive their `Query` from
    /// the runner via [`SystemParam::fetch`].
    ///
    /// # Panics
    ///
    /// Panics when `D` contains a `&mut T` and that storage is already
    /// borrowed (shared or exclusive).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Query, World};
    ///
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
        Self {
            state: D::init_state(world),
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
    /// use spark_ecs::{Query, World};
    ///
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
    pub fn iter_mut(&mut self) -> impl Iterator<Item = D::Item<'_>> + '_ {
        // Path B — strip the entity that the trait threads internally
        // for join logic. See module-level docs.
        D::iter(&mut self.state).map(|(_entity, item)| item)
    }
}

impl<'w, D: ReadOnlyQueryData + 'w> Query<'w, D> {
    /// Shared iteration. Available only for `D: ReadOnlyQueryData`
    /// (no `&mut T` anywhere in the shape).
    ///
    /// Yields `D::Item<'_>` directly — no `(Entity, …)` prefix.
    /// Path B; see the module-level docs.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Query, World};
    ///
    /// struct Position { x: f32, y: f32 }
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
    pub fn iter(&self) -> impl Iterator<Item = D::Item<'_>> + '_ {
        // Path B — see `Query::iter_mut` for the rationale.
        D::iter_ref(&self.state).map(|(_entity, item)| item)
    }
}

impl<D: QueryData> SystemParam for Query<'_, D> {
    type Item<'w>
        = Query<'w, D>
    where
        Self: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        Self: 'w,
    {
        Query::from_world(world)
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
    use crate::system::IntoSystem;

    // Integer fields keep unit tests free of `clippy::float_cmp`
    // assertions. Doc tests stay with the canonical `f32` flavour to
    // read like real engine code.
    #[derive(Debug, PartialEq)]
    struct Position(i32, i32);

    #[derive(Debug, PartialEq)]
    struct Velocity(i32, i32);

    #[derive(Debug, PartialEq)]
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
    #[derive(Debug, PartialEq)]
    struct A(i32);
    #[derive(Debug, PartialEq)]
    struct B(i32);
    #[derive(Debug, PartialEq)]
    struct C(i32);
    #[derive(Debug, PartialEq)]
    struct D(i32);

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
    fn query_nested_tuple_works_via_recursive_two_tuple_impl() {
        // `((&A, &B), (&C, &D))` exercises the 2-tuple `QueryData` /
        // `ReadOnlyQueryData` impls *recursively*: each half is a
        // 2-tuple, and the outer pair is another 2-tuple over those
        // halves. The driver convention picks the left half to drive
        // and the right half to per-entity-lookup; the right half then
        // does its own 2-tuple lookup internally.
        let mut world = World::new();
        world
            .spawn()
            .insert(A(1))
            .insert(B(2))
            .insert(C(3))
            .insert(D(4));
        // Missing D — should be skipped because (C, D) lookup fails.
        world.spawn().insert(A(10)).insert(B(20)).insert(C(30));

        let q = Query::<((&A, &B), (&C, &D))>::from_world(&world);
        let yielded: Vec<_> = q
            .iter()
            .map(|((a, b), (c, d))| (a.0, b.0, c.0, d.0))
            .collect();
        assert_eq!(yielded, vec![(1, 2, 3, 4)]);
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
}
