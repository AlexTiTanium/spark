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
use std::ops::{Deref, DerefMut};

use crate::Component;
use crate::entity::Entity;

/// A change-detecting mutable handle to a component, yielded by
/// `Query<&mut T>` iteration.
///
/// # Logic
///
/// Wraps the component plus a pointer to its `changed_tick` slot and the
/// component's current tick. [`Deref`] hands out `&T` and touches nothing;
/// [`DerefMut`] stamps `changed_tick = current_tick` *before* returning
/// `&mut T`. A component is therefore marked changed exactly when the
/// caller takes a `&mut` to it — reading through a `Mut` never marks it.
///
/// # Why it works
///
/// Change detection must answer "did this system write this component?"
/// A bare `&mut T` can't tell — the borrow exists whether or not you
/// assign through it. Routing the `&mut` through `DerefMut` makes the
/// *act of taking the mutable borrow* the trigger, so marking is precise:
/// iterate a thousand entities, write three, and only three
/// `changed_tick`s move. That precision is what lets `Query<&mut A>` drive
/// a tuple join or sit behind a filter without falsely marking entities
/// the body never wrote.
///
/// # How NOT to use
///
/// - Don't bind `let r = &mut *m;` and then *not* write — that still marks
///   changed, because you took the mutable borrow. Read through `&*m` (or
///   a plain `Query<&T>`) when you only intend to observe.
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
/// let mut q = Query::<&mut Health>::from_world(&world);
/// for mut hp in q.iter_mut() {
///     if hp.0 > 50 {
///         hp.0 -= 10; // DerefMut here marks the component changed
///     }
/// }
/// ```
pub struct Mut<'a, T> {
    value: &'a mut T,
    changed_tick: &'a mut u32,
    current_tick: u32,
}

impl<'a, T> Mut<'a, T> {
    /// Wraps a component slot and its `changed_tick` for change detection.
    /// Crate-internal: only the storage / query layers know the slot is
    /// kept in lockstep with `dense`.
    #[inline]
    pub(crate) fn new(value: &'a mut T, changed_tick: &'a mut u32, current_tick: u32) -> Self {
        Self {
            value,
            changed_tick,
            current_tick,
        }
    }
}

impl<T> Deref for Mut<'_, T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &T {
        self.value
    }
}

impl<T> DerefMut for Mut<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        // Taking the mutable borrow is the change signal.
        *self.changed_tick = self.current_tick;
        self.value
    }
}

/// Sparse-set storage for one component type, carrying its **own**
/// change-detection clock.
///
/// # Logic
///
/// Five fields. The three sparse-set vectors:
/// - `sparse[entity.index]` is `Some(dense_idx)` if this entity has the
///   component, `None` otherwise. Sparse: most slots are `None`.
/// - `dense` holds the packed `T` values, in arbitrary order.
/// - `entity_index[dense_idx]` names the [`Entity`] that owns
///   `dense[dense_idx]`. Required for [`remove`](Self::remove)'s
///   swap-remove and for [`iter`](Self::iter) yielding `(Entity, &T)`.
///
/// Plus the per-component change-detection state:
/// - `changed_tick[dense_idx]` / `added_tick[dense_idx]` — the tick at
///   which the slot was last written / first attached (parallel to
///   `dense`).
/// - `current_tick` — **this component's own clock.** Unlike a single
///   world tick, each storage advances independently: `Position`'s clock
///   bumps only when something writes `Position`. [`insert`](Self::insert)
///   advances it; the scheduler advances it once before a system that
///   declares a write of `T` (so that system's in-place edits share one
///   tick). Starts at 1, so `0` is a clean "never observed" baseline.
///
/// Operations:
/// - [`insert`](Self::insert) — advance the clock, then overwrite an
///   existing dense slot or push a new one, stamping the tick(s).
/// - [`remove`](Self::remove) — swap-remove the doomed slot across **all
///   four** parallel arrays; patch the moved entity's `sparse` pointer.
/// - [`get`](Self::get) — `sparse[index]` → `dense[dense_idx]`, with a
///   generation check via `entity_index`.
///
/// # Memory layout
///
/// Three entities `E0`, `E2`, `E4` holding `Position`, last written at
/// this component's ticks 4 / 4 / 9, first attached at 4 / 4 / 7:
///
/// ```text
///   sparse:        [Some(0), None, Some(1), None, Some(2)]
///                     E0           E2             E4
///   dense:         [Pos₀,           Pos₂,         Pos₄]
///                  dense_idx 0     dense_idx 1   dense_idx 2
///   entity_index:  [E0,             E2,           E4]
///   changed_tick:  [4,              4,            9]   ← parallel to dense
///   added_tick:    [4,              4,            7]   ← parallel to dense
///   current_tick:  9   (this Position storage's own clock)
/// ```
///
/// Removing `E2` swap-removes index 1 across all four arrays and patches
/// `sparse[E4.index] = Some(1)`:
///
/// ```text
///   sparse:        [Some(0), None, None, None, Some(1)]
///   dense:         [Pos₀,                       Pos₄]
///   entity_index:  [E0,                         E4]
///   changed_tick:  [4,                          9]
///   added_tick:    [4,                          7]
/// ```
///
/// `dense` stays packed; one swap + one pop per array = O(1).
///
/// # Why it works
///
/// The `entity_index` parallel array is what makes swap-remove sound:
/// after the swap, the entity that *was* at the tail now lives at the
/// freed dense slot, so its `sparse` pointer must move with it.
/// `changed_tick` / `added_tick` ride the same swap so all four stay
/// aligned by `dense_idx`.
///
/// The generation check inside [`get`](Self::get) / [`remove`](Self::remove)
/// is what makes stale [`Entity`] handles safe: the sparse pointer
/// might still be `Some(_)` from a previous tenant of the same slot,
/// but the `entity_index[dense_idx]` won't match the stale handle, so
/// the lookup returns `None`.
///
/// The per-component clock is the heart of change detection here: a
/// reader records "I last saw `T` at tick N" and a write bumps `T`'s
/// clock past N, so `changed_tick > reader_baseline` answers "changed
/// since I looked" without any global frame counter. The invariant
/// `changed_tick[i] >= added_tick[i]` always holds.
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
/// use spark_ecs::{Component, World};
///
/// #[derive(Component)]
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
    /// Tick of the last write to each slot. Parallel to `dense`.
    changed_tick: Vec<u32>,
    /// Tick each slot's component was first attached. Parallel to `dense`.
    added_tick: Vec<u32>,
    /// This component type's own change-detection clock. Advanced by
    /// [`insert`](Self::insert) and by the scheduler before a writing
    /// system; never by a read. Starts at 1.
    current_tick: u32,
}

impl<T: Component> ComponentStorage<T> {
    /// Creates an empty storage.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, ComponentStorage};
    ///
    /// #[derive(Component)]
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
    /// use spark_ecs::{Component, ComponentStorage, EntityAllocator};
    ///
    /// #[derive(Component)]
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
        // A structural attach/overwrite is its own write event: advance
        // this component's clock so the stamps below land strictly after
        // any prior observation. This is what makes a `Commands`-spawned
        // or build-time component visible to a later reader with no
        // separate flush-tick bookkeeping. (Cost: a bulk spawn of N
        // entities advances *this* component's clock N times — harmless
        // for the wrapping `u32`, and per-component so it stays local to
        // the spawn-heavy type.)
        self.current_tick = self.current_tick.wrapping_add(1);
        let tick = self.current_tick;

        let slot = entity.index as usize;
        if slot >= self.sparse.len() {
            self.sparse.resize(slot + 1, None);
        }
        if let Some(dense_idx) = self.sparse[slot] {
            let di = dense_idx as usize;
            if self.entity_index[di] == entity {
                // Genuine overwrite: same entity, same generation. The
                // value changed but it was not newly added — bump
                // `changed_tick` only; `added_tick[di]` keeps its stamp.
                self.changed_tick[di] = tick;
                return Some(std::mem::replace(&mut self.dense[di], value));
            }
            // Sparse slot holds a previous tenant's dense pointer
            // (stale generation). Through `World` this can't happen —
            // `despawn` cleans every storage. Direct callers of
            // `ComponentStorage` may not, so reuse the dense slot in
            // place: overwrite `entity_index[di]`, `dense[di]`, and both
            // ticks. This is a *fresh attach* for the new handle, so both
            // ticks bump. Returns `None` because *this* entity had no
            // prior value.
            self.entity_index[di] = entity;
            self.dense[di] = value;
            self.changed_tick[di] = tick;
            self.added_tick[di] = tick;
            return None;
        }
        let dense_idx = u32::try_from(self.dense.len())
            .expect("dense storage index space exhausted (u32::MAX components)");
        self.sparse[slot] = Some(dense_idx);
        self.dense.push(value);
        self.entity_index.push(entity);
        self.changed_tick.push(tick);
        self.added_tick.push(tick);
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
    /// use spark_ecs::{Component, ComponentStorage, EntityAllocator};
    ///
    /// #[derive(Component)]
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
        self.changed_tick.swap_remove(dense_idx);
        self.added_tick.swap_remove(dense_idx);
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
    pub fn get(&self, entity: Entity) -> Option<&T> {
        let dense_idx = self.dense_idx_of(entity)?;
        Some(&self.dense[dense_idx])
    }

    /// Returns a mutable reference to `entity`'s component, or `None`,
    /// stamping its `changed_tick` with this component's `current_tick`.
    ///
    /// Stamps but does **not** advance the clock; `added_tick` is
    /// untouched. Whoever wants the stamp to land *past* prior
    /// observations advances first — [`World::get_mut`](crate::World::get_mut)
    /// does so before delegating here, and the scheduler advances a
    /// written component's clock before the system runs (so a
    /// `Query<&mut T>`'s edits all share one tick).
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
    pub fn get_mut(&mut self, entity: Entity) -> Option<&mut T> {
        let dense_idx = self.dense_idx_of(entity)?;
        self.changed_tick[dense_idx] = self.current_tick;
        Some(&mut self.dense[dense_idx])
    }

    /// Returns a change-marking [`Mut`] handle to `entity`'s component, or
    /// `None` if absent / stale — the single-entity counterpart of
    /// [`iter_mut`](Self::iter_mut), and the path
    /// [`Query::get_mut`](crate::Query::get_mut) takes for one-off fetches so
    /// precise change detection survives outside iteration.
    ///
    /// The returned [`Mut`] stamps `changed_tick` only on a write through
    /// [`DerefMut`](std::ops::DerefMut); dropping the handle without writing
    /// marks nothing — same precision contract as `iter_mut`.
    #[inline]
    pub(crate) fn get_mut_handle(&mut self, entity: Entity) -> Option<Mut<'_, T>> {
        let dense_idx = self.dense_idx_of(entity)?;
        let tick = self.current_tick;
        Some(Mut::new(
            &mut self.dense[dense_idx],
            &mut self.changed_tick[dense_idx],
            tick,
        ))
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
    /// use spark_ecs::{Component, ComponentStorage, EntityAllocator};
    ///
    /// #[derive(Component)]
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

    /// Iterates `(Entity, Mut<T>)` pairs in dense (packed) order.
    ///
    /// Each component is wrapped in a [`Mut`] change-marker: the entry's
    /// `changed_tick` moves to this component's `current_tick` only when
    /// the caller takes a `&mut` through [`DerefMut`](std::ops::DerefMut)
    /// (writing). Reading through `Deref`, or skipping an entry, marks
    /// nothing — so marking is precise, with no driver/filter over-marking.
    /// Does not advance the clock (the scheduler does that once before a
    /// writing system). `added_tick` is never touched here.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, ComponentStorage, EntityAllocator};
    ///
    /// #[derive(Component)]
    /// struct Health(u32);
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e0 = alloc.allocate();
    /// let mut storage = ComponentStorage::<Health>::new();
    /// storage.insert(e0, Health(100));
    /// for (_, mut hp) in storage.iter_mut() {
    ///     hp.0 -= 10; // DerefMut marks this entry changed
    /// }
    /// assert_eq!(storage.get(e0).unwrap().0, 90);
    /// ```
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Entity, Mut<'_, T>)> {
        let tick = self.current_tick;
        self.entity_index
            .iter()
            .copied()
            .zip(self.dense.iter_mut())
            .zip(self.changed_tick.iter_mut())
            .map(move |((entity, value), changed)| (entity, Mut::new(value, changed, tick)))
    }

    /// Returns the number of live components stored.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, ComponentStorage};
    ///
    /// #[derive(Component)]
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
    /// use spark_ecs::{Component, ComponentStorage};
    ///
    /// #[derive(Component)]
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
    /// use spark_ecs::{Component, World};
    ///
    /// #[derive(Component)]
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

    /// The entities holding this component, in dense (packed) order.
    ///
    /// This is the storage's *candidate set*: every entity with a `T`, each
    /// appearing exactly once, in the same order [`iter`](Self::iter) walks.
    /// Query driver selection (issue #65) reads it to drive iteration off the
    /// smallest matching storage — the list whose length [`len`](Self::len)
    /// reports — instead of the live entity set. Returns an empty slice when
    /// the storage is empty.
    ///
    /// Because it is the *same* order `iter` yields, a query that drives off
    /// this slice and looks up each entity's components produces the same
    /// rows `iter` would, just for a chosen storage.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Component, ComponentStorage, EntityAllocator};
    ///
    /// #[derive(Component)]
    /// struct Tag;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let a = alloc.allocate();
    /// let b = alloc.allocate();
    /// let mut storage = ComponentStorage::<Tag>::new();
    /// storage.insert(a, Tag);
    /// storage.insert(b, Tag);
    /// assert_eq!(storage.entities(), &[a, b]);
    /// ```
    #[must_use]
    pub fn entities(&self) -> &[Entity] {
        &self.entity_index
    }

    /// Hands out the parallel arrays for one specific consumer: the
    /// `DenseMut` random-access view that powers the `(D1, &mut T)` tuple
    /// impl — i.e. the multi-mut path (`Query<(&mut A, &mut B)>`).
    /// `dense` and `changed_tick` mutably, `sparse` and `entity_index` by
    /// shared reference, plus this component's `current_tick` to stamp.
    ///
    /// Crate-private on purpose — raw access to the parallel arrays
    /// would break every invariant `ComponentStorage` is built around if
    /// called from outside the query layer.
    ///
    /// `entity_index` lets the random-access view perform the generation
    /// check before handing out `&mut T`. The mutable `changed_tick`
    /// slice + `current_tick` let `DenseMut::get` build a [`Mut`] that
    /// marks only the entities the join actually writes.
    #[allow(clippy::type_complexity)]
    // Five-element raw split for a single consumer (`DenseMut::new`) that
    // destructures it immediately with named bindings; a wrapper struct
    // would be ceremony for one crate-private call site.
    pub(crate) fn split_for_join(
        &mut self,
    ) -> (&mut [T], &mut [u32], &[Option<u32>], &[Entity], u32) {
        (
            self.dense.as_mut_slice(),
            self.changed_tick.as_mut_slice(),
            self.sparse.as_slice(),
            self.entity_index.as_slice(),
            self.current_tick,
        )
    }

    /// The tick at which `entity`'s component was last written, or `None`
    /// if absent. Read by the [`Changed<T>`](crate::Changed) filter.
    pub(crate) fn changed_tick_for(&self, entity: Entity) -> Option<u32> {
        self.dense_idx_of(entity).map(|i| self.changed_tick[i])
    }

    /// The tick at which `entity`'s component was first attached, or
    /// `None` if absent. Read by the [`Added<T>`](crate::Added) filter.
    pub(crate) fn added_tick_for(&self, entity: Entity) -> Option<u32> {
        self.dense_idx_of(entity).map(|i| self.added_tick[i])
    }

    /// This component's current clock value — the reference point the
    /// [`Changed<T>`](crate::Changed) / [`Added<T>`](crate::Added) filters
    /// measure tick *ages* against in their wrapping-aware comparison
    /// (always `>=` every stamped `changed_tick` / `added_tick` and every
    /// parked baseline, since the clock only advances).
    pub(crate) fn current_tick(&self) -> u32 {
        self.current_tick
    }

    /// Overrides this component's clock. Test-only control point for
    /// driving ticks to specific values; normal operation advances the
    /// clock via [`insert`](Self::insert) and [`AnyStorage::advance_tick`].
    #[cfg(test)]
    pub(crate) fn set_current_tick(&mut self, tick: u32) {
        self.current_tick = tick;
    }

    /// Resolves the dense index for `entity` if it's live in this
    /// storage. Checks both the sparse pointer *and* the generation
    /// via `entity_index` to reject stale handles.
    #[inline]
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
            changed_tick: Vec::new(),
            added_tick: Vec::new(),
            // Starts at 1 so the `0` baseline means "never observed" and
            // a component's first write is always visible.
            current_tick: 1,
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
/// use spark_ecs::{Component, World};
///
/// #[derive(Component)]
/// struct Position(f32, f32);
/// #[derive(Component)]
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

    /// Advances this component's change-detection clock by one tick.
    /// Type-erased so the [`World`](crate::World) can advance a storage
    /// it knows only by [`TypeId`](std::any::TypeId) — used before a
    /// system that declares a write of this component.
    fn advance_tick(&mut self);

    /// This component's current change-detection tick. Type-erased so the
    /// [`World`](crate::World) can record a system's "last seen" baseline
    /// without naming the concrete component type.
    fn current_tick(&self) -> u32;
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

    fn advance_tick(&mut self) {
        self.current_tick = self.current_tick.wrapping_add(1);
    }

    fn current_tick(&self) -> u32 {
        self.current_tick
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Component;
    use crate::entity::EntityAllocator;

    #[derive(Debug, PartialEq, Component)]
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

    // -------- change detection: per-component clock --------

    #[test]
    fn fresh_insert_advances_clock_and_stamps_both_ticks() {
        // A new storage starts at tick 1; the first insert advances to 2
        // and stamps both ticks (so a baseline-0 reader sees it).
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        assert_eq!(storage.current_tick(), 1);
        storage.insert(entities[0], Pos(1, 2));
        assert_eq!(storage.current_tick(), 2);
        assert_eq!(storage.changed_tick_for(entities[0]), Some(2));
        assert_eq!(storage.added_tick_for(entities[0]), Some(2));
    }

    #[test]
    fn overwrite_advances_and_bumps_changed_not_added() {
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(entities[0], Pos(1, 2)); // added = changed = 2
        storage.insert(entities[0], Pos(3, 4)); // advance → 3, changed only
        assert_eq!(storage.changed_tick_for(entities[0]), Some(3));
        assert_eq!(storage.added_tick_for(entities[0]), Some(2));
    }

    #[test]
    fn iter_mut_marks_only_written_entries() {
        // The `Mut` deref-marker is precise: iterating without writing
        // marks nothing; writing one entry marks only that one.
        let (_alloc, entities) = alloc_n(2);
        let mut storage = ComponentStorage::<Pos>::new();
        storage.insert(entities[0], Pos(0, 0)); // changed = 2
        storage.insert(entities[1], Pos(1, 1)); // changed = 3

        storage.set_current_tick(10);
        for (_, _) in storage.iter_mut() {} // consume, mutate nothing
        assert_eq!(storage.changed_tick_for(entities[0]), Some(2)); // untouched
        assert_eq!(storage.changed_tick_for(entities[1]), Some(3)); // untouched

        for (entity, mut pos) in storage.iter_mut() {
            if entity == entities[0] {
                pos.0 += 1; // DerefMut marks entity 0 only
            }
        }
        assert_eq!(storage.changed_tick_for(entities[0]), Some(10)); // written
        assert_eq!(storage.changed_tick_for(entities[1]), Some(3)); // skipped
    }

    #[test]
    fn remove_keeps_all_four_arrays_aligned() {
        // Distinct ticks per entity so a misaligned swap_remove would show
        // up as a tick travelling to the wrong dense slot.
        let (_alloc, entities) = alloc_n(3);
        let mut storage = ComponentStorage::<Pos>::new();
        for (i, &e) in entities.iter().enumerate() {
            storage.insert(e, Pos(i32::try_from(i).unwrap(), 0));
        }
        // Inserts advanced the clock to 2, 3, 4 → those are the added ticks.
        storage.remove(entities[0]); // tail (entity 2) slides into slot 0
        assert_eq!(storage.added_tick_for(entities[2]), Some(4));
        assert_eq!(storage.changed_tick_for(entities[2]), Some(4));
        assert_eq!(storage.added_tick_for(entities[1]), Some(3));
        assert!(storage.changed_tick_for(entities[0]).is_none());
    }

    #[test]
    fn changed_never_below_added_invariant() {
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        let ok = |s: &ComponentStorage<Pos>, e| {
            s.changed_tick_for(e).unwrap() >= s.added_tick_for(e).unwrap()
        };
        storage.insert(entities[0], Pos(0, 0));
        assert!(ok(&storage, entities[0]));
        storage.insert(entities[0], Pos(1, 1)); // overwrite
        assert!(ok(&storage, entities[0]));
        storage.set_current_tick(9);
        let _ = storage.get_mut(entities[0]); // stamp changed
        assert!(ok(&storage, entities[0]));
    }

    #[test]
    fn any_storage_advance_and_read_clock() {
        // The type-erased clock controls the `World` uses by `TypeId`.
        let mut storage = ComponentStorage::<Pos>::new();
        assert_eq!(AnyStorage::current_tick(&storage), 1);
        AnyStorage::advance_tick(&mut storage);
        assert_eq!(AnyStorage::current_tick(&storage), 2);
    }

    #[test]
    fn tick_wraparound_does_not_panic_and_is_detected_across_the_wrap() {
        // The clock is `wrapping_add`. Driving it across `u32::MAX` must not
        // panic, and — after the Phase-4 change-detection fix (issue #80) —
        // a write stamped *after* the wrap must still be detected against a
        // pre-wrap baseline. (Before the fix this was a documented
        // false-negative: `0 > u32::MAX - 1` is false. The
        // `Changed`/`Added` filters now use a wrapping-aware relative-age
        // comparison; this pins the storage-level inputs to it.)
        let (_alloc, entities) = alloc_n(1);
        let mut storage = ComponentStorage::<Pos>::new();
        storage.set_current_tick(u32::MAX - 1);
        storage.insert(entities[0], Pos(1, 2)); // advance → u32::MAX, stamp both
        assert_eq!(storage.changed_tick_for(entities[0]), Some(u32::MAX));
        storage.insert(entities[0], Pos(3, 4)); // overwrite: advance wraps → 0
        assert_eq!(storage.current_tick(), 0);
        assert_eq!(storage.changed_tick_for(entities[0]), Some(0)); // wrapped, no panic

        // A reader whose baseline was the pre-wrap tick now SEES this write.
        // The filter's comparison is `current - tick < current - baseline`:
        // with current = 0, tick = 0, baseline = u32::MAX - 1 that is
        // `0 < 2`, i.e. changed. The old strict `0 > u32::MAX - 1` missed it.
        let baseline = u32::MAX - 1;
        let current = storage.current_tick();
        let tick = storage.changed_tick_for(entities[0]).unwrap();
        assert!(
            current.wrapping_sub(tick) < current.wrapping_sub(baseline),
            "post-wrap write must be detected against a pre-wrap baseline"
        );
    }
}
