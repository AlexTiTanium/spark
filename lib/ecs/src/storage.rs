//! Per-component sparse-set storage.
//!
//! Each component type `T` gets its own [`ComponentStorage<T>`]: three
//! parallel vectors that give O(1) insert / remove / lookup while
//! keeping the `dense` data array packed for cache-friendly iteration.
//! [`AnyStorage`] is the object-safe view [`World`](crate::World)
//! holds in its `HashMap<TypeId, _>`, so [`despawn`](crate::World::despawn)
//! can wipe an entity out of every storage without knowing the
//! concrete component types.

use std::any::Any;

use crate::entity::Entity;

/// Marker trait for anything that can live in a [`ComponentStorage`].
///
/// Blanket-implemented over `T: 'static`. The `Send + Sync` bound that
/// the parallel scheduler will need lands in a follow-up PR with the
/// `#[derive(Component)]` macro; today the storage layer accepts any
/// `'static` type so we can prove out the sparse-set machinery first.
///
/// # Examples
///
/// ```
/// use spark_ecs::Component;
///
/// fn _accepts<T: Component>() {}
/// struct Position { x: f32, y: f32 }
/// _accepts::<Position>();
/// ```
pub trait Component: 'static {}
impl<T: 'static> Component for T {}

/// Sparse-set storage for one component type.
///
/// # Logic
///
/// Three vectors working together:
/// - `sparse[entity.index]` is `Some(dense_idx)` if this entity has the
///   component, `None` otherwise. Sparse: most slots are `None`.
/// - `dense` holds the packed `T` values, in arbitrary order.
/// - `entity_index[dense_idx]` names the [`Entity`] that owns
///   `dense[dense_idx]`. Required for [`remove`](Self::remove)'s
///   swap-remove and for [`iter`](Self::iter) yielding `(Entity, &T)`.
///
/// Operations:
/// - [`insert`](Self::insert) — extend `sparse` if needed; either
///   overwrite an existing dense slot or push a new one.
/// - [`remove`](Self::remove) — swap-remove the doomed dense slot with
///   the tail; patch the moved entity's `sparse` pointer.
/// - [`get`](Self::get) — `sparse[index]` → `dense[dense_idx]`, with a
///   generation check via `entity_index`.
///
/// # Memory layout
///
/// For three entities `E0`, `E2`, `E4` holding `Position` components:
///
/// ```text
///   sparse:        [Some(0), None, Some(1), None, Some(2)]
///                     E0           E2             E4
///   dense:         [Pos₀,           Pos₂,         Pos₄]
///                  dense_idx 0     dense_idx 1   dense_idx 2
///   entity_index:  [E0,             E2,           E4]
/// ```
///
/// Removing `E2` swaps `dense[1]` with `dense[2]`, pops the tail, and
/// patches `sparse[E4.index] = Some(1)`:
///
/// ```text
///   sparse:        [Some(0), None, None, None, Some(1)]
///   dense:         [Pos₀,                       Pos₄]
///   entity_index:  [E0,                         E4]
/// ```
///
/// `dense` stays packed; one swap + one pop = O(1).
///
/// # Why it works
///
/// The `entity_index` parallel array is what makes swap-remove sound:
/// after the swap, the entity that *was* at the tail now lives at the
/// freed dense slot, so its `sparse` pointer must move with it.
/// Without `entity_index` you'd have to scan `sparse` to find which
/// entity's pointer to fix — O(n) instead of O(1).
///
/// The generation check inside [`get`](Self::get) / [`remove`](Self::remove)
/// is what makes stale [`Entity`] handles safe: the sparse pointer
/// might still be `Some(_)` from a previous tenant of the same slot,
/// but the `entity_index[dense_idx]` won't match the stale handle, so
/// the lookup returns `None`.
///
/// # How NOT to use
///
/// - Don't iterate `sparse` directly — most entries are `None`.
///   Always walk `dense` (or `iter()` / `iter_mut()`) for `O(n_live)`
///   instead of `O(n_slots)`.
/// - Don't store handles into `dense` — its order changes on every
///   [`remove`](Self::remove). The stable identity is the [`Entity`],
///   not the dense index.
///
/// # Examples
///
/// ```
/// use spark_ecs::World;
///
/// struct Position { x: f32, y: f32 }
///
/// let mut world = World::new();
/// let e = world.spawn().insert(Position { x: 1.0, y: 2.0 }).id();
///
/// let pos = world.get::<Position>(e).unwrap();
/// assert_eq!(pos.x, 1.0);
/// ```
pub struct ComponentStorage<T: Component> {
    sparse: Vec<Option<u32>>,
    dense: Vec<T>,
    entity_index: Vec<Entity>,
}

impl<T: Component> ComponentStorage<T> {
    /// Creates an empty storage.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::ComponentStorage;
    ///
    /// struct Position(f32, f32);
    /// let storage: ComponentStorage<Position> = ComponentStorage::new();
    /// assert!(storage.is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Attaches `value` to `entity`. Returns the previous value if one
    /// was already attached, else `None`.
    ///
    /// # Logic
    ///
    /// 1. Extend `sparse` with `None` entries as needed so
    ///    `sparse[entity.index]` is in-bounds.
    /// 2. If the slot already holds a dense index, overwrite that
    ///    dense entry and return the displaced value.
    /// 3. Otherwise push the value onto `dense`, push the entity onto
    ///    `entity_index`, and record the new dense index in `sparse`.
    ///
    /// # Why it works
    ///
    /// Both branches are O(1) amortised: a `Vec::push` (and an
    /// occasional resize when `sparse` grows). The
    /// `entity_index.push(entity)` mirrors `dense.push(value)` so the
    /// two arrays stay parallel — required by
    /// [`remove`](Self::remove)'s swap-remove dance.
    ///
    /// # Panics
    ///
    /// Panics if the storage already holds [`u32::MAX`] components —
    /// the dense-index space is exhausted. Theoretical concern; the
    /// engine will OOM long before this fires.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{ComponentStorage, EntityAllocator};
    ///
    /// struct Position(f32, f32);
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e = alloc.allocate();
    /// let mut storage = ComponentStorage::<Position>::new();
    ///
    /// assert!(storage.insert(e, Position(1.0, 2.0)).is_none());
    /// let replaced = storage.insert(e, Position(3.0, 4.0)).unwrap();
    /// assert_eq!(replaced.0, 1.0);
    /// ```
    pub fn insert(&mut self, entity: Entity, value: T) -> Option<T> {
        let slot = entity.index as usize;
        if slot >= self.sparse.len() {
            self.sparse.resize(slot + 1, None);
        }
        if let Some(dense_idx) = self.sparse[slot] {
            let di = dense_idx as usize;
            if self.entity_index[di] == entity {
                // Genuine overwrite: same entity, same generation.
                return Some(std::mem::replace(&mut self.dense[di], value));
            }
            // Sparse slot holds a previous tenant's dense pointer
            // (stale generation). Through `World` this can't happen —
            // `despawn` cleans every storage. Direct callers of
            // `ComponentStorage` may not, so reuse the dense slot in
            // place: overwrite both `entity_index[di]` and
            // `dense[di]`. The dead tenant's data is dropped here;
            // nothing reachable through any public API was holding
            // it. Returns `None` because *this* entity had no prior
            // value.
            self.entity_index[di] = entity;
            self.dense[di] = value;
            return None;
        }
        let dense_idx = u32::try_from(self.dense.len())
            .expect("dense storage index space exhausted (u32::MAX components)");
        self.sparse[slot] = Some(dense_idx);
        self.dense.push(value);
        self.entity_index.push(entity);
        None
    }

    /// Detaches `entity`'s component and returns it, or `None` if the
    /// entity didn't have one (or holds a stale handle).
    ///
    /// # Logic
    ///
    /// 1. Take the dense index from `sparse[entity.index]`; bail out
    ///    if absent.
    /// 2. Generation check: `entity_index[dense_idx]` must equal the
    ///    incoming handle. If a previous tenant of the same slot had
    ///    the component but the current one doesn't, this rejects the
    ///    stale handle cleanly.
    /// 3. Swap-remove: if `dense_idx` isn't already the last entry,
    ///    overwrite it with the tail and patch the moved entity's
    ///    `sparse` pointer to its new home.
    ///
    /// # Why it works
    ///
    /// Plain `Vec::remove(dense_idx)` would shift every later element
    /// down — O(n). Swap-remove turns it into one move + one pop, but
    /// requires us to fix up exactly one entity's `sparse` pointer:
    /// the one that got swapped into the freed slot. `entity_index`
    /// is what tells us which entity that is.
    ///
    /// # How NOT to use
    ///
    /// - Don't call `remove` in a tight loop expecting `dense` order
    ///   to stay stable; it doesn't. Iterate first, then remove.
    ///
    /// # Panics
    ///
    /// Panics on internal invariant violation if the dense vector
    /// has grown past [`u32::MAX`] entries — see
    /// [`insert`](Self::insert).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{ComponentStorage, EntityAllocator};
    ///
    /// struct Tag;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e = alloc.allocate();
    /// let mut storage = ComponentStorage::<Tag>::new();
    /// storage.insert(e, Tag);
    /// assert!(storage.remove(e).is_some());
    /// assert!(storage.remove(e).is_none());        // gone
    /// ```
    pub fn remove(&mut self, entity: Entity) -> Option<T> {
        let slot = entity.index as usize;
        let dense_idx = self.sparse.get_mut(slot)?.take()? as usize;

        // Stale handle: same sparse slot, different (older) tenant.
        // Re-insert the dense pointer we just took, then bail.
        if self.entity_index[dense_idx] != entity {
            self.sparse[slot] = Some(u32::try_from(dense_idx).expect("dense idx fits u32"));
            return None;
        }

        let last_dense_idx = self.dense.len() - 1;
        if dense_idx != last_dense_idx {
            // Whoever was at the tail will move into the freed slot.
            // Patch their sparse pointer first, before the swap.
            let displaced = self.entity_index[last_dense_idx];
            self.sparse[displaced.index as usize] =
                Some(u32::try_from(dense_idx).expect("dense idx fits u32"));
        }
        self.entity_index.swap_remove(dense_idx);
        Some(self.dense.swap_remove(dense_idx))
    }

    /// Returns a reference to `entity`'s component, or `None` if it
    /// has none (or holds a stale handle).
    ///
    /// The generation check via `entity_index` rejects stale handles
    /// to recycled slots cleanly — see the type-level docs for why
    /// the parallel array is required.
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
    pub fn get(&self, entity: Entity) -> Option<&T> {
        let dense_idx = self.dense_idx_of(entity)?;
        Some(&self.dense[dense_idx])
    }

    /// Returns a mutable reference to `entity`'s component, or `None`.
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
    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        let dense_idx = self.dense_idx_of(entity)?;
        Some(&mut self.dense[dense_idx])
    }

    /// Iterates `(Entity, &T)` pairs in dense (packed) order.
    ///
    /// Iteration walks `dense` directly, so it's O(n) over live
    /// components — cache-friendly and deterministic within a single
    /// frame (insertion order, minus swap-removes).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{ComponentStorage, EntityAllocator};
    ///
    /// struct Level(u32);
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e0 = alloc.allocate();
    /// let e1 = alloc.allocate();
    /// let mut storage = ComponentStorage::<Level>::new();
    /// storage.insert(e0, Level(1));
    /// storage.insert(e1, Level(5));
    /// let sum: u32 = storage.iter().map(|(_, lvl)| lvl.0).sum();
    /// assert_eq!(sum, 6);
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (Entity, &T)> {
        self.entity_index.iter().copied().zip(self.dense.iter())
    }

    /// Iterates `(Entity, &mut T)` pairs in dense (packed) order.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{ComponentStorage, EntityAllocator};
    ///
    /// struct Health(u32);
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e0 = alloc.allocate();
    /// let mut storage = ComponentStorage::<Health>::new();
    /// storage.insert(e0, Health(100));
    /// for (_, hp) in storage.iter_mut() {
    ///     hp.0 -= 10;
    /// }
    /// assert_eq!(storage.get(e0).unwrap().0, 90);
    /// ```
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, &mut T)> {
        self.entity_index.iter().copied().zip(self.dense.iter_mut())
    }

    /// Returns the number of live components stored.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::ComponentStorage;
    ///
    /// struct Tag;
    /// let storage = ComponentStorage::<Tag>::new();
    /// assert_eq!(storage.len(), 0);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.dense.len()
    }

    /// Returns `true` iff the storage holds no components.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::ComponentStorage;
    ///
    /// struct Tag;
    /// let storage = ComponentStorage::<Tag>::new();
    /// assert!(storage.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    /// Returns `true` iff `entity` has a component in this storage.
    ///
    /// Equivalent to `self.get(entity).is_some()` but doesn't borrow
    /// the component.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::World;
    ///
    /// struct Tag;
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Tag).id();
    /// assert!(world.get::<Tag>(e).is_some());
    /// ```
    #[must_use]
    pub fn contains(&self, entity: Entity) -> bool {
        self.dense_idx_of(entity).is_some()
    }

    /// Hands out the three parallel arrays for one specific consumer:
    /// the `DenseMut` random-access view that powers the
    /// `(D1, &mut T)` tuple impl — i.e. the multi-mut path
    /// (`Query<(&mut A, &mut B)>`). `dense` mutably, `sparse` and
    /// `entity_index` by shared reference.
    ///
    /// Crate-private on purpose — raw access to the parallel arrays
    /// would break every invariant `ComponentStorage` is built around if
    /// called from outside the query layer.
    ///
    /// `entity_index` lets the random-access view perform the
    /// generation check before handing out `&mut T`, matching
    /// [`get`](Self::get) / [`get_mut`](Self::get_mut)'s discipline.
    pub(crate) fn split_for_join(&mut self) -> (&mut [T], &[Option<u32>], &[Entity]) {
        (
            self.dense.as_mut_slice(),
            self.sparse.as_slice(),
            self.entity_index.as_slice(),
        )
    }

    /// Resolves the dense index for `entity` if it's live in this
    /// storage. Checks both the sparse pointer *and* the generation
    /// via `entity_index` to reject stale handles.
    fn dense_idx_of(&self, entity: Entity) -> Option<usize> {
        let dense_idx = (*self.sparse.get(entity.index as usize)?)? as usize;
        if self.entity_index[dense_idx] != entity {
            return None;
        }
        Some(dense_idx)
    }
}

impl<T: Component> Default for ComponentStorage<T> {
    fn default() -> Self {
        Self {
            sparse: Vec::new(),
            dense: Vec::new(),
            entity_index: Vec::new(),
        }
    }
}

/// Object-safe view over a [`ComponentStorage<T>`] of erased `T`.
///
/// [`World`](crate::World) stores `Box<dyn AnyStorage>` per
/// [`TypeId`](std::any::TypeId), so [`despawn`](crate::World::despawn)
/// can walk every storage and ask it to drop the doomed entity —
/// without `World` knowing which component types are present.
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
/// let e = world.spawn().insert(Position(0.0, 0.0)).insert(Velocity(1.0, 0.0)).id();
/// world.despawn(e);
/// assert!(world.get::<Position>(e).is_none());
/// assert!(world.get::<Velocity>(e).is_none());
/// ```
pub trait AnyStorage: Any {
    /// Upcast to `&dyn Any` for downcasting back to a concrete
    /// `ComponentStorage<T>`.
    fn as_any(&self) -> &dyn Any;

    /// Mutable upcast to `&mut dyn Any`.
    fn as_any_mut(&mut self) -> &mut dyn Any;

    /// Removes `entity` from this storage, if present. No-op
    /// otherwise. Called by [`World::despawn`](crate::World::despawn)
    /// for every registered storage.
    fn remove_entity(&mut self, entity: Entity);
}

impl<T: Component> AnyStorage for ComponentStorage<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn remove_entity(&mut self, entity: Entity) {
        let _ = self.remove(entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityAllocator;

    #[derive(Debug, PartialEq)]
    struct Pos(i32, i32);

    fn alloc_n(n: usize) -> (EntityAllocator, Vec<Entity>) {
        let mut alloc = EntityAllocator::new();
        let entities = (0..n).map(|_| alloc.allocate()).collect();
        (alloc, entities)
    }

    #[test]
    fn insert_then_get() {
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        assert!(storage.insert(entities[0], Pos(1, 2)).is_none());
        assert_eq!(storage.get(entities[0]), Some(&Pos(1, 2)));
        assert_eq!(storage.len(), 1);
    }

    #[test]
    fn insert_twice_overwrites_and_returns_old() {
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(entities[0], Pos(1, 2));
        let old = storage.insert(entities[0], Pos(3, 4)).unwrap();
        assert_eq!(old, Pos(1, 2));
        assert_eq!(storage.get(entities[0]), Some(&Pos(3, 4)));
    }

    #[test]
    fn remove_swap_keeps_dense_packed() {
        let (_alloc, entities) = alloc_n(3);
        let mut storage = ComponentStorage::<Pos>::new();
        for (i, &e) in entities.iter().enumerate() {
            storage.insert(e, Pos(i32::try_from(i).unwrap(), 0));
        }
        let removed = storage.remove(entities[0]).unwrap();
        assert_eq!(removed, Pos(0, 0));
        // dense stays packed; the previously-last entity slid into slot 0.
        assert_eq!(storage.len(), 2);
        assert_eq!(storage.get(entities[1]), Some(&Pos(1, 0)));
        assert_eq!(storage.get(entities[2]), Some(&Pos(2, 0)));
        assert!(storage.get(entities[0]).is_none());
    }

    #[test]
    fn remove_tail_does_not_swap() {
        // Regression: removing the *last* dense entry must NOT patch a
        // sparse pointer for itself.
        let (_alloc, entities) = alloc_n(2);
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(entities[0], Pos(0, 0));
        storage.insert(entities[1], Pos(1, 1));
        assert_eq!(storage.remove(entities[1]), Some(Pos(1, 1)));
        assert_eq!(storage.get(entities[0]), Some(&Pos(0, 0)));
        assert!(storage.get(entities[1]).is_none());
    }

    #[test]
    fn insert_with_fresh_handle_evicts_stale_tenant_in_place() {
        // Regression for the stale-tenant branch in `insert`: when the
        // sparse slot is still pointing at a dead previous tenant
        // (only reachable when bypassing World — through World,
        // despawn cleans every storage), inserting with a fresh
        // handle must overwrite that dense slot in place, not push a
        // new entry and orphan the old one.
        let mut alloc = EntityAllocator::new();
        let dead = alloc.allocate();
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(dead, Pos(7, 7));
        // Spawn a sibling at a different index so the storage has
        // more than one dense entry — confirms we touch only the
        // stale slot, not the sibling.
        let sibling = alloc.allocate();
        storage.insert(sibling, Pos(99, 99));

        // Destroy `dead` *through the allocator only*; the storage
        // is intentionally not cleaned, mimicking a direct
        // `ComponentStorage` user who skipped a despawn cascade.
        alloc.destroy(dead);
        let fresh = alloc.allocate(); // reuses dead's slot, new generation
        assert_eq!(dead.index, fresh.index);
        assert_ne!(dead, fresh);

        assert_eq!(storage.len(), 2);
        let prev = storage.insert(fresh, Pos(1, 2));

        // The fresh handle had no prior value of its own.
        assert!(prev.is_none());
        // Storage stays the same size — the stale entry was reused
        // in place, not pushed alongside.
        assert_eq!(storage.len(), 2);
        // Fresh handle now resolves.
        assert_eq!(storage.get(fresh), Some(&Pos(1, 2)));
        // Stale handle no longer resolves.
        assert!(storage.get(dead).is_none());
        // Sibling untouched.
        assert_eq!(storage.get(sibling), Some(&Pos(99, 99)));
    }

    #[test]
    fn stale_handle_rejected() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.allocate();
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(e, Pos(7, 7));

        alloc.destroy(e);
        let fresh = alloc.allocate(); // reuses slot, new generation

        // The storage still has Pos under the slot's old tenant.
        // The fresh handle should not see it.
        assert!(storage.get(fresh).is_none());
        assert!(storage.remove(fresh).is_none());

        // The original (stale) handle also doesn't see it — but the
        // data is *physically* still there until something cleans it
        // up. World::despawn is what cleans it; raw ComponentStorage
        // doesn't know about the allocator.
        assert!(
            storage.get(e).is_some(),
            "raw storage isn't generation-aware on its own"
        );
    }

    #[test]
    fn iter_yields_packed_pairs() {
        let (_alloc, entities) = alloc_n(3);
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(entities[0], Pos(0, 0));
        storage.insert(entities[1], Pos(1, 1));
        storage.insert(entities[2], Pos(2, 2));
        let pairs: Vec<_> = storage.iter().collect();
        assert_eq!(pairs.len(), 3);
        // Insertion order before any remove.
        assert_eq!(pairs[0].0, entities[0]);
        assert_eq!(pairs[2].0, entities[2]);
    }

    #[test]
    fn contains_matches_get() {
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        assert!(!storage.contains(entities[0]));
        storage.insert(entities[0], Pos(1, 2));
        assert!(storage.contains(entities[0]));
    }

    #[test]
    fn remove_entity_via_any_storage_trait() {
        // The despawn cascade goes through dyn AnyStorage — exercise
        // it via the trait method, not the inherent one.
        let (_alloc, entities) = alloc_n(1);
        let mut storage: Box<dyn AnyStorage> = Box::new(ComponentStorage::<Pos>::new());
        storage
            .as_any_mut()
            .downcast_mut::<ComponentStorage<Pos>>()
            .unwrap()
            .insert(entities[0], Pos(3, 3));
        storage.remove_entity(entities[0]);
        let typed = storage
            .as_any()
            .downcast_ref::<ComponentStorage<Pos>>()
            .unwrap();
        assert!(typed.is_empty());
    }
}
