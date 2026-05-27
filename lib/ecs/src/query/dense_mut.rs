//! `DenseMut`: the crate's single `unsafe fn`, isolated to one file.
//!
//! Random-access mutable view into one storage's `dense` array, used by
//! every non-driver `&mut T` position in a tuple join to hand out `&mut T`
//! by entity. Splitting it out keeps the workspace's `unsafe_code = "warn"`
//! allowance pinned to the one type that genuinely needs it; the full
//! soundness argument lives on [`DenseMut::get`]'s `# Safety` block and the
//! module-level *Joins* docs in the parent [`query`](super) module.

use std::marker::PhantomData;

use crate::entity::Entity;
use crate::storage::Mut;

/// Mutable random-access view into one storage's `dense`, used by
/// non-driver `&mut T` positions in every tuple join (arities 2–5, and
/// the `(Entity, …)` family) to hand out `&mut T` by entity.
///
/// The chosen driver element walks its entities; this view is the
/// random-access lookup for each non-driver `&mut T` position, which needs a
/// raw `*mut T` + index (the price of leaving the borrow checker). The full
/// soundness argument lives on [`DenseMut::get`]'s `# Safety` block and the
/// module-level *Joins* docs.
///
/// `PhantomData<&'s mut [T]>` ties the view's lifetime to the
/// storage's exclusive borrow (so it cannot dangle) and gives `T` the
/// invariance that `&mut` requires.
///
/// `changed_ptr` + `current_tick` carry change detection through the
/// join: `get` builds a [`Mut`] over the matched slot, so a `&mut B` in
/// `Query<(&mut A, &mut B)>` marks **only** the entities the body writes,
/// never the rest of `B`'s storage. The pointer aliases the same
/// `dense_idx` space as `ptr` and obeys the same "fetched at most once"
/// contract, so the borrows never overlap.
pub(crate) struct DenseMut<'s, T> {
    ptr: *mut T,
    changed_ptr: *mut u32,
    current_tick: u32,
    len: usize,
    sparse: &'s [Option<u32>],
    entity_index: &'s [Entity],
    _marker: PhantomData<&'s mut [T]>,
}

// The whole point of `DenseMut` is to host one carefully-contracted
// `unsafe fn` (and one inner `unsafe` block) — scope the workspace's
// `unsafe_code = "warn"` lint allowance to this impl rather than
// relaxing it crate-wide.
#[allow(unsafe_code)]
impl<'s, T> DenseMut<'s, T> {
    /// Builds a view from the storage's exclusively-borrowed `dense` and
    /// `changed_tick` slices plus shared borrows of its `sparse` table and
    /// `entity_index`, and the component's `current_tick`. All at lifetime
    /// `'s`; converting `dense` / `changed_tick` to raw pointers consumes
    /// the unique borrows. `PhantomData<&'s mut [T]>` keeps `'s` tracked
    /// and `T` invariant; `changed_ptr` needs no separate phantom — it
    /// comes from the same `split_for_join(&mut self)` call on the same
    /// `RefMut`-guarded storage, so it shares `'s` and cannot dangle, and
    /// a `*mut u32` is already invariant in its pointee.
    /// `changed_tick.len() == dense.len()` by the `ComponentStorage`
    /// parallel-array invariant.
    pub(in crate::query) fn new(
        dense: &'s mut [T],
        changed_tick: &'s mut [u32],
        sparse: &'s [Option<u32>],
        entity_index: &'s [Entity],
        current_tick: u32,
    ) -> Self {
        Self {
            ptr: dense.as_mut_ptr(),
            changed_ptr: changed_tick.as_mut_ptr(),
            current_tick,
            len: dense.len(),
            sparse,
            entity_index,
            _marker: PhantomData,
        }
    }

    /// Returns a change-marking [`Mut`] handle to `entity`'s component, or
    /// `None` if this storage has no entry for `entity` (or holds a stale
    /// handle to a recycled slot — same generation check
    /// [`ComponentStorage::get_mut`] uses).
    ///
    /// Like [`ComponentStorage::iter_mut`], the returned `Mut` marks the
    /// component changed only when the caller writes through it — `get`
    /// itself stamps nothing.
    ///
    /// # Safety
    ///
    /// Across the whole lifetime `'s`, `get` must never be called twice
    /// with the same `entity`. Two `&mut T` to one dense slot would
    /// alias. See the module-level *Joins* docs for how the
    /// `(D1, &mut T)` tuple impls uphold this — structural driver
    /// shape plus [`QueryAccess::assert_no_self_conflict`] at query
    /// construction. The same once-per-entity contract makes the two raw
    /// borrows below sound: each slot (value + `changed_tick`) is handed
    /// out at most once across `'s`, so no live borrow overlaps.
    ///
    /// Also assumes `ComponentStorage`'s sparse/dense parallel-array
    /// invariant: every `Some(idx)` in `sparse` points at an in-bounds
    /// `dense` slot, and `changed_tick` is the same length as `dense`.
    /// The `debug_assert!` catches violations in tests; release builds
    /// trust the invariant — or rather, the bounds-checked
    /// `entity_index[dense_idx]` access below would panic first, so
    /// reaching the `ptr.add` proves the index is in bounds.
    pub(in crate::query) unsafe fn get(&self, entity: Entity) -> Option<Mut<'s, T>> {
        let dense_idx = (*self.sparse.get(entity.index as usize)?)? as usize;
        debug_assert!(
            dense_idx < self.len,
            "sparse/dense desync — ComponentStorage swap_remove invariant violated"
        );
        // Generation check + implicit bounds check. A stale `Entity`
        // whose `index` collides with a different live tenant returns
        // `None`, matching `ComponentStorage::get_mut`. The Vec
        // indexing is bounds-checked in release too, so reaching the
        // `ptr.add` below proves `dense_idx < entity_index.len() ==
        // dense.len() == changed_tick.len() == self.len`.
        if self.entity_index[dense_idx] != entity {
            return None;
        }
        // SAFETY: bounds for BOTH borrows:
        //   1. The `entity_index[dense_idx]` access above is bounds-checked,
        //      so `dense_idx < entity_index.len()`.
        //   2. The `ComponentStorage` parallel-array invariant keeps
        //      `entity_index.len() == dense.len() == changed_tick.len()`,
        //      and `DenseMut::new` set `self.len = dense.len()` and took
        //      `changed_tick` as a `&'s mut [u32]` of that same length —
        //      so both `ptr.add(dense_idx)` and `changed_ptr.add(dense_idx)`
        //      are in bounds.
        // Aliasing: by this fn's contract the entity is fetched at most
        // once across `'s` (the driver's linear scan visits each entity
        // once), so neither `&'s mut` below overlaps another live borrow.
        // The returned `Mut` stamps the tick only if the caller writes.
        let value = unsafe { &mut *self.ptr.add(dense_idx) };
        let changed = unsafe { &mut *self.changed_ptr.add(dense_idx) };
        Some(Mut::new(value, changed, self.current_tick))
    }
}
