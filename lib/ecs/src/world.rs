//! The [`World`] resource container.
//!
//! Holds one value per type, keyed by [`TypeId`]. Read accessors land in
//! the next PR alongside `Res<T>` / `ResMut<T>` system params (#11);
//! today the only verb is [`World::add_resource`].

use std::any::{Any, TypeId};
use std::collections::HashMap;

/// Type-erased container that owns one value per `TypeId`.
///
/// `World` is the canonical home for long-lived engine state that needs
/// to survive across stages and frames — wgpu device, window handle, UI
/// context, game time. Each call to [`World::add_resource`] inserts the
/// value under `TypeId::of::<T>()`; a second insert of the same `T`
/// silently overwrites the first.
///
/// # Why no `Send`/`Sync` bound
///
/// `add_resource` takes any `T: 'static`, including `!Send` and `!Sync`
/// types (`winit::window::Window` on macOS, `Rc`, raw platform handles).
/// `World` itself is therefore `!Send + !Sync`. This is fine while the
/// scheduler is single-threaded; once parallel systems arrive in M4 the
/// API splits Bevy-style into `add_resource` (send) and
/// `add_non_send_resource` — additive on top of what is here today.
///
/// # Examples
///
/// ```
/// use spark_ecs::World;
///
/// struct GameTime { dt: f32 }
/// struct Score(u32);
///
/// let mut world = World::new();
/// world.add_resource(GameTime { dt: 0.016 });
/// world.add_resource(Score(0));
/// ```
#[derive(Default)]
pub struct World {
    resources: HashMap<TypeId, Box<dyn Any>>,
}

impl World {
    /// Creates an empty `World` with no resources.
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
        self.resources.insert(TypeId::of::<T>(), Box::new(value));
    }
}

#[cfg(test)]
impl World {
    /// Test-only helper. Read accessors are deferred to M4; until then
    /// tests assert on the underlying map's size to verify
    /// `add_resource` behaviour.
    fn len(&self) -> usize {
        self.resources.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct A(#[allow(dead_code)] u32);
    struct B(#[allow(dead_code)] &'static str);

    #[test]
    fn new_world_is_empty() {
        let world = World::new();
        assert_eq!(world.len(), 0);
    }

    #[test]
    fn add_resource_stores_one_value() {
        let mut world = World::new();
        world.add_resource(A(7));
        assert_eq!(world.len(), 1);
    }

    #[test]
    fn same_type_overwrites() {
        let mut world = World::new();
        world.add_resource(A(1));
        world.add_resource(A(2));
        assert_eq!(world.len(), 1, "second insert must replace, not append");
    }

    #[test]
    fn different_types_coexist() {
        let mut world = World::new();
        world.add_resource(A(1));
        world.add_resource(B("hello"));
        assert_eq!(world.len(), 2);
    }

    #[test]
    fn non_send_resource_compiles() {
        // `Rc<Cell<_>>` is `!Send + !Sync`. The `'static`-only bound on
        // `add_resource` must accept it.
        let counter = Rc::new(Cell::new(0_u32));
        let mut world = World::new();
        world.add_resource(counter);
        assert_eq!(world.len(), 1);
    }
}
