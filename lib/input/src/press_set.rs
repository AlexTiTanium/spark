//! [`PressSet`] — the held-plus-edges bookkeeping shared by keyboard keys and
//! mouse buttons.
//!
//! Keyboard and mouse-button state track exactly the same thing: which items
//! are held, which were pressed *this frame*, which were released this frame.
//! Rather than hand-roll that twice, both
//! [`KeyboardState`](crate::KeyboardState) and [`MouseState`](crate::MouseState)
//! store a `PressSet<T>` and delegate to it. The collection systems drive it
//! with three calls per frame: [`begin_frame`](PressSet::begin_frame), then
//! [`release_all`](PressSet::release_all) on focus loss, then
//! [`set`](PressSet::set) per event.
//!
//! # Why `Vec`, not `HashSet`
//!
//! Only a handful of keys/buttons are held at once, so linear scans beat
//! hashing — and `Vec` iteration is deterministic where `HashSet`'s is not.
//! Spark bans `HashSet` iteration in simulation code (it would make
//! saves/replays diverge), so a `Vec` sidesteps the hazard entirely.

/// A set of currently-held items plus this frame's press/release edges.
///
/// `T` is a small `Copy` id ([`KeyCode`](crate::KeyCode) /
/// [`MouseButton`](crate::MouseButton)). All three collections are `Vec`s in
/// insertion order; membership is a linear scan (see the module docs for why).
#[derive(Debug)]
pub(crate) struct PressSet<T> {
    held: Vec<T>,
    just_pressed: Vec<T>,
    just_released: Vec<T>,
}

// Hand-written `Default` so `T` needn't be `Default` — `KeyCode` / `MouseButton`
// aren't, and three empty `Vec`s need no bound, unlike `#[derive(Default)]`.
impl<T> Default for PressSet<T> {
    fn default() -> Self {
        Self {
            held: Vec::new(),
            just_pressed: Vec::new(),
            just_released: Vec::new(),
        }
    }
}

impl<T: Copy + PartialEq> PressSet<T> {
    /// Clears the per-frame edge sets. Call once at the top of each frame's
    /// collection, before applying that frame's events.
    pub(crate) fn begin_frame(&mut self) {
        self.just_pressed.clear();
        self.just_released.clear();
    }

    /// Applies one event: a press (`true`) or a release (`false`).
    pub(crate) fn set(&mut self, item: T, pressed: bool) {
        if pressed {
            self.press(item);
        } else {
            self.release(item);
        }
    }

    /// Releases everything held, recording each as a release edge.
    ///
    /// Used on focus loss: the OS delivers the real key-ups to whichever window
    /// took focus, so our held set would otherwise go stale. `mem::take` empties
    /// `held` in one move (a `drain` loop pushing into `just_released` wouldn't
    /// borrow-check — both are fields of `self`).
    pub(crate) fn release_all(&mut self) {
        let released = std::mem::take(&mut self.held);
        self.just_released.extend(released);
    }

    /// Whether `item` is currently held.
    pub(crate) fn is_held(&self, item: T) -> bool {
        self.held.contains(&item)
    }

    /// Whether `item` was pressed this frame.
    pub(crate) fn just_pressed(&self, item: T) -> bool {
        self.just_pressed.contains(&item)
    }

    /// Whether `item` was released this frame.
    pub(crate) fn just_released(&self, item: T) -> bool {
        self.just_released.contains(&item)
    }

    /// Iterates the currently-held items, in insertion order.
    pub(crate) fn iter_held(&self) -> impl Iterator<Item = T> + '_ {
        self.held.iter().copied()
    }

    /// Records a press. A duplicate press for an already-held item is ignored,
    /// so OS auto-repeat that slips past the window filter can't double-register.
    fn press(&mut self, item: T) {
        if !self.held.contains(&item) {
            self.held.push(item);
            self.just_pressed.push(item);
        }
    }

    /// Records a release. A release for an item that isn't held is ignored.
    ///
    /// Uses `Vec::remove` (not `swap_remove`) so the surviving items keep their
    /// insertion order — the order [`iter_held`](Self::iter_held) promises. The
    /// shift is O(n), but `held` holds only a handful of items, so it's free in
    /// practice.
    fn release(&mut self, item: T) {
        if let Some(i) = self.held.iter().position(|held| *held == item) {
            self.held.remove(i);
            self.just_released.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::PressSet;

    #[test]
    fn press_sets_held_and_edge() {
        let mut set = PressSet::default();
        set.set(1, true);
        assert!(set.is_held(1));
        assert!(set.just_pressed(1));
        assert!(!set.just_released(1));
    }

    #[test]
    fn duplicate_press_is_ignored() {
        let mut set = PressSet::default();
        set.set(1, true);
        set.set(1, true);
        assert_eq!(set.iter_held().filter(|x| *x == 1).count(), 1);
    }

    #[test]
    fn release_clears_held_and_records_edge() {
        let mut set = PressSet::default();
        set.set(1, true);
        set.set(1, false);
        assert!(!set.is_held(1));
        assert!(set.just_released(1));
    }

    #[test]
    fn release_of_unheld_item_is_a_noop() {
        let mut set = PressSet::default();
        set.set(7, false);
        assert!(!set.is_held(7));
        assert!(
            !set.just_released(7),
            "no edge for an item that was never held"
        );
    }

    #[test]
    fn begin_frame_clears_only_edges_not_held() {
        let mut set = PressSet::default();
        set.set(1, true);
        set.begin_frame();
        assert!(set.is_held(1), "held persists across frames");
        assert!(!set.just_pressed(1), "edge cleared by begin_frame");
    }

    #[test]
    fn release_all_moves_every_held_to_released() {
        let mut set = PressSet::default();
        set.set(1, true);
        set.set(2, true);
        set.release_all();
        assert_eq!(set.iter_held().count(), 0);
        assert!(set.just_released(1) && set.just_released(2));
    }

    #[test]
    fn press_then_release_same_frame_holds_nothing_but_records_both_edges() {
        let mut set = PressSet::default();
        set.set(1, true);
        set.set(1, false);
        assert!(!set.is_held(1));
        assert!(set.just_pressed(1) && set.just_released(1));
    }

    #[test]
    fn releasing_a_non_last_item_preserves_insertion_order() {
        // Regression guard: `swap_remove` would reorder the survivors here,
        // breaking the insertion-order contract `iter_held` documents.
        let mut set = PressSet::default();
        set.set(1, true);
        set.set(2, true);
        set.set(3, true);
        set.set(1, false); // release the *first*-pressed item
        assert_eq!(set.iter_held().collect::<Vec<_>>(), vec![2, 3]);
    }
}
