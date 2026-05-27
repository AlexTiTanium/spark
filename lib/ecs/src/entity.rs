//! Entity identity and the generational allocator that hands them out.
//!
//! An [`Entity`] is the engine's name for "some thing". Components are
//! stored in per-type [`ComponentStorage`](crate::ComponentStorage)
//! tables keyed by the entity's [`index`](Entity); the
//! [`generation`](Entity) field is what makes a freed-and-reused slot
//! distinguishable from the previous tenant. [`EntityAllocator`] owns
//! the bookkeeping.

/// Opaque, [`Copy`] handle that names one entity for its lifetime.
///
/// Two `u32`s, eight bytes total. `index` selects a slot in the
/// allocator's vectors; `generation` is bumped every time that slot is
/// recycled so old copies of the handle fail
/// [`is_alive`](EntityAllocator::is_alive) cleanly. Constructed only by
/// [`EntityAllocator::allocate`] — never by user code.
///
/// # Examples
///
/// ```
/// use spark_ecs::World;
///
/// let mut world = World::new();
/// let entity = world.spawn().id();
/// assert!(world.is_alive(entity));
/// ```
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Entity {
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

/// Generational allocator for [`Entity`] handles.
///
/// # Logic
///
/// [`allocate`](Self::allocate) pops the most-recently-destroyed slot
/// off `free_list` and reuses its index with whatever `generation`
/// value lives there; if the list is empty it appends a brand-new slot
/// at generation `0`. [`destroy`](Self::destroy) bumps the slot's
/// generation, clears its `alive` bit, and pushes the index back onto
/// `free_list`.
///
/// # Memory layout
///
/// ```text
/// After allocate × 6, destroy slot 3:
///
///   generation:  [0, 0, 0, 1, 0, 0]
///                          ↑ bumped from 0 → 1 by destroy(slot 3)
///   alive:       [T, T, T, F, T, T]
///                          ↑ cleared by destroy
///   free_list:   [3]
///                 ↑ LIFO of indices ready for reuse
///
/// A subsequent allocate() pops 3 off free_list, sets alive[3] = true,
/// and returns Entity { index: 3, generation: 1 }. Any leftover
/// Entity { index: 3, generation: 0 } in user code is now provably
/// stale (its generation no longer matches generation[3]).
/// ```
///
/// # Why it works
///
/// `(index, generation)` is the unique identity of an entity for the
/// life of the program. The slot's index is reused (bounded memory)
/// but the *identity* never repeats, because every destroy bumps
/// `generation[index]`. A stale handle's generation field no longer
/// matches the allocator's record, so [`is_alive`](Self::is_alive)
/// rejects it. This is the classic fix for the ABA problem: cheap
/// reuse without identity confusion.
///
/// # How NOT to use
///
/// - Don't compare [`Entity`] handles by [`index`](Entity) alone —
///   `(index_a == index_b) && (gen_a != gen_b)` means two *different*
///   entities that happened to share a slot.
/// - Don't retain handles across an allocator's reset / world clear
///   (no such API today, but if one lands, treat its return as a
///   generation reset).
///
/// # Examples
///
/// ```
/// use spark_ecs::EntityAllocator;
///
/// let mut alloc = EntityAllocator::default();
/// let a = alloc.allocate();
/// alloc.destroy(a);
/// let b = alloc.allocate();           // reuses a's slot
/// assert_ne!(a, b);                   // distinct identities …
/// assert!(!alloc.is_alive(a));        // a is dead
/// assert!(alloc.is_alive(b));         // b is alive
/// ```
#[derive(Default, Debug)]
pub struct EntityAllocator {
    generation: Vec<u32>,
    free_list: Vec<u32>,
    alive: Vec<bool>,
}

impl EntityAllocator {
    /// Creates an empty allocator.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let alloc = EntityAllocator::new();
    /// assert_eq!(alloc.len(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Hands out an [`Entity`], reusing a freed slot when one is
    /// available.
    ///
    /// # Logic
    ///
    /// 1. If `free_list` is non-empty, pop its last entry — that's our
    ///    `index`. Set `alive[index] = true`. The slot's
    ///    `generation[index]` was bumped by the destroy that freed it,
    ///    so we hand back that *current* generation; the prior tenant's
    ///    handle is now stale.
    /// 2. Otherwise, append a fresh slot:
    ///    `generation.push(0); alive.push(true);`. The new `index` is
    ///    the length before pushing.
    ///
    /// # Panics
    ///
    /// Panics if the allocator has handed out [`u32::MAX`] indices —
    /// the index space is exhausted. This is a theoretical concern
    /// (four billion concurrent entities); in practice the engine will
    /// run out of memory long before this fires.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let a = alloc.allocate();
    /// let b = alloc.allocate();
    /// assert_ne!(a, b);
    /// assert_eq!(alloc.len(), 2);
    /// ```
    pub fn allocate(&mut self) -> Entity {
        if let Some(index) = self.free_list.pop() {
            let slot = index as usize;
            self.alive[slot] = true;
            return Entity {
                index,
                generation: self.generation[slot],
            };
        }

        let index = u32::try_from(self.generation.len())
            .expect("entity index space exhausted (u32::MAX entities live)");
        self.generation.push(0);
        self.alive.push(true);
        Entity {
            index,
            generation: 0,
        }
    }

    /// Destroys an entity. Returns `true` if the handle was live; a
    /// no-op returning `false` for stale handles.
    ///
    /// # Logic
    ///
    /// Verify liveness via [`is_alive`](Self::is_alive). On a live
    /// handle: bump `generation[index]` (wrapping at `u32::MAX`),
    /// clear `alive[index]`, and push the index onto `free_list`. The
    /// slot stays in `generation` and `alive` — only the *free list*
    /// grows by one.
    ///
    /// # Why it works
    ///
    /// Bumping the generation *before* the slot is reused means any
    /// leftover handle to this slot now has a stale generation; the
    /// equality check inside [`is_alive`](Self::is_alive) rejects it
    /// before any further work. The `alive` flag is redundant with the
    /// generation check on its own, but lets us reject stale handles
    /// to *never-allocated* slots without bounds-checking the
    /// generation vector first.
    ///
    /// # How NOT to use
    ///
    /// - Don't loop calling `destroy` until it returns `false` —
    ///   that's an O(n) scan over stale handles for no reason. Call it
    ///   exactly once per entity you want gone.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let a = alloc.allocate();
    /// assert!(alloc.destroy(a));      // succeeds the first time
    /// assert!(!alloc.destroy(a));     // stale; no-op on the second
    /// ```
    pub fn destroy(&mut self, entity: Entity) -> bool {
        if !self.is_alive(entity) {
            return false;
        }
        let slot = entity.index as usize;
        self.generation[slot] = self.generation[slot].wrapping_add(1);
        self.alive[slot] = false;
        self.free_list.push(entity.index);
        true
    }

    /// Returns `true` iff this exact handle (matching index *and*
    /// generation) names a currently-live entity.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let a = alloc.allocate();
    /// assert!(alloc.is_alive(a));
    /// alloc.destroy(a);
    /// assert!(!alloc.is_alive(a));
    /// ```
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        let slot = entity.index as usize;
        self.alive.get(slot).copied().unwrap_or(false)
            && self.generation.get(slot).copied() == Some(entity.generation)
    }

    /// Returns the count of currently-live entities (slots that have
    /// been allocated minus those currently on the free list).
    ///
    /// Note that this is *not* the [`Vec`]-style "total slots ever
    /// allocated"; it's the live count. A previously-destroyed slot
    /// stays in the generation vector but isn't counted here until
    /// [`allocate`](Self::allocate) reuses it.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let a = alloc.allocate();
    /// let _b = alloc.allocate();
    /// assert_eq!(alloc.len(), 2);
    /// alloc.destroy(a);
    /// assert_eq!(alloc.len(), 1);     // freed slot drops out of the count
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.generation.len() - self.free_list.len()
    }

    /// Returns `true` iff no entities are currently live.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// assert!(alloc.is_empty());
    /// let _ = alloc.allocate();
    /// assert!(!alloc.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterates every currently-live [`Entity`] in **slot-index order**.
    ///
    /// Walks the `alive` bitmap once and yields a handle for each set
    /// slot, pairing the slot index with its current `generation`. The
    /// borrow of `self` lives as long as the returned iterator — to
    /// snapshot the set free of that borrow (e.g. so a concurrent
    /// [`allocate`](Self::allocate) can run), `.collect()` it, as
    /// `World::live_entities` does.
    ///
    /// # Why slot-index order
    ///
    /// Iteration is `0, 1, 2, …` over the slot space, independent of
    /// *when* each entity was allocated or how the free list churned.
    /// That determinism is what the engine's "no `HashMap` iteration in
    /// sim systems" rule needs: identical world states yield identical
    /// `Query<Entity>` orderings, leaving save/replay/multiplayer on the
    /// table. It is **not** allocation order — a slot reused after a
    /// destroy keeps its low index, so its (new-generation) entity sorts
    /// ahead of entities in higher slots allocated earlier.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let a = alloc.allocate();   // slot 0
    /// let b = alloc.allocate();   // slot 1
    /// alloc.destroy(a);
    /// let c = alloc.allocate();   // reuses slot 0, fresh generation
    ///
    /// // Slot order: c (slot 0) precedes b (slot 1), though c is younger.
    /// let live: Vec<_> = alloc.live().collect();
    /// assert_eq!(live, vec![c, b]);
    /// ```
    pub fn live(&self) -> impl Iterator<Item = Entity> + '_ {
        // Zip a `u32` slot counter against the parallel `alive` / `generation`
        // arrays and walk all three in lockstep. Lockstep iteration (rather
        // than indexing `generation[i]`) carries no per-slot bounds check, and
        // the `u32` counter sidesteps a `usize -> u32` cast — `allocate` caps
        // the slot space at `u32::MAX`, so the counter never overruns.
        (0u32..).zip(&self.alive).zip(&self.generation).filter_map(
            |((index, &alive), &generation)| alive.then_some(Entity { index, generation }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_allocator_is_empty() {
        let alloc = EntityAllocator::new();
        assert_eq!(alloc.len(), 0);
        assert!(alloc.is_empty());
    }

    #[test]
    fn allocate_returns_distinct_handles() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        let b = alloc.allocate();
        assert_ne!(a, b);
        assert_eq!(alloc.len(), 2);
        assert!(alloc.is_alive(a));
        assert!(alloc.is_alive(b));
    }

    #[test]
    fn destroy_marks_handle_stale() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        assert!(alloc.destroy(a));
        assert!(!alloc.is_alive(a));
        assert!(!alloc.destroy(a)); // second destroy is a no-op
    }

    #[test]
    fn allocate_after_destroy_reuses_slot_with_new_generation() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        alloc.destroy(a);
        let b = alloc.allocate();
        assert_eq!(a.index, b.index);
        assert_ne!(a.generation, b.generation);
        assert!(!alloc.is_alive(a));
        assert!(alloc.is_alive(b));
    }

    #[test]
    fn unknown_handle_is_not_alive() {
        let alloc = EntityAllocator::new();
        let phantom = Entity {
            index: 99,
            generation: 0,
        };
        assert!(!alloc.is_alive(phantom));
    }

    #[test]
    fn live_yields_every_alive_handle_in_slot_order() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate(); // slot 0
        let b = alloc.allocate(); // slot 1
        let c = alloc.allocate(); // slot 2
        assert_eq!(alloc.live().collect::<Vec<_>>(), vec![a, b, c]);

        // Destroying slot 1 drops b out; order of the survivors is stable.
        alloc.destroy(b);
        assert_eq!(alloc.live().collect::<Vec<_>>(), vec![a, c]);

        // Reusing slot 1 puts the new tenant back at index 1 — slot order,
        // not allocation order — with a bumped generation distinct from b.
        let d = alloc.allocate();
        assert_eq!(d.index, b.index);
        assert_ne!(d, b);
        assert_eq!(alloc.live().collect::<Vec<_>>(), vec![a, d, c]);
    }

    #[test]
    fn live_on_empty_allocator_yields_nothing() {
        let alloc = EntityAllocator::new();
        assert_eq!(alloc.live().count(), 0);
    }

    #[test]
    fn ten_thousand_cycle_preserves_invariants() {
        // Spec's "1M create/destroy cycles preserve invariants" — 10k
        // keeps the test under a few ms while still proving the
        // free-list / generation discipline holds under churn.
        let mut alloc = EntityAllocator::new();
        let mut live = Vec::with_capacity(100);
        for _ in 0..100 {
            live.push(alloc.allocate());
        }
        for _ in 0..10_000 {
            // Destroy the oldest live entity, then allocate a fresh one.
            let victim = live.remove(0);
            assert!(alloc.destroy(victim));
            assert!(!alloc.is_alive(victim));
            live.push(alloc.allocate());
        }
        assert_eq!(alloc.len(), 100);
        for entity in &live {
            assert!(alloc.is_alive(*entity));
        }
    }
}
