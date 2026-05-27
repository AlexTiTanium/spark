//! The [`QueryData`] / [`ReadOnlyQueryData`] trait definitions and every
//! single-component impl: `&T`, `&mut T`, `Option<&T>`, `Option<&mut T>`,
//! and `Entity` (the id itself, as data).
//!
//! The trait is the type-level description of what a [`Query`](super::super::Query)
//! fetches; these hand-written impls cover the arity-1 shapes, while the
//! tuple shapes (arity 2+) are generated in
//! [`tuple_codegen`](super::tuple_codegen). The driver runtime
//! ([`DriveSource`](super::DriveSource) / [`DriverIter`](super::DriverIter))
//! and the `counted!` test harness live in the parent [`query`](super)
//! module and are reused here unchanged.

use std::cell::{Ref, RefMut};

use crate::Component;
use crate::access::QueryAccess;
use crate::entity::Entity;
use crate::storage::{ComponentStorage, Mut};
use crate::world::World;

use super::DriveSource;
use super::dense_mut::DenseMut;

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
/// `&mut T`, `Option<&T>` / `Option<&mut T>`, and tuples cover every query
/// shape. Naming it is useful when writing generic helpers; here we just
/// confirm the impl exists:
///
/// ```
/// use spark_ecs::{Component, QueryData};
///
/// fn _accepts<D: QueryData>() {}
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// _accepts::<&Position>();
/// _accepts::<&mut Position>();
/// _accepts::<Option<&Position>>();
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

    /// The smallest *candidate* among this shape's component elements: the
    /// `(element index, population)` of the data element with the fewest
    /// entities, ties broken toward the earliest element. `None` when the
    /// shape names no component ([`Entity`] alone) — it then offers nothing
    /// to drive off.
    ///
    /// Read once at [`Query::from_world`] to pick the query-wide driver
    /// (issue #65); each population is an O(1) `len()`. The default returns
    /// `None`, so a shape that does not override it simply never wins driver
    /// selection and falls back to today's behavior.
    fn min_data_candidate<'w>(_state: &Self::State<'w>) -> Option<(usize, usize)>
    where
        Self: 'w,
    {
        None
    }

    /// Exclusive iteration driven by `driver` — the generalization of
    /// [`iter`](Self::iter) that lets the *smallest* candidate lead the loop
    /// instead of always the first element (issue #65).
    ///
    /// [`DriveSource::Data(k)`](DriveSource::Data) drives off element `k`'s
    /// entities; [`DriveSource::External`] drives off a filter's candidate,
    /// looking every element up per entity. The shipped impls override this;
    /// the default ignores `driver` and falls back to [`iter`](Self::iter) —
    /// correct, just unoptimized, so any external impl keeps working.
    fn drive<'s, 'w>(
        state: &'s mut Self::State<'w>,
        _driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        Self::iter(state)
    }
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
/// _accepts::<Option<&Position>>();                  // optional is read-only
/// _accepts::<(&Position, Option<&Velocity>)>();
/// // _accepts::<&mut Position>();                   // would not compile — exclusive
/// // _accepts::<(&Position, Option<&mut Velocity>)>(); // nor this — exclusive
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

    /// Shared mirror of [`QueryData::drive`] for [`Query::iter`]. The shipped
    /// impls override it; the default ignores `driver` and falls back to
    /// [`iter_ref`](Self::iter_ref).
    fn drive_ref<'s, 'w>(
        state: &'s Self::State<'w>,
        _driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Self::Item<'s>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        Self::iter_ref(state)
    }
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
            Some(storage) => Box::new(counted!(storage.iter())),
            None => Box::new(std::iter::empty()),
        }
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }

    fn min_data_candidate<'w>(state: &Self::State<'w>) -> Option<(usize, usize)>
    where
        Self: 'w,
    {
        Some((0, state.as_ref().map_or(0, |s| s.len())))
    }

    fn drive<'s, 'w>(
        state: &'s mut Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            // Single element: the only driver is itself, whatever index the
            // plan chose — delegate to `iter` (yields the value directly, no
            // redundant lookup).
            DriveSource::Data(_) => Self::iter(state),
            DriveSource::External(di) => match state {
                Some(storage) => {
                    let storage = &*storage;
                    Box::new(counted!(di).filter_map(move |e| storage.get(e).map(|v| (e, v))))
                }
                None => Box::new(std::iter::empty()),
            },
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
            Some(storage) => Box::new(counted!(storage.iter())),
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

    fn drive_ref<'s, 'w>(
        state: &'s Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, &'s T)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::Data(_) => Self::iter_ref(state),
            DriveSource::External(di) => match state {
                Some(storage) => {
                    Box::new(counted!(di).filter_map(move |e| storage.get(e).map(|v| (e, v))))
                }
                None => Box::new(std::iter::empty()),
            },
        }
    }
}

// -------- &mut T --------

#[allow(unsafe_code)] // `drive`'s external path looks up `&mut T` via `DenseMut`.
impl<T: Component> QueryData for &mut T {
    type Item<'w>
        = Mut<'w, T>
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
    ) -> Box<dyn Iterator<Item = (Entity, Mut<'s, T>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match state {
            Some(storage) => Box::new(counted!(storage.iter_mut())),
            None => Box::new(std::iter::empty()),
        }
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_write::<T>();
    }

    fn min_data_candidate<'w>(state: &Self::State<'w>) -> Option<(usize, usize)>
    where
        Self: 'w,
    {
        Some((0, state.as_ref().map_or(0, |s| s.len())))
    }

    fn drive<'s, 'w>(
        state: &'s mut Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Mut<'s, T>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::Data(_) => Self::iter(state),
            DriveSource::External(di) => match state.as_mut() {
                Some(storage) => {
                    let (dense, changed, sparse, ents, tick) = storage.split_for_join();
                    let view = DenseMut::new(dense, changed, sparse, ents, tick);
                    Box::new(counted!(di).filter_map(move |e| {
                        // SAFETY: the external driver visits each entity at
                        // most once (any entity list holds each entity once),
                        // and the self-conflict check ruled out `&mut T`
                        // appearing twice — so `get` runs at most once per
                        // entity across `'s`. See `DenseMut::get`.
                        unsafe { view.get(e) }.map(|v| (e, v))
                    }))
                }
                None => Box::new(std::iter::empty()),
            },
        }
    }
}

// No ReadOnlyQueryData for &mut T — that's the entire point of the split.

// -------- Option<&T> / Option<&mut T> (optional fetch) --------

/// Fetches `T` when the entity has it, yields `None` when it doesn't —
/// `Query<Option<&T>>` and the `Option<&T>` element of a join.
///
/// # Logic
///
/// `Option<&T>` is **not** a filter: it never removes an entity from the
/// result, it only decides whether the yielded item is `Some(&T)` or
/// `None`. A required `&T` skips entities that lack `T` (`fetch` returns
/// `None` and the join drops the row); `Option<&T>` keeps the row and
/// reports the absence as a `None` *value*.
///
/// # Why it never drives
///
/// Because it removes nothing, `Option<&T>` has no smaller candidate set
/// to offer — it keeps the `None` default of
/// [`min_data_candidate`](QueryData::min_data_candidate), exactly like
/// [`Entity`]. In a tuple the first element is always *required* (the
/// `impl_all_tuple_opt!` shape guarantees it), so a driver always exists —
/// the smallest required element drives and the optional is looked up per
/// entity. Standing alone, `Query<Option<&T>>` has no required element, so
/// driver selection falls to the live-set plan — the `World::live_entities`
/// snapshot captured in `State` — and every live entity is visited, each
/// yielding `Some`/`None`. That snapshot is why `State` is a
/// `(Vec<Entity>, Option<Ref<…>>)` pair rather than the bare storage borrow
/// `&T` uses.
///
/// # Snapshot semantics
///
/// Standalone, the snapshot follows the same contract as [`Entity`] (see its
/// *Snapshot semantics* section): it is frozen at query construction, so an
/// entity `Commands::despawn`'d mid-iteration is still yielded (despawn is
/// deferred) and one `Commands::spawn`'d after construction is not. In a join
/// the optional rides the driver and the snapshot is unused.
///
/// # Examples
///
/// Optional in a join — present on one entity, absent on the other; both
/// rows are yielded:
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
/// world.spawn().insert(Position { x: 1.0, y: 0.0 }).insert(Velocity { x: 2.0, y: 0.0 });
/// world.spawn().insert(Position { x: 3.0, y: 0.0 }); // no Velocity
///
/// let q = Query::<(&Position, Option<&Velocity>)>::from_world(&world);
/// let rows: Vec<_> = q.iter().map(|(p, v)| (p.x, v.map(|v| v.x))).collect();
/// assert!(rows.contains(&(1.0, Some(2.0)))); // Velocity present
/// assert!(rows.contains(&(3.0, None)));       // Velocity absent — still yielded
/// ```
///
/// Standing alone, it visits every live entity:
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
///
/// let mut world = World::new();
/// world.spawn().insert(Velocity { x: 2.0, y: 0.0 });
/// world.spawn(); // no components at all
///
/// let q = Query::<Option<&Velocity>>::from_world(&world);
/// assert_eq!(q.iter().count(), 2); // every live entity, Some or None
/// assert_eq!(q.iter().filter(|v| v.is_some()).count(), 1);
/// ```
///
/// The optional still reports its access, so mixing it with a conflicting
/// borrow of the same component panics at construction (note: the
/// `impl_all_tuple_opt!` shape forbids optional in the first slot, so the
/// reachable conflict shape is `(&A, Option<&mut A>)`, not the reversed
/// order):
///
/// ```should_panic
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position(f32, f32);
///
/// let mut world = World::new();
/// world.spawn().insert(Position(0.0, 0.0));
/// // `&Position` reads and `Option<&mut Position>` writes the SAME
/// // component — the per-query self-conflict check panics here.
/// let _q = Query::<(&Position, Option<&mut Position>)>::from_world(&world);
/// ```
///
/// An optional may only sit in a *trailing* position — the first element
/// must be required so a driver always exists. Optional-first is a compile
/// error (write the required element first):
///
/// ```compile_fail
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position(f32, f32);
/// #[derive(Component)]
/// struct Velocity(f32, f32);
///
/// let mut world = World::new();
/// world.spawn().insert(Position(0.0, 0.0)).insert(Velocity(1.0, 0.0));
/// // No impl for optional-first — use `(&Velocity, Option<&Position>)` instead.
/// let _q = Query::<(Option<&Position>, &Velocity)>::from_world(&world);
/// ```
impl<T: Component> QueryData for Option<&T> {
    type Item<'w>
        = Option<&'w T>
    where
        Self: 'w;
    // (live snapshot, storage borrow): the snapshot drives a standalone
    // `Query<Option<&T>>` (every live entity); the borrow fetches `T` per
    // entity. In a join the optional is never the driver, so only the
    // borrow is touched — the snapshot just rides along unused.
    type State<'w>
        = (Vec<Entity>, Option<Ref<'w, ComponentStorage<T>>>)
    where
        Self: 'w;

    fn init_state<'w>(world: &'w World) -> Self::State<'w>
    where
        Self: 'w,
    {
        (world.live_entities(), world.storage::<T>())
    }

    fn iter<'s, 'w>(
        state: &'s mut Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Option<&'s T>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        let (entities, storage) = state;
        let storage = &*storage;
        Box::new(
            counted!(entities.iter()).map(move |&e| (e, storage.as_ref().and_then(|s| s.get(e)))),
        )
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_read::<T>();
    }

    // `min_data_candidate` keeps the `None` default — an optional never
    // drives (it narrows nothing). See the type-level docs.

    fn drive<'s, 'w>(
        state: &'s mut Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Option<&'s T>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::External(di) => {
                // The external driver feeds entities; unlike `iter`, the
                // snapshot is unused here (the join's required element drives).
                let (_entities, storage) = state;
                let storage = &*storage;
                Box::new(counted!(di).map(move |e| (e, storage.as_ref().and_then(|s| s.get(e)))))
            }
            // Unreachable in practice: no candidate ⇒ the plan never picks
            // `Data` for an optional. Falls back to the snapshot drive.
            // (`<Self as QueryData>` disambiguates from `Option::iter`.)
            DriveSource::Data(_) => <Self as QueryData>::iter(state),
        }
    }
}

impl<T: Component> ReadOnlyQueryData for Option<&T> {
    fn iter_ref<'s, 'w>(
        state: &'s Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Option<&'s T>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        let (entities, storage) = state;
        Box::new(
            counted!(entities.iter()).map(move |&e| (e, storage.as_ref().and_then(|s| s.get(e)))),
        )
    }

    fn lookup<'s, 'w>(state: &'s Self::State<'w>, entity: Entity) -> Option<Option<&'s T>>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        // Always `Some(…)` — an optional never rejects the entity. The
        // inner `Option` carries the presence/absence of `T`.
        Some(state.1.as_ref().and_then(|s| s.get(entity)))
    }

    fn drive_ref<'s, 'w>(
        state: &'s Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Option<&'s T>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::External(di) => {
                // The external driver feeds entities; the snapshot is unused.
                let (_entities, storage) = state;
                Box::new(counted!(di).map(move |e| (e, storage.as_ref().and_then(|s| s.get(e)))))
            }
            DriveSource::Data(_) => Self::iter_ref(state),
        }
    }
}

/// The mutable optional — `Option<&mut T>`. Same never-skips semantics as
/// `Option<&T>`, but hands out a change-marking [`Mut<T>`](crate::Mut) per
/// present entity.
///
/// Like `&mut T` it does **not** implement [`ReadOnlyQueryData`] (the write
/// claim is exclusive) and its per-entity lookup goes through the same
/// unsafe `DenseMut` view. The safety contract — each entity fetched at most
/// once across `'s` — holds because the live snapshot (standalone, frozen at
/// construction like `Option<&T>`'s) and every built-in driver list each
/// entity exactly once (storage entity lists are duplicate-free by the
/// sparse-set invariant; `Or` dedups its union), and
/// [`QueryAccess::assert_no_self_conflict`] rules out the component appearing
/// twice.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Health(i32);
/// #[derive(Component)]
/// struct Poison(i32);
///
/// let mut world = World::new();
/// world.spawn().insert(Health(100)).insert(Poison(5));
/// world.spawn().insert(Health(50)); // no Poison
///
/// {
///     let mut q = Query::<(&mut Health, Option<&mut Poison>)>::from_world(&world);
///     for (mut hp, poison) in q.iter_mut() {
///         if let Some(mut p) = poison {
///             hp.0 -= p.0; // apply poison damage…
///             p.0 = 0;     // …and consume it
///         }
///     }
/// }
///
/// let hps: Vec<i32> = Query::<&Health>::from_world(&world).iter().map(|h| h.0).collect();
/// assert!(hps.contains(&95)); // 100 - 5, had Poison
/// assert!(hps.contains(&50)); // untouched, no Poison
/// ```
///
/// A shape containing `Option<&mut T>` is exclusive, so it is **not**
/// [`ReadOnlyQueryData`] — `.iter()` is a compile error (use `.iter_mut()`):
///
/// ```compile_fail
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Position(i32, i32);
/// #[derive(Component)]
/// struct Velocity(i32, i32);
///
/// let mut world = World::new();
/// world.spawn().insert(Position(0, 0)).insert(Velocity(1, 0));
///
/// let q = Query::<(&Position, Option<&mut Velocity>)>::from_world(&world);
/// for _ in q.iter() {} // ← `Option<&mut Velocity>` ⇒ not read-only: compile error
/// ```
#[allow(unsafe_code)] // per-entity `&mut T` lookup goes through `DenseMut`.
impl<T: Component> QueryData for Option<&mut T> {
    type Item<'w>
        = Option<Mut<'w, T>>
    where
        Self: 'w;
    type State<'w>
        = (Vec<Entity>, Option<RefMut<'w, ComponentStorage<T>>>)
    where
        Self: 'w;

    fn init_state<'w>(world: &'w World) -> Self::State<'w>
    where
        Self: 'w,
    {
        (world.live_entities(), world.storage_mut::<T>())
    }

    fn iter<'s, 'w>(
        state: &'s mut Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Option<Mut<'s, T>>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        let (entities, storage) = state;
        let view = storage.as_mut().map(|refmut| {
            let (dense, changed, sparse, ents, tick) = refmut.split_for_join();
            DenseMut::new(dense, changed, sparse, ents, tick)
        });
        Box::new(counted!(entities.iter()).map(move |&e| {
            // SAFETY: the live snapshot lists each entity once, so `get`
            // runs at most once per entity across `'s`. See `DenseMut::get`.
            (e, unsafe { view.as_ref().and_then(|v| v.get(e)) })
        }))
    }

    fn collect_access(access: &mut QueryAccess) {
        access.add_write::<T>();
    }

    // `min_data_candidate` keeps the `None` default — never drives.

    fn drive<'s, 'w>(
        state: &'s mut Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Option<Mut<'s, T>>)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::External(di) => {
                // The external driver feeds entities; unlike `iter`, the
                // snapshot is unused here (the join's required element drives).
                let (_entities, storage) = state;
                let view = storage.as_mut().map(|refmut| {
                    let (dense, changed, sparse, ents, tick) = refmut.split_for_join();
                    DenseMut::new(dense, changed, sparse, ents, tick)
                });
                Box::new(counted!(di).map(move |e| {
                    // SAFETY: the external driver lists each entity once, so
                    // `get` runs at most once per entity across `'s`.
                    (e, unsafe { view.as_ref().and_then(|v| v.get(e)) })
                }))
            }
            DriveSource::Data(_) => <Self as QueryData>::iter(state),
        }
    }
}

// -------- Entity (the id itself, as data) --------

/// Yields the [`Entity`] id of every live entity — `Query<Entity>`.
///
/// # Logic
///
/// `Entity` names no component, so it has no storage to walk and cannot
/// *drive* a join. Standing alone it must enumerate the whole live set,
/// which [`init_state`](QueryData::init_state) captures as a
/// `World::live_entities` snapshot. [`iter`](QueryData::iter) maps each
/// snapshotted id `e` to the pair `(e, e)`: the first is the entity the
/// trait threads internally (the filter consumes it, [`Query::iter`]
/// strips it); the second is the id the caller asked for. `Entity` is
/// `Copy` (two `u32`s), so duplicating it is free.
///
/// # Why a snapshot, not a held borrow
///
/// The state is an owned `Vec<Entity>`, not a `Ref<EntityAllocator>`.
/// Holding a live allocator borrow across iteration would panic the
/// instant a co-resident [`Commands::spawn`](crate::Commands::spawn) took
/// `borrow_mut` — so a system could not take `Query<Entity>` and
/// `Commands` together. `World::live_entities` releases the borrow
/// before returning; see its docs.
///
/// # Snapshot semantics (the contract)
///
/// The live set is frozen **once, at query construction**
/// ([`Query::from_world`] / `SystemParam` fetch), and every `iter` call
/// re-walks that same `Vec`. For a system mutating the entity set *while
/// iterating* via `Commands`:
///
/// - An entity **spawned** during iteration is **not** yielded.
///   `Commands::spawn` allocates the id synchronously, so it *is* alive —
///   but it postdates the snapshot. A query constructed *after* the spawn
///   would include it.
/// - An entity **despawned** during iteration **is still yielded**.
///   `Commands::despawn` is deferred to the next flush, so the entity
///   stays alive and in the snapshot for the rest of this frame.
///
/// Both reduce to one rule — the yielded set is whatever was live when the
/// query was built — which is what keeps iteration stable against
/// concurrent structural edits.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Query, World};
///
/// #[derive(Component)]
/// struct Tile;
///
/// let mut world = World::new();
/// let a = world.spawn().id();              // no components at all
/// let b = world.spawn().insert(Tile).id();
///
/// let ids: Vec<_> = Query::<spark_ecs::Entity>::from_world(&world).iter().collect();
/// assert_eq!(ids.len(), 2);                // every live entity, components or not
/// assert!(ids.contains(&a) && ids.contains(&b));
/// ```
impl QueryData for Entity {
    type Item<'w>
        = Entity
    where
        Self: 'w;
    // Owned snapshot — not a borrow guard. `'w` is unused (legal for a GAT:
    // the `where Self: 'w` bound is vacuous since `Entity: 'static`).
    type State<'w>
        = Vec<Entity>
    where
        Self: 'w;

    fn init_state<'w>(world: &'w World) -> Self::State<'w>
    where
        Self: 'w,
    {
        world.live_entities()
    }

    fn iter<'s, 'w>(
        state: &'s mut Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Entity)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        Box::new(counted!(state.iter()).map(|&e| (e, e)))
    }

    fn collect_access(_access: &mut QueryAccess) {
        // Entity reads no component — invisible to the self-conflict
        // check, so `Query<(Entity, &mut A, &A)>` still panics on `A`.
    }

    // `Entity` names no component, so it keeps the `None` default for
    // `min_data_candidate` — it offers no candidate to drive off. But it
    // *can* be driven externally: when a filter wins, `drive` yields the
    // filter's entities, not the live snapshot.
    fn drive<'s, 'w>(
        state: &'s mut Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Entity)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::External(di) => Box::new(counted!(di).map(|e| (e, e))),
            // Unreachable in practice: `Entity` returns `None` from
            // `min_data_candidate`, so the plan never picks `Data` for it.
            // Falls back to the snapshot drive as a defensive default.
            DriveSource::Data(_) => Self::iter(state),
        }
    }
}

impl ReadOnlyQueryData for Entity {
    fn iter_ref<'s, 'w>(
        state: &'s Self::State<'w>,
    ) -> Box<dyn Iterator<Item = (Entity, Entity)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        Box::new(counted!(state.iter()).map(|&e| (e, e)))
    }

    fn lookup<'s, 'w>(_state: &'s Self::State<'w>, entity: Entity) -> Option<Entity>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        // Every entity trivially "matches" the `Entity` shape and its item
        // is its own id. (`lookup` has no caller today — it exists for the
        // read side of a tuple join, where `Entity` is never a non-driver.)
        Some(entity)
    }

    fn drive_ref<'s, 'w>(
        state: &'s Self::State<'w>,
        driver: DriveSource<'s>,
    ) -> Box<dyn Iterator<Item = (Entity, Entity)> + 's>
    where
        Self: 's,
        Self: 'w,
        'w: 's,
    {
        match driver {
            DriveSource::External(di) => Box::new(counted!(di).map(|e| (e, e))),
            // Unreachable in practice (no candidate ⇒ never `Data`); defensive.
            DriveSource::Data(_) => Self::iter_ref(state),
        }
    }
}
