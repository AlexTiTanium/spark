//! Generational entity identifiers and the allocator that manages them.
//!
//! An `Entity` is a lightweight, copyable handle — an `(index, generation)`
//! pair — that uniquely names a live object in the `World`. After the object
//! is destroyed, any stale copy of its handle is reliably detected as dead,
//! preventing use-after-free at the logical level without any raw pointers
//! or unsafe code.

/// A lightweight, copyable handle to a live object in the `World`.
///
/// # What it is
///
/// An `Entity` is a `(index, generation)` pair. The `index` is a slot number
/// into the allocator's bookkeeping arrays; the `generation` is a counter
/// bumped every time that slot is recycled. Together they form a unique name
/// that is valid for exactly one lifetime of one object.
///
/// # Why generations prevent use-after-free
///
/// When you `destroy` an entity, its slot's generation is bumped. Any old
/// `Entity` value still pointing at that slot now carries an outdated
/// generation number. `EntityAllocator::is_alive` compares the stored
/// generation against the handle's generation and returns `false` for all
/// stale handles — no matter how many times the slot was reused afterwards.
///
/// # How NOT to use
///
/// - Do not compare entities by `index` alone. Two entities can share an
///   index at different times. Always compare the full `Entity` value or
///   call `is_alive`.
/// - Do not retain `Entity` handles across `World::clear()`. Index space
///   is reset; previously valid handles become spuriously alive or dead.
///
/// # Examples
///
/// ```
/// use spark_ecs::entity::EntityAllocator;
///
/// let mut alloc = EntityAllocator::new();
/// let a = alloc.allocate();
/// let b = alloc.allocate();
///
/// assert_ne!(a, b);
/// assert!(alloc.is_alive(a));
/// assert!(alloc.is_alive(b));
/// ```
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Entity {
    /// Slot number into the allocator's `generation` array.
    pub index: u32,
    /// Monotonically increasing counter for this slot. Mismatches indicate
    /// a stale handle.
    pub generation: u32,
}

/// Allocates, recycles, and validates `Entity` handles.
///
/// # Logic
///
/// `allocate` pops the most-recently freed index off `free_list` (LIFO).
/// If the list is empty it appends a new entry to `generation` and uses
/// that fresh index instead. Either way it returns
/// `Entity { index, generation: generation[index] }`.
///
/// `destroy` bumps `generation[entity.index]` and pushes the index back
/// onto `free_list`. It panics immediately on a double-free (calling
/// `destroy` with a stale handle), which surfaces bugs early rather than
/// silently corrupting state.
///
/// # Memory layout
///
/// ```text
/// free_list:  [3, 7, 12]           ← recycled slot indices (LIFO stack)
/// generation: [0, 1, 0, 2, 0, …]   ← generation[i] for slot i
///              ^              ^
///              slot 0         slot N
/// ```
///
/// `is_alive` is a single array read + integer compare: O(1), no
/// allocations, no locking.
///
/// # Why it works
///
/// Destroying slot `i` bumps `generation[i]`. Every `Entity` returned by
/// `allocate` for slot `i` carries the current value of `generation[i]`
/// at allocation time. After destruction the stored value no longer matches
/// any handle issued before the bump, so `is_alive` correctly rejects all
/// of them — including if slot `i` is later reallocated and the bump
/// happens again.
///
/// # How NOT to use
///
/// - Do not call `destroy` twice on the same entity. The second call will
///   panic because the handle is already stale.
/// - Do not fabricate `Entity` values by hand (e.g. `Entity { index: 0,
///   generation: 999 }`). Only allocator-issued handles are valid.
///
/// # Examples
///
/// ```
/// use spark_ecs::entity::EntityAllocator;
///
/// let mut alloc = EntityAllocator::new();
/// let a = alloc.allocate();
///
/// alloc.destroy(a);
///
/// let b = alloc.allocate();          // reuses a's slot
/// assert_eq!(a.index, b.index);
/// assert_ne!(a.generation, b.generation);
/// assert!(!alloc.is_alive(a));
/// assert!(alloc.is_alive(b));
/// ```
#[derive(Debug, Default)]
pub struct EntityAllocator {
    /// Indices of slots that have been freed and are ready for reuse.
    free_list: Vec<u32>,
    /// `generation[i]` is the current generation of slot `i`. Starts at 0;
    /// bumped on every `destroy`.
    generation: Vec<u32>,
}

impl EntityAllocator {
    /// Creates an empty allocator with no live entities.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::entity::EntityAllocator;
    /// let alloc = EntityAllocator::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocates a new entity, reusing a freed slot when one is available.
    ///
    /// # Panics
    ///
    /// Panics if more than `u32::MAX` (≈ 4 billion) entities have been
    /// allocated without reuse. This limit is never reached in practice.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::entity::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e = alloc.allocate();
    /// assert!(alloc.is_alive(e));
    /// ```
    pub fn allocate(&mut self) -> Entity {
        if let Some(index) = self.free_list.pop() {
            Entity {
                index,
                generation: self.generation[index as usize],
            }
        } else {
            let index = u32::try_from(self.generation.len())
                .expect("entity count overflow: more than u32::MAX entities allocated");
            self.generation.push(0);
            Entity { index, generation: 0 }
        }
    }

    /// Destroys a live entity, recycling its slot for future allocations.
    ///
    /// # Panics
    ///
    /// Panics if `entity` is already dead (stale handle or double-free).
    /// This is intentional: double-frees are logic errors that should
    /// surface loudly rather than silently corrupt the generation counter.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::entity::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e = alloc.allocate();
    /// alloc.destroy(e);
    /// assert!(!alloc.is_alive(e));
    /// ```
    pub fn destroy(&mut self, entity: Entity) {
        assert!(
            self.is_alive(entity),
            "destroy called on a dead entity {entity:?} — double-free or stale handle",
        );
        self.generation[entity.index as usize] =
            self.generation[entity.index as usize].wrapping_add(1);
        self.free_list.push(entity.index);
    }

    /// Returns `true` if `entity` was issued by this allocator and has not
    /// yet been destroyed.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::entity::EntityAllocator;
    ///
    /// let mut alloc = EntityAllocator::new();
    /// let e = alloc.allocate();
    /// assert!(alloc.is_alive(e));
    /// alloc.destroy(e);
    /// assert!(!alloc.is_alive(e));
    /// ```
    #[must_use]
    pub fn is_alive(&self, entity: Entity) -> bool {
        let index = entity.index as usize;
        index < self.generation.len() && self.generation[index] == entity.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocate_returns_live_entity() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.allocate();
        assert!(alloc.is_alive(e));
    }

    #[test]
    fn destroy_makes_entity_dead() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.allocate();
        alloc.destroy(e);
        assert!(!alloc.is_alive(e));
    }

    #[test]
    fn slot_is_reused_with_bumped_generation() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        alloc.destroy(a);
        let b = alloc.allocate();

        assert_eq!(a.index, b.index, "slot must be reused");
        assert_ne!(a.generation, b.generation, "generation must be bumped");
        assert!(!alloc.is_alive(a), "stale handle must be dead");
        assert!(alloc.is_alive(b), "new handle must be live");
    }

    #[test]
    fn multiple_entities_are_independent() {
        let mut alloc = EntityAllocator::new();
        let a = alloc.allocate();
        let b = alloc.allocate();
        let c = alloc.allocate();

        alloc.destroy(b);

        assert!(alloc.is_alive(a));
        assert!(!alloc.is_alive(b));
        assert!(alloc.is_alive(c));
    }

    #[test]
    #[should_panic(expected = "double-free or stale handle")]
    fn double_destroy_panics() {
        let mut alloc = EntityAllocator::new();
        let e = alloc.allocate();
        alloc.destroy(e);
        alloc.destroy(e); // must panic
    }

    #[test]
    fn stale_handle_rejected_after_reallocation() {
        let mut alloc = EntityAllocator::new();
        let old = alloc.allocate();
        alloc.destroy(old);
        let _new = alloc.allocate(); // reuses old's slot
        assert!(!alloc.is_alive(old), "original handle must still be dead after reuse");
    }

    #[test]
    fn one_million_allocate_destroy_cycles() {
        let mut alloc = EntityAllocator::new();

        // Allocate an initial pool.
        let mut live: Vec<Entity> = (0..1_000).map(|_| alloc.allocate()).collect();

        for _ in 0..1_000 {
            // Destroy every entity and reallocate, checking invariants.
            let snapshot: Vec<Entity> = live.clone();
            for &e in &snapshot {
                alloc.destroy(e);
            }
            for &e in &snapshot {
                assert!(!alloc.is_alive(e), "destroyed entity must be dead");
            }
            live = (0..1_000).map(|_| alloc.allocate()).collect();
            for &e in &live {
                assert!(alloc.is_alive(e), "freshly allocated entity must be live");
            }
        }
    }
}
