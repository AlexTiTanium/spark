//! The public [`Query`] type — the system parameter that walks entities
//! matching a data shape `D` under an optional filter `F` — plus its
//! `from_world` constructor (which runs the self-conflict check and freezes
//! the driver plan), `iter` / `iter_mut`, the [`SystemParam`] impl, and the
//! `IntoIterator` sugar.
//!
//! Sits on top of the rest of the `query` module: the [`QueryData`] /
//! [`ReadOnlyQueryData`] traits ([`data`](super::data)), the generated tuple
//! impls ([`tuple_codegen`](super::tuple_codegen)), and the driver runtime
//! ([`DriveSource`](super::DriveSource) / [`DriverIter`](super::DriverIter) /
//! `DriverPlan`) in the parent module. The additive random-access APIs
//! [`Query::get`] / [`Query::get_mut`] (#72) and the read-only batched
//! [`Query::get_many`] (#73) ship from this file too. A mutable
//! `get_many_mut` was considered for #73 and **not** pursued: handing out
//! `N` simultaneous `&mut` by id needs a per-shape [`DenseMut`]-style
//! batched lookup (the existing single-entity `lookup_mut` re-derives
//! `&mut [T]` per call, so calling it `N` times aliases under Stacked
//! Borrows), and the lone read-only call site — the power-grid solver
//! fetching an edge's two endpoint nodes — does not justify that machinery.
//! Revisit if a write-batch call site appears. [`Query::single`] (#71) is
//! not yet landed.
//!
//! [`DenseMut`]: super::dense_mut::DenseMut

use crate::access::{Access, QueryAccess};
use crate::entity::Entity;
use crate::filter::QueryFilter;
use crate::system::SystemParam;
use crate::world::World;

use super::{DriveSource, DriverIter, DriverPlan, QueryData, ReadOnlyQueryData};

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
/// # Random-access fetch
///
/// Beside iteration, every [`Query`] supports one-entity lookup via
/// [`get`](Self::get) (shared, gated on [`ReadOnlyQueryData`] just like
/// [`iter`](Self::iter)) and [`get_mut`](Self::get_mut) (any [`QueryData`],
/// `&mut self`), plus the batched read-only [`get_many`](Self::get_many).
/// All return `None` for any missing required component, a despawned /
/// stale / never-allocated id, or a filter rejection — the same pattern
/// iteration would have skipped. They share every contract with
/// iteration: optional elements still yield a `None` *value*, and
/// [`Mut<T>`](crate::Mut) still marks change only on a `DerefMut` write.
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

/// Selects the entity stream for a filter-driven query and wraps it in a
/// [`DriverIter`]: a borrowed slice into a single-storage candidate
/// (`With`/`Changed`/`Added`/`And`), or into the `Or` union *already*
/// materialized at `from_world` and passed here as `materialized`. The
/// expensive union build happens at construction (issue #65), not here — this
/// only picks the right pre-built slice. Only reached when
/// [`QueryFilter::candidate_len`] was `Some`, so one branch always applies.
fn select_filter_driver<'s, F: QueryFilter>(
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
        // and `materialized_driver` (shared) are disjoint fields, so the driver
        // and the per-entity `matches` closure can borrow them side by side.
        let filter_state = &self.filter_state;
        let materialized = self.materialized_driver.as_deref();
        let driven = match self.plan {
            DriverPlan::LiveSet => D::iter(&mut self.state),
            DriverPlan::Data(idx) => D::drive(&mut self.state, DriveSource::Data(idx)),
            DriverPlan::Filter => {
                let driver = select_filter_driver::<F>(filter_state, materialized);
                D::drive(&mut self.state, DriveSource::External(driver))
            }
        };
        // Path B — strip the entity that the trait threads internally for
        // join logic; the filter consumes it first. See module docs.
        driven
            .filter(move |(entity, _)| F::matches(*entity, filter_state))
            .map(|(_entity, item)| item)
    }

    /// Fetches one specific entity through this query, mutably.
    ///
    /// The random-access counterpart of [`iter_mut`](Self::iter_mut). Returns
    /// `Some` when `entity` has the full data shape **and** passes the filter
    /// `F`; `None` if any required component is missing, the entity is
    /// despawned / stale / never-allocated, or the filter rejects it. The same
    /// snapshot rules iteration follows apply: filter state and (where it
    /// matters) the live-set snapshot are frozen at
    /// [`from_world`](Self::from_world).
    ///
    /// Available for every `D: QueryData`, including `&mut T` and tuples that
    /// contain a `&mut T`. The `&mut self` is the borrow checker's guarantee
    /// that the returned `Mut<'_, T>` (for any mutable element) is the only
    /// one outstanding at a time — the same way `iter_mut`'s `&mut self`
    /// protects per-row exclusivity.
    ///
    /// # Examples
    ///
    /// Fetch by id, observe `None` for every reason `get_mut` rejects an entity:
    ///
    /// ```
    /// use spark_ecs::{Component, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let a = world.spawn().insert(Health(100)).id();
    /// let b = world.spawn().insert(Health(50)).id();
    /// let c = world.spawn().id();                // alive, no Health
    /// let doomed = world.spawn().insert(Health(99)).id(); // value is incidental
    /// world.despawn(doomed);                     // stale handle from here on
    ///
    /// let mut q = Query::<&mut Health>::from_world(&world);
    /// q.get_mut(a).unwrap().0 -= 10;             // mutate through the handle
    /// assert_eq!(q.get_mut(b).unwrap().0, 50);
    /// assert!(q.get_mut(c).is_none());           // missing component → None
    /// assert!(q.get_mut(doomed).is_none());      // despawned          → None
    /// drop(q);
    ///
    /// // The write through `a` persisted past the query's borrow.
    /// assert_eq!(Query::<&Health>::from_world(&world).get(a).unwrap().0, 90);
    /// ```
    pub fn get_mut(&mut self, entity: Entity) -> Option<D::Item<'_>> {
        // Filter first — match `iter_mut`'s contract that an entity rejected
        // by `F` is invisible to the system. Reading `filter_state` borrows
        // `self` immutably; the subsequent `D::lookup_mut` takes `&mut state`
        // through a disjoint field, so the two borrows coexist.
        if !F::matches(entity, &self.filter_state) {
            return None;
        }
        D::lookup_mut(&mut self.state, entity)
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
        // shared, so the driver slice and the `matches` closure coexist freely.
        let filter_state = &self.filter_state;
        let materialized = self.materialized_driver.as_deref();
        let driven = match self.plan {
            DriverPlan::LiveSet => D::iter_ref(&self.state),
            DriverPlan::Data(idx) => D::drive_ref(&self.state, DriveSource::Data(idx)),
            DriverPlan::Filter => {
                let driver = select_filter_driver::<F>(filter_state, materialized);
                D::drive_ref(&self.state, DriveSource::External(driver))
            }
        };
        driven
            .filter(move |(entity, _)| F::matches(*entity, filter_state))
            .map(|(_entity, item)| item)
    }

    /// Fetches one specific entity through this query, shared.
    ///
    /// The random-access counterpart of [`iter`](Self::iter), gated on
    /// [`ReadOnlyQueryData`] for the same reason `iter` is: a `&mut T` shape
    /// must go through [`get_mut`](Self::get_mut) so the borrow checker can
    /// guard the returned `Mut<'_, T>`. Returns `Some` when `entity` has the
    /// full data shape **and** passes the filter `F`; `None` otherwise — any
    /// missing required component, a despawned / stale / never-allocated id,
    /// or a filter rejection collapses to `None`.
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
    /// let movable = world
    ///     .spawn()
    ///     .insert(Position { x: 1.0, y: 2.0 })
    ///     .insert(Velocity { x: 0.5, y: 0.0 })
    ///     .id();
    /// let stationary = world.spawn().insert(Position { x: 9.0, y: 9.0 }).id();
    /// let doomed = world.spawn().insert(Position { x: 0.0, y: 0.0 }).id();
    /// world.despawn(doomed); // stale handle from here on
    ///
    /// let q = Query::<(&Position, &Velocity)>::from_world(&world);
    /// // The joined entity returns both components.
    /// let (pos, vel) = q.get(movable).unwrap();
    /// assert_eq!(pos.x + vel.x, 1.5);
    /// // The Position-only entity is missing the Velocity half — `None`.
    /// assert!(q.get(stationary).is_none());
    /// // Despawned handles also collapse to `None`, never a stale read.
    /// assert!(q.get(doomed).is_none());
    /// ```
    pub fn get(&self, entity: Entity) -> Option<D::Item<'_>> {
        if !F::matches(entity, &self.filter_state) {
            return None;
        }
        D::lookup(&self.state, entity)
    }

    /// Fetches several entities by id in one call, shared — the batched
    /// counterpart of [`get`](Self::get).
    ///
    /// Returns a fixed-size array the same length as `ids`, one slot per id
    /// in order. Each slot follows [`get`](Self::get)'s rule exactly: `Some`
    /// when that entity has the full data shape **and** passes the filter `F`;
    /// `None` for any missing required component, a despawned / stale /
    /// never-allocated id, or a filter rejection. The batch is just `N`
    /// independent `get`s — there is no cross-slot interaction, so a repeated
    /// id is harmless (each lookup is a shared `&` borrow; both slots read the
    /// same data back).
    ///
    /// Gated on [`ReadOnlyQueryData`] for the same reason [`get`](Self::get)
    /// and [`iter`](Self::iter) are. A `&mut T` shape has no batched form:
    /// `get_many_mut` would hand out several `Mut<'_, T>` from one call, which
    /// cannot ride a shared `&self`, and the sound `&mut` machinery it needs
    /// was deliberately not built (see the module-level docs).
    ///
    /// # Examples
    ///
    /// A power-grid edge fetching both endpoint nodes in one shot — the
    /// motivating call site — instead of two [`get`](Self::get)s and two
    /// `Option` unwraps:
    ///
    /// ```
    /// use spark_ecs::{Component, Query, World};
    ///
    /// #[derive(Component, PartialEq, Debug)]
    /// struct Node { load: f32 }
    ///
    /// let mut world = World::new();
    /// let from = world.spawn().insert(Node { load: 3.0 }).id();
    /// let to = world.spawn().insert(Node { load: 5.0 }).id();
    /// let ghost = world.spawn().id(); // alive, but no Node
    ///
    /// let q = Query::<&Node>::from_world(&world);
    /// let [a, b, c] = q.get_many([from, to, ghost]);
    /// assert_eq!(a, Some(&Node { load: 3.0 }));
    /// assert_eq!(b, Some(&Node { load: 5.0 }));
    /// assert!(c.is_none()); // missing component → None, exactly like `get`
    /// ```
    ///
    /// Like [`get`](Self::get) it is gated on [`ReadOnlyQueryData`], so a
    /// `&mut` shape cannot call it — a **compile-time** error, not a runtime
    /// one:
    ///
    /// ```compile_fail
    /// use spark_ecs::{Component, Query, World};
    ///
    /// // `Position` is a component, so `insert` compiles and the failure
    /// // lands on `get_many` — the `ReadOnlyQueryData` bound this example
    /// // is about.
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Position(0.0, 0.0)).id();
    /// let q = Query::<&mut Position>::from_world(&world);
    /// // error[E0599]: the method `get_many` exists for `Query<&mut Position>`,
    /// // but its trait bounds were not satisfied: `&mut Position:
    /// // ReadOnlyQueryData`.
    /// let _ = q.get_many([e]);
    /// ```
    pub fn get_many<const N: usize>(&self, ids: [Entity; N]) -> [Option<D::Item<'_>>; N] {
        // `N` independent `get`s — `Entity` is `Copy`, so `array::map` consumes
        // the id array by value. Every slot is a shared `&self.state` borrow, so
        // all `N` items coexist with no aliasing concern (the reason the `&mut`
        // batch is a separate, unbuilt design).
        ids.map(|entity| {
            if !F::matches(entity, &self.filter_state) {
                return None;
            }
            D::lookup(&self.state, entity)
        })
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
