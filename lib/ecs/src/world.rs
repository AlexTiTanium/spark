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
//! threaded today; the `Send + Sync` bound that the parallel
//! scheduler will need lands with the upcoming `#[derive(Component)]`
//! / `#[derive(Resource)]` macros.

use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;

use crate::entity::{Entity, EntityAllocator};
use crate::storage::{AnyStorage, Component, ComponentStorage};

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
/// # Why only `'static` today, `Send + Sync` later
///
/// `add_resource` / `insert` take any `T: 'static` — the minimum bound
/// to drop a value into a `Box<dyn Any>` map. That isn't the bound
/// real resources or components will ship with. Spark targets a heavy
/// simulation and parallel system execution is a committed M4
/// requirement, not an optional extra. The agreed direction: the
/// `Resource` and `Component` traits — introduced with the derive PR
/// — carry `Send + Sync + 'static`, [`SystemParam`](crate::SystemParam)
/// impls thread that bound through, and the M4 scheduler uses it as
/// the safety proof for lockless parallel execution.
///
/// This PR doesn't introduce that trait yet, so there's genuinely no
/// bound to add to the storage maps today — the permissive `'static`
/// defers the choice to the trait, where it belongs. `World` itself
/// is `!Send + !Sync` by construction, matching the single-threaded
/// scheduler that ships with this PR.
///
/// # Examples
///
/// ```
/// use spark_ecs::World;
///
/// struct Position { x: f32, y: f32 }
/// struct Velocity { x: f32, y: f32 }
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
    entities: EntityAllocator,
    components: HashMap<TypeId, RefCell<Box<dyn AnyStorage>>>,
    resources: HashMap<TypeId, RefCell<Box<dyn Any>>>,
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
    /// use spark_ecs::World;
    ///
    /// struct FrameRate(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(FrameRate(60));
    /// world.add_resource(FrameRate(120)); // replaces the previous value
    /// ```
    pub fn add_resource<T: 'static>(&mut self, value: T) {
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
    /// use spark_ecs::World;
    ///
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// assert!(world.get_resource::<Score>().is_none());
    /// world.add_resource(Score(7));
    /// assert_eq!(world.get_resource::<Score>().unwrap().0, 7);
    /// ```
    #[must_use]
    pub fn get_resource<T: 'static>(&self) -> Option<Ref<'_, T>> {
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
    /// use spark_ecs::World;
    ///
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(0));
    /// world.get_resource_mut::<Score>().unwrap().0 = 1;
    /// assert_eq!(world.get_resource::<Score>().unwrap().0, 1);
    /// ```
    #[must_use]
    pub fn get_resource_mut<T: 'static>(&self) -> Option<RefMut<'_, T>> {
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
    /// use spark_ecs::World;
    ///
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(9));
    /// assert_eq!(world.resource::<Score>().0, 9);
    /// ```
    #[must_use]
    pub fn resource<T: 'static>(&self) -> Ref<'_, T> {
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
    /// use spark_ecs::World;
    ///
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(0));
    /// world.resource_mut::<Score>().0 = 100;
    /// assert_eq!(world.resource::<Score>().0, 100);
    /// ```
    #[must_use]
    pub fn resource_mut<T: 'static>(&self) -> RefMut<'_, T> {
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
    /// use spark_ecs::World;
    ///
    /// struct Position(f32, f32);
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
        let entity = self.entities.allocate();
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
    /// use spark_ecs::World;
    ///
    /// struct Position(f32, f32);
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
        if !self.entities.is_alive(entity) {
            return false;
        }
        for cell in self.components.values_mut() {
            cell.get_mut().remove_entity(entity);
        }
        self.entities.destroy(entity)
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
        self.entities.is_alive(entity)
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
    /// use spark_ecs::World;
    ///
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().id();
    /// assert!(world.insert(e, Position(1.0, 2.0)).is_none());
    /// let old = world.insert(e, Position(3.0, 4.0)).unwrap();
    /// assert!((old.0 - 1.0).abs() < f32::EPSILON);
    /// ```
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> Option<T> {
        if !self.entities.is_alive(entity) {
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
    /// use spark_ecs::World;
    ///
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Position(1.0, 2.0)).id();
    /// let pos = world.remove::<Position>(e).unwrap();
    /// assert!((pos.0 - 1.0).abs() < f32::EPSILON);
    /// assert!(world.get::<Position>(e).is_none());
    /// ```
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Option<T> {
        if !self.entities.is_alive(entity) {
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
    /// use spark_ecs::World;
    ///
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Health(100)).id();
    /// assert_eq!(world.get::<Health>(e).unwrap().0, 100);
    /// ```
    #[must_use]
    pub fn get<T: Component>(&self, entity: Entity) -> Option<Ref<'_, T>> {
        if !self.entities.is_alive(entity) {
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
    /// use spark_ecs::World;
    ///
    /// struct Health(u32);
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Health(100)).id();
    /// world.get_mut::<Health>(e).unwrap().0 -= 25;
    /// assert_eq!(world.get::<Health>(e).unwrap().0, 75);
    /// ```
    #[must_use]
    pub fn get_mut<T: Component>(&self, entity: Entity) -> Option<RefMut<'_, T>> {
        if !self.entities.is_alive(entity) {
            return None;
        }
        let cell = self.components.get(&TypeId::of::<T>())?;
        RefMut::filter_map(cell.borrow_mut(), |b| {
            b.as_any_mut()
                .downcast_mut::<ComponentStorage<T>>()
                .expect("TypeId key and stored storage type must agree")
                .get_mut(entity)
        })
        .ok()
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
/// use spark_ecs::World;
///
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
    /// use spark_ecs::World;
    ///
    /// struct Position(f32, f32);
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
    use std::cell::Cell;
    use std::rc::Rc;

    struct A(u32);
    struct B(&'static str);

    // Integer fields — keeps the unit tests free of `clippy::float_cmp`
    // assertions. Doc tests (which clippy doesn't lint) stay with the
    // canonical `f32` flavour to read like real engine code.
    #[derive(Debug, PartialEq)]
    struct Position(i32, i32);

    #[derive(Debug, PartialEq)]
    struct Velocity(i32, i32);

    #[derive(Debug, PartialEq)]
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
    fn non_send_resource_compiles() {
        // `Rc<Cell<_>>` is `!Send + !Sync`. While there's no `Resource`
        // trait yet, the storage map's bare `'static` bound has to
        // accept it. Once the trait lands with `Send + Sync + 'static`
        // (committed M4 direction), this test goes away in favour of a
        // compile-fail check that `!Send` types can't be stored.
        let counter = Rc::new(Cell::new(0_u32));
        let mut world = World::new();
        world.add_resource(counter);
        assert_eq!(world.resource::<Rc<Cell<u32>>>().get(), 0);
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
}
