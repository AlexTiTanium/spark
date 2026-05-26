//! The [`World`] — the single container for engine state.
//!
//! Resources (singletons), entities (generational handles), and
//! components (per-type sparse-set storages) all live here. Resources
//! get [`add_resource`](World::add_resource) /
//! [`resource`](World::resource); entities and components get
//! [`spawn`](World::spawn) / [`despawn`](World::despawn) /
//! [`insert`](World::insert) / [`remove`](World::remove) /
//! [`get`](World::get) / [`get_mut`](World::get_mut).
//!
//! Each map slot is wrapped in a [`RefCell`] so accessors take `&self`
//! — two `ResMut<T>` (or `&mut Position`) over *different* `T` can
//! coexist in one system without either holding `&mut World`. Single
//! threaded today; the `Send + Sync` bound the parallel scheduler will
//! need is enforced at the derive site by `#[derive(Component)]` (the
//! `Resource` derive stays `'static`-only — see the type docs).

use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;

use crate::access::Access;
use crate::commands::CommandQueue;
use crate::entity::{Entity, EntityAllocator};
use crate::storage::{AnyStorage, ComponentStorage};
use crate::{Component, Resource};

/// Type-erased container that owns the engine's long-lived state.
///
/// Three logical regions:
/// - `resources` — one value per type, the canonical home for
///   long-lived singletons (wgpu device, time, input state).
/// - `entities` — a generational allocator handing out [`Entity`]
///   handles.
/// - `components` — one [`ComponentStorage<T>`] per component type,
///   each behind its own [`RefCell`] so `&self` lookups can succeed.
///
/// # The trait bounds the two maps enforce
///
/// `insert` / `get` / `storage` take `T: Component`
/// (`Send + Sync + 'static`); `add_resource` / `resource` take
/// `T: Resource` (`'static`). Both bounds come from explicit derives, so
/// a type lands in exactly one region — a [`Component`] can't be added
/// as a resource, a [`Resource`] can't be inserted onto an entity.
///
/// The asymmetry is deliberate. Components are the parallel-iteration
/// surface, so they must be `Send + Sync`: the M4 scheduler hands their
/// storages to worker threads and leans on that bound as its safety
/// proof. Resources hold the engine's non-thread-safe singletons (a
/// `wgpu` surface, an OS handle), so they stay `'static`-only;
/// parallel-safety for a resource is the scheduler's job — keep the
/// system that touches it on the main thread — not the type system's.
/// `World` itself is `!Send + !Sync` by construction, matching the
/// single-threaded scheduler that ships today.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, Resource, World};
///
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
/// #[derive(Resource)]
/// struct GameTime { dt: f32 }
///
/// let mut world = World::new();
/// world.add_resource(GameTime { dt: 0.016 });
///
/// let player = world.spawn()
///     .insert(Position { x: 0.0, y: 0.0 })
///     .insert(Velocity { x: 1.0, y: 0.5 })
///     .id();
///
/// assert!(world.is_alive(player));
/// assert_eq!(world.get::<Position>(player).unwrap().x, 0.0);
/// ```
#[derive(Default)]
pub struct World {
    // Wrapped in `RefCell` so [`crate::Commands::spawn`] can
    // `borrow_mut` the allocator through a shared `&World` borrow —
    // the same trick we already use per-component-storage and
    // per-resource. Through `&mut World` we still reach the underlying
    // allocator with `get_mut()` (no runtime borrow check).
    entities: RefCell<EntityAllocator>,
    components: HashMap<TypeId, RefCell<Box<dyn AnyStorage>>>,
    resources: HashMap<TypeId, RefCell<Box<dyn Any>>>,
    // Deferred ops queued by [`crate::Commands`] within a stage and
    // drained by [`flush_commands`](Self::flush_commands) at the stage
    // boundary. One cell per `World`; the queue itself is FIFO.
    pending: RefCell<CommandQueue>,
    // The change-detection baseline for the *currently running* system:
    // one `(TypeId, tick)` per component it accesses, the tick that
    // component's clock read when the system last ran. Parked here by
    // [`run_system`](Self::run_system) (O(1) `mem::swap`) so the static
    // [`QueryFilter::matches`](crate::QueryFilter::matches), which sees only
    // `&World`, can read it. Each component carries its own clock, so the
    // baseline is per-component, not a single number. Empty between runs.
    current_baselines: Vec<(TypeId, u32)>,
}

impl World {
    /// Creates an empty `World` — no entities, no components, no
    /// resources.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::World;
    ///
    /// let _world = World::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // -------- resources --------

    /// Inserts a resource. A second insert of the same type silently
    /// overwrites the first.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct FrameRate(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(FrameRate(60));
    /// world.add_resource(FrameRate(120)); // replaces the previous value
    /// ```
    pub fn add_resource<T: Resource>(&mut self, value: T) {
        self.resources
            .insert(TypeId::of::<T>(), RefCell::new(Box::new(value)));
    }

    /// Returns a shared borrow of the resource of type `T`, or `None` if
    /// no such resource has been inserted.
    ///
    /// The returned [`Ref`] holds a runtime borrow on the underlying
    /// cell; while it is live, [`World::get_resource_mut`] /
    /// [`World::resource_mut`] for the same `T` will panic.
    ///
    /// # Panics
    ///
    /// Only on internal invariant violation — the stored value's type
    /// must agree with the `TypeId` key it lives under. This can only
    /// fail if a future change introduces a way to overwrite a slot
    /// with a value of a different type without rebuilding the key.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// assert!(world.get_resource::<Score>().is_none());
    /// world.add_resource(Score(7));
    /// assert_eq!(world.get_resource::<Score>().unwrap().0, 7);
    /// ```
    #[must_use]
    pub fn get_resource<T: Resource>(&self) -> Option<Ref<'_, T>> {
        let cell = self.resources.get(&TypeId::of::<T>())?;
        Some(Ref::map(cell.borrow(), |b| {
            b.downcast_ref::<T>()
                .expect("TypeId key and stored value must agree")
        }))
    }

    /// Returns an exclusive borrow of the resource of type `T`, or
    /// `None` if no such resource has been inserted.
    ///
    /// The returned [`RefMut`] holds a runtime exclusive borrow on the
    /// underlying cell; any other concurrent borrow of the same `T`
    /// (shared or exclusive) panics until the guard is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the resource is already borrowed (the [`RefCell`]
    /// uniqueness check). Only on internal invariant violation if the
    /// stored value's type disagrees with its `TypeId` key.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(0));
    /// world.get_resource_mut::<Score>().unwrap().0 = 1;
    /// assert_eq!(world.get_resource::<Score>().unwrap().0, 1);
    /// ```
    #[must_use]
    pub fn get_resource_mut<T: Resource>(&self) -> Option<RefMut<'_, T>> {
        let cell = self.resources.get(&TypeId::of::<T>())?;
        Some(RefMut::map(cell.borrow_mut(), |b| {
            b.downcast_mut::<T>()
                .expect("TypeId key and stored value must agree")
        }))
    }

    /// Returns a shared borrow of the resource of type `T`. Panics if
    /// the resource is missing.
    ///
    /// Sharper-edged sibling of [`World::get_resource`] for the common
    /// case where the caller has already inserted the resource and
    /// treating absence as a bug is more useful than an `Option`.
    ///
    /// # Panics
    ///
    /// Panics with a message containing
    /// [`std::any::type_name::<T>()`](std::any::type_name) when no
    /// resource of type `T` has been inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(9));
    /// assert_eq!(world.resource::<Score>().0, 9);
    /// ```
    #[must_use]
    pub fn resource<T: Resource>(&self) -> Ref<'_, T> {
        self.get_resource::<T>().unwrap_or_else(|| {
            panic!(
                "resource of type `{}` has not been inserted",
                std::any::type_name::<T>()
            )
        })
    }

    /// Returns an exclusive borrow of the resource of type `T`. Panics
    /// if the resource is missing or already borrowed.
    ///
    /// # Panics
    ///
    /// - Panics with a message containing
    ///   [`std::any::type_name::<T>()`](std::any::type_name) when no
    ///   resource of type `T` has been inserted.
    /// - Panics if the resource is already borrowed (the [`RefCell`]
    ///   uniqueness check). Two `ResMut<T>` over the same `T` inside
    ///   one system trip this — the M4 scheduler will catch it at
    ///   registration time.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(0));
    /// world.resource_mut::<Score>().0 = 100;
    /// assert_eq!(world.resource::<Score>().0, 100);
    /// ```
    #[must_use]
    pub fn resource_mut<T: Resource>(&self) -> RefMut<'_, T> {
        self.get_resource_mut::<T>().unwrap_or_else(|| {
            panic!(
                "resource of type `{}` has not been inserted",
                std::any::type_name::<T>()
            )
        })
    }

    // -------- entities & components --------

    /// Allocates a fresh [`Entity`] and returns an [`EntityMut`]
    /// builder that lets you chain `.insert(component)` calls.
    ///
    /// The entity exists in the world the moment `spawn` returns,
    /// whether or not `.id()` is called.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    /// #[derive(Component)]
    /// struct Velocity(f32, f32);
    ///
    /// let mut world = World::new();
    /// let entity = world.spawn()
    ///     .insert(Position(0.0, 0.0))
    ///     .insert(Velocity(1.0, 0.0))
    ///     .id();
    /// assert!(world.is_alive(entity));
    /// ```
    pub fn spawn(&mut self) -> EntityMut<'_> {
        // `&mut self` lets us reach the allocator without a runtime
        // borrow check via `get_mut()`.
        let entity = self.entities.get_mut().allocate();
        EntityMut {
            world: self,
            entity,
        }
    }

    /// Destroys `entity` and removes every one of its components from
    /// every registered storage. Returns `true` if the handle was
    /// live, `false` for stale or never-allocated handles.
    ///
    /// Internally walks every `Box<dyn AnyStorage>` and calls
    /// `remove_entity` — that's why despawn cost is O(K) in the
    /// number of *component types* (not the number of components on
    /// this entity).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    /// #[derive(Component)]
    /// struct Velocity(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn()
    ///     .insert(Position(0.0, 0.0))
    ///     .insert(Velocity(1.0, 0.0))
    ///     .id();
    ///
    /// assert!(world.despawn(e));
    /// assert!(!world.is_alive(e));
    /// assert!(world.get::<Position>(e).is_none());
    /// assert!(world.get::<Velocity>(e).is_none());
    /// assert!(!world.despawn(e));      // second despawn is a no-op
    /// ```
    pub fn despawn(&mut self, entity: Entity) -> bool {
        if !self.entities.get_mut().is_alive(entity) {
            return false;
        }
        for cell in self.components.values_mut() {
            cell.get_mut().remove_entity(entity);
        }
        self.entities.get_mut().destroy(entity)
    }

    /// Returns `true` iff this exact handle (matching index *and*
    /// generation) names a live entity.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::World;
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().id();
    /// assert!(world.is_alive(e));
    /// world.despawn(e);
    /// assert!(!world.is_alive(e));
    /// ```
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        self.entities.borrow().is_alive(entity)
    }

    /// Attaches `value` to `entity`. Returns the previous component
    /// of type `T` if there was one, else `None`. No-op (returning
    /// `None`) when `entity` is stale or never-allocated.
    ///
    /// The component storage for `T` is lazily created on the first
    /// `insert::<T>` against this world.
    ///
    /// # Panics
    ///
    /// Panics on internal invariant violation if a storage stored
    /// under `TypeId::of::<T>()` is not a `ComponentStorage<T>` — only
    /// possible if a future change subverts the type-keyed map.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().id();
    /// assert!(world.insert(e, Position(1.0, 2.0)).is_none());
    /// let old = world.insert(e, Position(3.0, 4.0)).unwrap();
    /// assert!((old.0 - 1.0).abs() < f32::EPSILON);
    /// ```
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> Option<T> {
        if !self.entities.get_mut().is_alive(entity) {
            return None;
        }
        let cell = self
            .components
            .entry(TypeId::of::<T>())
            .or_insert_with(|| RefCell::new(Box::new(ComponentStorage::<T>::new())));
        cell.get_mut()
            .as_any_mut()
            .downcast_mut::<ComponentStorage<T>>()
            .expect("TypeId key and stored storage type must agree")
            .insert(entity, value)
    }

    /// Detaches and returns `entity`'s component of type `T`. Returns
    /// `None` when the entity has no `T`, or holds a stale handle.
    ///
    /// # Panics
    ///
    /// Panics on internal invariant violation if a storage stored
    /// under `TypeId::of::<T>()` is not a `ComponentStorage<T>` — only
    /// possible if a future change subverts the type-keyed map.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Position(1.0, 2.0)).id();
    /// let pos = world.remove::<Position>(e).unwrap();
    /// assert!((pos.0 - 1.0).abs() < f32::EPSILON);
    /// assert!(world.get::<Position>(e).is_none());
    /// ```
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.entities.get_mut().is_alive(entity) {
            return None;
        }
        let cell = self.components.get_mut(&TypeId::of::<T>())?;
        cell.get_mut()
            .as_any_mut()
            .downcast_mut::<ComponentStorage<T>>()
            .expect("TypeId key and stored storage type must agree")
            .remove(entity)
    }

    /// Returns a shared borrow of `entity`'s component of type `T`, or
    /// `None` if absent (or `entity` is stale).
    ///
    /// The returned [`Ref`] holds a runtime borrow on the storage's
    /// cell — while it's live, [`get_mut`](Self::get_mut) on the same
    /// component type panics.
    ///
    /// # Panics
    ///
    /// Panics on internal invariant violation if the storage's
    /// `TypeId` key disagrees with its stored type.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Health(100)).id();
    /// assert_eq!(world.get::<Health>(e).unwrap().0, 100);
    /// ```
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<Ref<'_, T>> {
        if !self.entities.borrow().is_alive(entity) {
            return None;
        }
        let cell = self.components.get(&TypeId::of::<T>())?;
        Ref::filter_map(cell.borrow(), |b| {
            b.as_any()
                .downcast_ref::<ComponentStorage<T>>()
                .expect("TypeId key and stored storage type must agree")
                .get(entity)
        })
        .ok()
    }

    /// Returns an exclusive borrow of `entity`'s component of type
    /// `T`, or `None` if absent (or `entity` is stale).
    ///
    /// This is the ad-hoc single-entity mutate path; it advances `T`'s
    /// change-detection clock and marks the component changed. The
    /// systems/`Query<&mut T>` path is the normal one — reach for this
    /// from setup or tests. (Two `get_mut`s of the same `T` back-to-back
    /// before any system runs each advance the clock, so a reader
    /// observes both as changed on its next run, then nothing after.)
    ///
    /// # Panics
    ///
    /// Panics if any other borrow of this storage is live (the
    /// [`RefCell`] uniqueness check), or on internal invariant
    /// violation if the storage's `TypeId` key disagrees with its
    /// stored type.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Health(100)).id();
    /// world.get_mut::<Health>(e).unwrap().0 -= 25;
    /// assert_eq!(world.get::<Health>(e).unwrap().0, 75);
    /// ```
    #[must_use]
    pub fn get_mut<T: Component>(&self, entity: Entity) -> Option<RefMut<'_, T>> {
        if !self.entities.borrow().is_alive(entity) {
            return None;
        }
        let cell = self.components.get(&TypeId::of::<T>())?;
        RefMut::filter_map(cell.borrow_mut(), |b| {
            let storage = b
                .as_any_mut()
                .downcast_mut::<ComponentStorage<T>>()
                .expect("TypeId key and stored storage type must agree");
            // A direct mutable borrow is its own change event: advance the
            // component's clock so the stamp below lands past any prior
            // observation (the scheduler can't advance for an ad-hoc
            // `get_mut` outside a declared-write system). `get_mut` stamps.
            storage.advance_tick();
            storage.get_mut(entity)
        })
        .ok()
    }

    // -------- deferred command queue --------

    /// Drains every queued [`crate::Commands`] op into this world, in
    /// push order.
    ///
    /// Called by
    /// [`Application::run_stage`](../../spark_core/struct.Application.html#method.run_stage)
    /// after a stage's sequential systems, and by
    /// [`Schedule::run`](crate::Schedule::run) at every workload boundary —
    /// so deferred ops apply before the next group of systems that should
    /// see them. Ops that enqueue more ops (e.g. a closure that constructs
    /// a fresh [`crate::Commands`] mid-flush) are picked up by the
    /// internal loop and applied in the same call.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, IntoSystem, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Tag;
    ///
    /// let mut world = World::new();
    /// let mut sys = IntoSystem::into_system(|mut commands: Commands| {
    ///     commands.spawn().insert(Tag);
    /// });
    /// sys(&world);
    /// assert_eq!(Query::<&Tag>::from_world(&world).iter().count(), 0);
    /// world.flush_commands();
    /// assert_eq!(Query::<&Tag>::from_world(&world).iter().count(), 1);
    /// ```
    pub fn flush_commands(&mut self) {
        loop {
            // Snatch the queue's contents — `self.pending` is now a
            // fresh empty queue, so any op that re-enqueues during the
            // inner flush lands there and we catch it on the next
            // iteration.
            let mut queue = std::mem::take(self.pending.get_mut());
            if queue.is_empty() {
                return;
            }
            queue.flush(self);
        }
    }

    // -------- internal cell handles (Commands plumbing) --------
    //
    // Crate-private — `Commands` is the only sanctioned caller. The
    // borrowed cells are disjoint from every component storage, which
    // is why a system can take `Commands` and `Query<&mut T>` for any
    // `T` in the same signature without panicking at runtime.

    /// Returns a reference to the [`RefCell`] guarding the
    /// [`EntityAllocator`]. Used by [`crate::Commands::spawn`] to
    /// allocate synchronously without `&mut World`.
    #[must_use]
    pub(crate) fn entities_cell(&self) -> &RefCell<EntityAllocator> {
        &self.entities
    }

    /// Returns a reference to the [`RefCell`] guarding the pending
    /// [`CommandQueue`]. Used by [`crate::Commands`] to enqueue
    /// deferred ops through a shared world borrow.
    #[must_use]
    pub(crate) fn pending_cell(&self) -> &RefCell<CommandQueue> {
        &self.pending
    }

    // -------- storage handles (Query plumbing) --------
    //
    // Crate-private on purpose: queries are the only sanctioned way for
    // engine code to walk a whole `ComponentStorage<T>`. These two
    // accessors are the query-iteration chokepoint for the M4
    // `RefCell → UnsafeCell` swap; the per-entity accessors above
    // (`get`, `get_mut`, `insert`, `remove`, `despawn`) touch the same
    // cells through different paths and migrate alongside.

    /// Returns a shared borrow of the storage for `T`, or `None` if no
    /// entity has ever held a `T` (no insert has lazily created the
    /// cell yet).
    ///
    /// The returned [`Ref`] holds a runtime shared borrow on the
    /// underlying cell; while it is live, [`storage_mut`](Self::storage_mut)
    /// for the same `T` panics.
    ///
    /// # Panics
    ///
    /// Panics only on internal invariant violation — the storage stored
    /// under `TypeId::of::<T>()` must be a `ComponentStorage<T>`. This
    /// can only fail if a future change subverts the type-keyed map.
    #[must_use]
    pub(crate) fn storage<T: Component>(&self) -> Option<Ref<'_, ComponentStorage<T>>> {
        let cell = self.components.get(&TypeId::of::<T>())?;
        Some(Ref::map(cell.borrow(), |b| {
            b.as_any()
                .downcast_ref::<ComponentStorage<T>>()
                .expect("TypeId key and stored storage type must agree")
        }))
    }

    /// Returns an exclusive borrow of the storage for `T`, or `None` if
    /// no entity has ever held a `T`.
    ///
    /// # Panics
    ///
    /// - Panics if the storage is already borrowed (shared or
    ///   exclusive). That is the [`RefCell`] rule that lets two
    ///   `Query<&mut T>` over the same `T` in one system surface as a
    ///   runtime panic until the M4 scheduler catches the conflict at
    ///   registration time.
    /// - Panics on internal invariant violation if the storage stored
    ///   under `TypeId::of::<T>()` is not a `ComponentStorage<T>`.
    #[must_use]
    pub(crate) fn storage_mut<T: Component>(&self) -> Option<RefMut<'_, ComponentStorage<T>>> {
        let cell = self.components.get(&TypeId::of::<T>())?;
        Some(RefMut::map(cell.borrow_mut(), |b| {
            b.as_any_mut()
                .downcast_mut::<ComponentStorage<T>>()
                .expect("TypeId key and stored storage type must agree")
        }))
    }

    // -------- per-component change detection --------

    /// Runs one system, driving the per-component change-detection dance,
    /// and updates `last_seen` (the system's per-component baselines) in
    /// place.
    ///
    /// Both scheduling paths funnel through here so the dance lives once:
    ///
    /// 1. **Advance** the clock of every component the system *writes*, so
    ///    its in-place edits stamp a tick strictly past any prior look.
    /// 2. **Copy** `last_seen` into the world so the system's `Changed<T>`
    ///    / `Added<T>` filters — which see only `&World` — can read their
    ///    baselines via [`baseline_for`](Self::baseline_for).
    /// 3. **Run** the system.
    /// 4. **Record** each accessed component's current clock into
    ///    `last_seen`, so next run compares against where this one left off.
    ///
    /// `Commands` writes are *not* advanced here (a `Commands` system
    /// declares no specific access); [`ComponentStorage::insert`] advances
    /// each touched component's clock itself at flush time.
    ///
    /// # Caller contract
    ///
    /// `last_seen` is the system's **durable** per-component baseline — the
    /// caller must keep the same `Vec` across runs of the same logical
    /// system (both schedulers hold it in the system's struct). Passing a
    /// fresh `Vec::new()` each call resets every baseline to 0, so every
    /// component that was ever written looks `Changed` on every run.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Access, World};
    ///
    /// let mut world = World::new();
    /// // The scheduler keeps `last_seen` in the system's struct across
    /// // runs; here it touches nothing, so its contents never matter.
    /// let mut last_seen = Vec::new();
    /// let access = Access::new();
    /// world.run_system(&access, &mut last_seen, &mut |_w: &World| {});
    /// ```
    pub fn run_system(
        &mut self,
        access: &Access,
        last_seen: &mut Vec<(TypeId, u32)>,
        run: &mut dyn FnMut(&World),
    ) {
        for tid in access.component_write_ids() {
            self.advance_storage_tick(tid);
        }
        // Copy the system's baselines into the world so the static filter
        // `matches` can read them during the run via `baseline_for`. A
        // *copy* (not a move) is what keeps this panic-safe: if `run`
        // panics, `last_seen` is still intact and `current_baselines` is
        // mere scratch — the next `run_system` clears and refills it
        // before anything reads it.
        self.current_baselines.clear();
        self.current_baselines.extend_from_slice(last_seen);
        run(&*self);
        // Record where each accessed component's clock landed, straight
        // into the caller's durable `last_seen`, so next run sees the delta.
        for tid in access.component_access_ids() {
            let tick = self.storage_tick(tid).unwrap_or(0);
            upsert_baseline(last_seen, tid, tick);
        }
        self.current_baselines.clear();
    }

    /// The change-detection baseline for component `T` in the currently
    /// running system — the tick its clock read when that system last
    /// ran, or `0` ("never observed") if it has no recorded baseline.
    ///
    /// Read by [`Changed<T>`](crate::Changed) / [`Added<T>`](crate::Added);
    /// the scheduler parks the value via [`run_system`](Self::run_system).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::World;
    ///
    /// #[derive(spark_ecs::Component)]
    /// struct Health(u32);
    ///
    /// let world = World::new();
    /// assert_eq!(world.baseline_for::<Health>(), 0); // nothing parked
    /// ```
    #[must_use]
    pub fn baseline_for<T: Component>(&self) -> u32 {
        let tid = TypeId::of::<T>();
        self.current_baselines
            .iter()
            .find(|(id, _)| *id == tid)
            .map_or(0, |(_, tick)| *tick)
    }

    /// Advances the clock of the storage keyed by `tid`, if it exists.
    /// No-op when no entity has ever held that component.
    fn advance_storage_tick(&mut self, tid: TypeId) {
        if let Some(cell) = self.components.get_mut(&tid) {
            cell.get_mut().advance_tick();
        }
    }

    /// The current clock of the storage keyed by `tid`, or `None` if no
    /// such storage exists yet.
    fn storage_tick(&self, tid: TypeId) -> Option<u32> {
        self.components
            .get(&tid)
            .map(|cell| cell.borrow().current_tick())
    }
}

/// Inserts or updates `(tid, tick)` in a small per-system baseline list.
/// Linear scan — the list holds one entry per component a system touches
/// (a handful), so a `Vec` beats a `HashMap` on both speed and footprint.
fn upsert_baseline(baselines: &mut Vec<(TypeId, u32)>, tid: TypeId, tick: u32) {
    if let Some(slot) = baselines.iter_mut().find(|(id, _)| *id == tid) {
        slot.1 = tick;
    } else {
        baselines.push((tid, tick));
    }
}

/// Chainable builder returned by [`World::spawn`].
///
/// Holds a unique borrow of the [`World`] for the duration of the
/// chain; call `.id()` at the end to capture the [`Entity`], or just
/// drop the value when you don't need it.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Component, World};
///
/// #[derive(Component)]
/// struct Tag;
///
/// let mut world = World::new();
/// // Capture the id at the end of the chain.
/// let e = world.spawn().insert(Tag).id();
/// // …or drop the builder when you don't need the id.
/// world.spawn().insert(Tag);
/// # let _ = e;
/// ```
pub struct EntityMut<'w> {
    world: &'w mut World,
    entity: Entity,
}

impl EntityMut<'_> {
    /// Attaches `value` to this entity and returns the builder for
    /// further chaining.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    /// #[derive(Component)]
    /// struct Velocity(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn()
    ///     .insert(Position(0.0, 0.0))
    ///     .insert(Velocity(1.0, 0.0))
    ///     .id();
    /// assert!(world.get::<Position>(e).is_some());
    /// ```
    // The common pattern is `world.spawn().insert(A).insert(B);` —
    // drop the builder, keep the entity. `#[must_use]` would warn on
    // that and force a stray `let _ =` everywhere.
    #[allow(
        clippy::return_self_not_must_use,
        reason = "builder is deliberately discardable mid-chain"
    )]
    pub fn insert<T: Component>(self, value: T) -> Self {
        self.world.insert(self.entity, value);
        self
    }

    /// Returns the [`Entity`] handle this builder names.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::World;
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().id();
    /// assert!(world.is_alive(e));
    /// ```
    #[must_use]
    pub fn id(&self) -> Entity {
        self.entity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Added, Changed, Component, Query, Resource};
    use std::cell::Cell;
    use std::rc::Rc;

    #[derive(Resource)]
    struct A(u32);
    #[derive(Resource)]
    struct B(&'static str);

    // Integer fields — keeps the unit tests free of `clippy::float_cmp`
    // assertions. Doc tests (which clippy doesn't lint) stay with the
    // canonical `f32` flavour to read like real engine code.
    #[derive(Debug, PartialEq, Component)]
    struct Position(i32, i32);

    #[derive(Debug, PartialEq, Component)]
    struct Velocity(i32, i32);

    #[derive(Debug, PartialEq, Component)]
    struct Walkable;

    // -------- resources (regression: pre-existing behaviour) --------

    #[test]
    fn new_world_is_empty() {
        let world = World::new();
        assert!(world.get_resource::<A>().is_none());
    }

    #[test]
    fn add_resource_then_read() {
        let mut world = World::new();
        world.add_resource(A(7));
        assert_eq!(world.get_resource::<A>().unwrap().0, 7);
    }

    #[test]
    fn same_type_overwrites() {
        let mut world = World::new();
        world.add_resource(A(1));
        world.add_resource(A(2));
        assert_eq!(world.resource::<A>().0, 2);
    }

    #[test]
    fn different_types_coexist() {
        let mut world = World::new();
        world.add_resource(A(1));
        world.add_resource(B("hello"));
        assert_eq!(world.resource::<A>().0, 1);
        assert_eq!(world.resource::<B>().0, "hello");
    }

    #[test]
    fn non_send_resource_is_allowed() {
        // `Rc<Cell<_>>` is `!Send + !Sync`. The `Resource` trait carries
        // only a `'static` bound — deliberately *not* `Send + Sync` — so
        // a resource wrapping non-thread-safe state is still storable.
        // The M4 scheduler keeps systems that touch such a resource on
        // the main thread rather than rejecting it at the type level.
        // (Components, by contrast, are `Send + Sync` and a non-`Send`
        // one fails to derive — see the `compile_fail` doctest on the
        // `Component` trait.)
        #[derive(Resource)]
        struct NonSendCounter(Rc<Cell<u32>>);

        let mut world = World::new();
        world.add_resource(NonSendCounter(Rc::new(Cell::new(0))));
        assert_eq!(world.resource::<NonSendCounter>().0.get(), 0);
    }

    #[test]
    fn resource_mut_writes_visible_through_resource() {
        let mut world = World::new();
        world.add_resource(A(1));
        world.resource_mut::<A>().0 = 99;
        assert_eq!(world.resource::<A>().0, 99);
    }

    #[test]
    fn different_types_can_be_borrowed_mut_simultaneously() {
        let mut world = World::new();
        world.add_resource(A(1));
        world.add_resource(B("x"));
        let mut a = world.resource_mut::<A>();
        let mut b = world.resource_mut::<B>();
        a.0 = 2;
        b.0 = "y";
        drop(a);
        drop(b);
        assert_eq!(world.resource::<A>().0, 2);
        assert_eq!(world.resource::<B>().0, "y");
    }

    #[test]
    #[should_panic(expected = "spark_ecs::world::tests::A")]
    fn missing_resource_panics_with_type_name() {
        let world = World::new();
        let _ = world.resource::<A>();
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn same_type_double_mut_borrow_panics() {
        let mut world = World::new();
        world.add_resource(A(1));
        let _a1 = world.resource_mut::<A>();
        let _a2 = world.resource_mut::<A>();
    }

    // -------- entities & components (new in this PR) --------

    #[test]
    fn spawn_creates_a_live_entity() {
        let mut world = World::new();
        let e = world.spawn().id();
        assert!(world.is_alive(e));
    }

    #[test]
    fn spawn_with_chained_inserts() {
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(1, 2))
            .insert(Velocity(3, 4))
            .insert(Walkable)
            .id();
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(1, 2));
        assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(3, 4));
        assert_eq!(*world.get::<Walkable>(e).unwrap(), Walkable);
    }

    #[test]
    fn despawn_cascades_through_every_storage() {
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(0, 0))
            .insert(Velocity(1, 0))
            .id();
        assert!(world.despawn(e));
        assert!(!world.is_alive(e));
        assert!(world.get::<Position>(e).is_none());
        assert!(world.get::<Velocity>(e).is_none());
        assert!(!world.despawn(e));
    }

    #[test]
    fn get_mut_writes_visible_through_get() {
        let mut world = World::new();
        let e = world.spawn().insert(Position(0, 0)).id();
        world.get_mut::<Position>(e).unwrap().0 = 42;
        assert_eq!(world.get::<Position>(e).unwrap().0, 42);
    }

    #[test]
    fn remove_returns_value_and_clears_storage() {
        let mut world = World::new();
        let e = world.spawn().insert(Position(7, 8)).id();
        let pos = world.remove::<Position>(e).unwrap();
        assert_eq!(pos, Position(7, 8));
        assert!(world.get::<Position>(e).is_none());
        assert!(world.remove::<Position>(e).is_none());
    }

    #[test]
    fn insert_twice_returns_previous_value() {
        let mut world = World::new();
        let e = world.spawn().insert(Position(1, 1)).id();
        let old = world.insert(e, Position(2, 2)).unwrap();
        assert_eq!(old, Position(1, 1));
        assert_eq!(world.get::<Position>(e).unwrap().0, 2);
    }

    #[test]
    fn dead_entity_insert_remove_get_are_noops() {
        let mut world = World::new();
        let e = world.spawn().insert(Position(1, 2)).id();
        world.despawn(e);

        assert!(world.insert(e, Velocity(0, 0)).is_none());
        assert!(world.remove::<Position>(e).is_none());
        assert!(world.get::<Position>(e).is_none());
        assert!(world.get_mut::<Position>(e).is_none());
    }

    #[test]
    fn stale_handle_after_slot_reuse_is_rejected() {
        let mut world = World::new();
        let a = world.spawn().insert(Position(1, 1)).id();
        world.despawn(a);
        let b = world.spawn().insert(Position(2, 2)).id();
        assert_eq!(a.index, b.index);
        assert_ne!(a, b);
        assert!(world.get::<Position>(a).is_none());
        assert_eq!(world.get::<Position>(b).unwrap().0, 2);
    }

    #[test]
    fn two_distinct_components_borrowed_mut_disjointly() {
        let mut world = World::new();
        let e = world
            .spawn()
            .insert(Position(0, 0))
            .insert(Velocity(0, 0))
            .id();
        let mut p = world.get_mut::<Position>(e).unwrap();
        let mut v = world.get_mut::<Velocity>(e).unwrap();
        p.0 = 1;
        v.0 = 2;
        drop(p);
        drop(v);
        assert_eq!(world.get::<Position>(e).unwrap().0, 1);
        assert_eq!(world.get::<Velocity>(e).unwrap().0, 2);
    }

    #[test]
    fn resources_and_components_coexist_on_one_world() {
        // Sanity: the new fields don't disturb the resource path.
        let mut world = World::new();
        world.add_resource(A(7));
        let e = world.spawn().insert(Position(0, 0)).id();
        assert_eq!(world.resource::<A>().0, 7);
        assert!(world.is_alive(e));
        assert!(world.get::<Position>(e).is_some());
    }

    // -------- change detection: per-component clocks --------

    #[test]
    fn per_component_clocks_advance_independently() {
        // The signature of this architecture: each component type owns its
        // own clock. Writing Position never moves Velocity's clock.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0)); // Position clock 1→2
        world
            .spawn()
            .insert(Position(1, 1)) // Position 2→3
            .insert(Velocity(1, 1)); // Velocity 1→2
        assert_eq!(world.storage::<Position>().unwrap().current_tick(), 3);
        assert_eq!(world.storage::<Velocity>().unwrap().current_tick(), 2);
    }

    #[test]
    fn run_system_advances_only_written_component_clocks() {
        let mut world = World::new();
        world.spawn().insert(Position(0, 0)).insert(Velocity(1, 1));
        let pos_before = world.storage::<Position>().unwrap().current_tick();
        let vel_before = world.storage::<Velocity>().unwrap().current_tick();

        // A system that writes Position and reads Velocity.
        let mut access = Access::new();
        access.components_mut().add_write::<Position>();
        access.components_mut().add_read::<Velocity>();
        let mut last_seen = Vec::new();
        world.run_system(&access, &mut last_seen, &mut |_w| {});

        // Only the written component's clock advanced.
        assert_eq!(
            world.storage::<Position>().unwrap().current_tick(),
            pos_before + 1
        );
        assert_eq!(
            world.storage::<Velocity>().unwrap().current_tick(),
            vel_before
        );
    }

    #[test]
    fn build_time_insert_visible_to_first_run_then_quiesces() {
        // Caveat #1 fixed architecturally: a component attached before any
        // system ran is visible to a system's first `Changed`/`Added`
        // (clock starts at 1, insert advances to ≥2; a fresh system's
        // baseline is 0). The next run, having observed it, sees nothing.
        let mut world = World::new();
        let e = world.spawn().insert(Position(1, 1)).id();
        let _ = e;

        let mut access = Access::new();
        access.components_mut().add_read::<Position>();
        let mut last_seen = Vec::new();

        let (mut changed, mut added) = (0, 0);
        world.run_system(&access, &mut last_seen, &mut |w| {
            changed = Query::<&Position, Changed<Position>>::from_world(w)
                .iter()
                .count();
            added = Query::<&Position, Added<Position>>::from_world(w)
                .iter()
                .count();
        });
        assert_eq!(changed, 1, "first run sees the pre-existing component");
        assert_eq!(added, 1);

        // Second run: last_seen now records Position's clock, so nothing
        // is newly changed/added.
        let (mut changed2, mut added2) = (0, 0);
        world.run_system(&access, &mut last_seen, &mut |w| {
            changed2 = Query::<&Position, Changed<Position>>::from_world(w)
                .iter()
                .count();
            added2 = Query::<&Position, Added<Position>>::from_world(w)
                .iter()
                .count();
        });
        assert_eq!(changed2, 0, "second run sees no new change");
        assert_eq!(added2, 0);
    }

    #[test]
    fn baseline_for_defaults_to_zero_between_runs() {
        let world = World::new();
        assert_eq!(world.baseline_for::<Position>(), 0);
    }

    #[test]
    fn run_system_is_panic_safe() {
        // A panicking system body must not corrupt change detection for
        // subsequent systems: `run_system` copies (never moves) the
        // baselines in, so `last_seen` survives and the scratch
        // `current_baselines` is overwritten on the next run.
        let mut world = World::new();
        world.spawn().insert(Position(1, 1));
        let mut access = Access::new();
        access.components_mut().add_read::<Position>();

        let mut panicking_seen = Vec::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            world.run_system(&access, &mut panicking_seen, &mut |_w| {
                panic!("system blew up mid-run");
            });
        }));
        assert!(result.is_err());

        // A fresh system still sees the pre-existing component correctly —
        // no stale baseline leaked from the panicking run.
        let mut seen = Vec::new();
        let mut changed = 0;
        world.run_system(&access, &mut seen, &mut |w| {
            changed = Query::<&Position, Changed<Position>>::from_world(w)
                .iter()
                .count();
        });
        assert_eq!(changed, 1);
    }

    #[test]
    fn despawn_then_respawn_at_same_slot_has_clean_change_ticks() {
        // Re-spawning into a freed slot must give the new entity fresh
        // change-detection state, never the dead tenant's stale ticks.
        let mut world = World::new();
        let a = world.spawn().insert(Position(1, 1)).id();
        world.despawn(a);
        let b = world.spawn().insert(Position(2, 2)).id();
        assert_eq!(a.index, b.index); // slot reused
        assert_ne!(a, b); // new generation

        let storage = world.storage::<Position>().unwrap();
        let current = storage.current_tick();
        // `b`'s component was freshly attached: both ticks are non-zero
        // and within the clock — a baseline-0 reader sees it as Added.
        assert_eq!(storage.added_tick_for(b), Some(current));
        assert_eq!(storage.changed_tick_for(b), Some(current));
        // The dead handle's slot is gone entirely.
        assert!(storage.changed_tick_for(a).is_none());
    }

    #[test]
    fn get_mut_write_is_observed_by_changed_filter() {
        // `World::get_mut` advances the clock then stamps, so an ad-hoc
        // direct write is seen by a later `Changed<T>` system once, then
        // quiesces.
        let mut world = World::new();
        let e = world.spawn().insert(Position(1, 1)).id();
        world.get_mut::<Position>(e).unwrap().0 = 99; // advance + stamp
        assert_eq!(world.get::<Position>(e).unwrap().0, 99);

        let mut access = Access::new();
        access.components_mut().add_read::<Position>();
        let mut last_seen = Vec::new();

        let mut first = 0;
        world.run_system(&access, &mut last_seen, &mut |w| {
            first = Query::<&Position, Changed<Position>>::from_world(w)
                .iter()
                .count();
        });
        assert_eq!(first, 1, "the get_mut write is seen on the next run");

        let mut second = 0;
        world.run_system(&access, &mut last_seen, &mut |w| {
            second = Query::<&Position, Changed<Position>>::from_world(w)
                .iter()
                .count();
        });
        assert_eq!(second, 0, "nothing changed since → quiesces");
    }
}
