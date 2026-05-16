//! The [`World`] resource container.
//!
//! Holds one value per type, keyed by [`TypeId`], inside a [`RefCell`]
//! so that read and write accessors take `&self`. Interior mutability is
//! what lets two [`ResMut<T>`](crate::ResMut) over **different** `T`
//! coexist inside one system: both fetch through `&World`, so no
//! exclusive `&mut World` is held while the system runs.

use std::any::{Any, TypeId};
use std::cell::{Ref, RefCell, RefMut};
use std::collections::HashMap;

/// Type-erased container that owns one value per `TypeId`.
///
/// `World` is the canonical home for long-lived engine state that needs
/// to survive across stages and frames — wgpu device, window handle, UI
/// context, game time. Each call to [`World::add_resource`] inserts the
/// value under `TypeId::of::<T>()`; a second insert of the same `T`
/// silently overwrites the first. Read access goes through
/// [`World::get_resource`] / [`World::resource`] (and the `_mut`
/// variants), which return [`Ref`] / [`RefMut`] guards into the cell.
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
///
/// assert_eq!(world.resource::<Score>().0, 0);
/// world.resource_mut::<Score>().0 = 42;
/// assert_eq!(world.resource::<Score>().0, 42);
/// ```
#[derive(Default)]
pub struct World {
    resources: HashMap<TypeId, RefCell<Box<dyn Any>>>,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    struct A(u32);
    struct B(&'static str);

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
        // `Rc<Cell<_>>` is `!Send + !Sync`. The `'static`-only bound on
        // `add_resource` must accept it.
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
}
