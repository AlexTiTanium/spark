//! Bevy-style system-parameter machinery.
//!
//! A *system* is a plain Rust function whose parameters describe what it
//! reads and writes — [`Res<T>`] borrows a resource immutably,
//! [`ResMut<T>`] borrows it mutably. The [`SystemParam`] trait teaches
//! [`World`] how to *fetch* each parameter; the [`IntoSystem`] trait
//! wraps any fn whose params are all `SystemParam` into a uniform
//! `Box<dyn FnMut(&World)>` that the engine can store next to other
//! systems regardless of their signatures.
//!
//! # The marker-type trick
//!
//! `IntoSystem<Marker>` carries a phantom `Marker` (a tuple of param
//! types) used only to disambiguate the impls — Rust would otherwise see
//! every arity as overlapping. The user never names `Marker`; the
//! compiler infers it from the function's parameters.
//!
//! # The `for<'w>` higher-ranked bound
//!
//! Each impl bounds the user's fn with
//! `for<'w> FnMut(<P as SystemParam>::Item<'w>, …)` — read as "for any
//! choice of `'w`, `F` implements `FnMut` with these param types". The
//! wrapper picks the actual `'w` at call time from the `&World`
//! argument, so the user's fn must accept *whatever* lifetime the
//! borrow ends up with.
//!
//! # The one macro
//!
//! `IntoSystem` is implemented for arities 0..=4 via a single
//! `macro_rules!` expansion ([`impl_into_system`]). One body to read;
//! arity is just the macro call list.

use std::cell::{Ref, RefMut};
use std::ops::{Deref, DerefMut};

use crate::{Access, Resource, World};

/// Trait teaching [`World`] how to fetch a single system parameter.
///
/// Implementors define a [generic associated type][gat] `Item<'w>` —
/// the concrete value passed to the user's fn, with its lifetime tied
/// to the world borrow. Today's implementors are [`Res<T>`] (shared
/// resource borrow), [`ResMut<T>`] (exclusive resource borrow),
/// [`Query`](crate::Query) (entity iteration), [`Commands`] (deferred structural
/// mutation), and [`EventReader`](crate::EventReader) /
/// [`EventWriter`](crate::EventWriter) (frame-decoupled events); per-system
/// `Local<T>` state lands in a later milestone. Each also declares its reads
/// and writes via [`collect_access`], which the scheduler folds into a
/// per-system [`Access`].
///
/// [`Commands`]: crate::Commands
/// [`collect_access`]: SystemParam::collect_access
///
/// [gat]: https://blog.rust-lang.org/2022/10/28/gats-stabilization.html
///
/// # Examples
///
/// ```
/// use spark_ecs::{Res, Resource, SystemParam, World};
///
/// #[derive(Resource)]
/// struct Score(u32);
///
/// let mut world = World::new();
/// world.add_resource(Score(7));
/// let r: Res<'_, Score> = <Res<'_, Score> as SystemParam>::fetch(&world);
/// assert_eq!(r.0, 7);
/// ```
pub trait SystemParam {
    /// The concrete value handed to the user's system fn. Carries the
    /// borrow lifetime `'w` taken from the `&World` argument.
    ///
    /// The `Self: 'w` bound is the GAT well-formedness rule modern Rust
    /// asks for on every lifetime-generic associated type. `Res<'a, T>`
    /// and `ResMut<'a, T>` thread this through as `'a: 'w`;
    /// `Query<'_, D>` does it as `D: 'w`.
    type Item<'w>
    where
        Self: 'w;

    /// Builds an `Item<'w>` from a shared world borrow. Called once per
    /// param every time the wrapped system runs. The explicit `'w`
    /// makes the world's borrow lifetime nameable in every impl's
    /// where clause, which is what lets the `Self: 'w` GAT bound
    /// resolve cleanly per call site.
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        Self: 'w;

    /// Folds this parameter's reads and writes into `access`.
    ///
    /// `Res<T>` adds a resource read, `ResMut<T>` a resource write, and
    /// `Query<D, F>` adds component reads/writes; `Commands` adds nothing
    /// because it mutates structure through a deferred queue, not
    /// component or resource storage. There is **no default** on purpose:
    /// every parameter must declare its access explicitly, since a
    /// silently-empty set would let the scheduler batch a real conflict
    /// onto parallel threads at M4 — an unsound foundation. A param with
    /// genuinely no access (like `Commands`) writes an empty body and
    /// says so in a comment.
    fn collect_access(access: &mut Access);
}

/// Immutable borrow of a resource of type `T`. Created by the system
/// runner via [`SystemParam::fetch`].
///
/// # Examples
///
/// ```
/// use spark_ecs::{Res, Resource, World};
///
/// #[derive(Resource)]
/// struct EngineVersion(&'static str);
///
/// let mut world = World::new();
/// world.add_resource(EngineVersion("0.1.0"));
/// let v: Res<'_, EngineVersion> = Res::from_world(&world);
/// assert_eq!(v.0, "0.1.0");
/// ```
pub struct Res<'w, T: Resource>(Ref<'w, T>);

impl<'w, T: Resource> Res<'w, T> {
    /// Fetches a `Res<T>` directly from a [`World`]. Convenience for
    /// tests and doc examples; system fns receive their `Res<T>` from
    /// the runner.
    ///
    /// # Panics
    ///
    /// Panics if no resource of type `T` has been inserted, or if the
    /// resource is currently mutably borrowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Res, Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct A(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(A(42));
    /// assert_eq!(Res::<A>::from_world(&world).0, 42);
    /// ```
    #[must_use]
    pub fn from_world(world: &'w World) -> Self {
        Self(world.resource::<T>())
    }
}

impl<T: Resource> Deref for Res<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'a, T: Resource> SystemParam for Res<'a, T> {
    type Item<'w>
        = Res<'w, T>
    where
        'a: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        'a: 'w,
    {
        Res(world.resource::<T>())
    }
    fn collect_access(access: &mut Access) {
        access.add_resource_read::<T>();
    }
}

/// Mutable borrow of a resource of type `T`. Created by the system
/// runner via [`SystemParam::fetch`].
///
/// # Examples
///
/// ```
/// use spark_ecs::{ResMut, Resource, World};
///
/// #[derive(Resource)]
/// struct GameTime { frame: u64 }
///
/// let mut world = World::new();
/// world.add_resource(GameTime { frame: 0 });
/// {
///     let mut t: ResMut<'_, GameTime> = ResMut::from_world(&world);
///     t.frame = 5;
/// }
/// assert_eq!(world.resource::<GameTime>().frame, 5);
/// ```
pub struct ResMut<'w, T: Resource>(RefMut<'w, T>);

impl<'w, T: Resource> ResMut<'w, T> {
    /// Fetches a `ResMut<T>` directly from a [`World`]. Convenience for
    /// tests and doc examples; system fns receive their `ResMut<T>`
    /// from the runner.
    ///
    /// # Panics
    ///
    /// Panics if no resource of type `T` has been inserted, or if the
    /// resource is already borrowed (shared or exclusive).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{ResMut, Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct A(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(A(1));
    /// ResMut::<A>::from_world(&world).0 = 2;
    /// assert_eq!(world.resource::<A>().0, 2);
    /// ```
    #[must_use]
    pub fn from_world(world: &'w World) -> Self {
        Self(world.resource_mut::<T>())
    }
}

impl<T: Resource> Deref for ResMut<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Resource> DerefMut for ResMut<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl<'a, T: Resource> SystemParam for ResMut<'a, T> {
    type Item<'w>
        = ResMut<'w, T>
    where
        'a: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        'a: 'w,
    {
        ResMut(world.resource_mut::<T>())
    }
    fn collect_access(access: &mut Access) {
        access.add_resource_write::<T>();
    }
}

/// Wraps a function whose parameters are all [`SystemParam`] into a
/// uniform `Box<dyn FnMut(&World)>`. The `Marker` type parameter is a
/// phantom tuple of param types used only to disambiguate the impls;
/// the user never names it — Rust infers it from the fn signature.
///
/// Implemented for arities 0..=4. A fn with more parameters needs a
/// new arity row in `impl_into_system` (or, eventually, a system
/// builder API).
///
/// # Examples
///
/// ```
/// use spark_ecs::{IntoSystem, ResMut, Resource, World};
///
/// #[derive(Resource)]
/// struct Counter(u32);
///
/// fn tick(mut c: ResMut<Counter>) {
///     c.0 += 1;
/// }
///
/// let mut world = World::new();
/// world.add_resource(Counter(0));
/// let mut system = IntoSystem::into_system(tick);
/// system(&world);
/// system(&world);
/// assert_eq!(world.resource::<Counter>().0, 2);
/// ```
pub trait IntoSystem<Marker>: Sized {
    /// Boxes the system fn so it can be stored alongside others with
    /// different param signatures.
    fn into_system(self) -> Box<dyn FnMut(&World) + 'static>;

    /// The aggregated read/write set of this system — the union of every
    /// parameter's [`SystemParam::collect_access`].
    ///
    /// Takes no `self` because access is fixed by the parameter *types*,
    /// not any runtime value: `fn(Res<Dt>, Query<&mut Pos>)` always reads
    /// `Dt` and writes `Pos`. The scheduler calls this once at
    /// registration to build each system's [`Access`], then never again.
    fn access() -> Access;
}

/// Emits one `IntoSystem` impl per arity. The body fetches each param
/// from `&World`, then forwards them to the wrapped fn — see the
/// module-level docs for the marker-type and `for<'w>` HRTB tricks
/// that make this work.
macro_rules! impl_into_system {
    ($($P:ident),*) => {
        impl<F $(, $P)*> IntoSystem<fn($($P,)*)> for F
        where
            $($P: SystemParam + 'static,)*
            // First bound anchors inference: from the user's fn
            // signature, Rust can match each parameter type against
            // the corresponding `$P` and pick a concrete `SystemParam`
            // impl (`Res<'_, T>` or `ResMut<'_, T>`).
            F: FnMut($($P,)*) + 'static,
            // Second bound is the actual calling convention used at
            // runtime: the wrapper passes `<$P>::Item<'w>` values out
            // of the world borrow. The `for<'w>` HRTB makes this work
            // for whatever lifetime the borrow ends up with.
            F: for<'w> FnMut($(<$P as SystemParam>::Item<'w>),*),
        {
            #[allow(non_snake_case, unused_variables, clippy::allow_attributes)]
            fn into_system(mut self) -> Box<dyn FnMut(&World) + 'static> {
                Box::new(move |world: &World| {
                    $(let $P = <$P as SystemParam>::fetch(world);)*
                    (self)($($P),*);
                })
            }

            // `mut` is unused at arity 0 (no params to fold in); allowed
            // here for the same reason `into_system` allows its unused
            // `world` at arity 0 — one macro body serves every arity.
            #[allow(unused_mut, clippy::allow_attributes)]
            fn access() -> Access {
                let mut access = Access::new();
                $(<$P as SystemParam>::collect_access(&mut access);)*
                access
            }
        }
    };
}

impl_into_system!();
impl_into_system!(P1);
impl_into_system!(P1, P2);
impl_into_system!(P1, P2, P3);
impl_into_system!(P1, P2, P3, P4);

#[cfg(test)]
#[allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    reason = "Test fns defined inside #[test] bodies live next to the assertions \
              they support, and SystemParam values like Res<T> are deliberately \
              passed by value to match how plugins write systems."
)]
mod tests {
    // Named-field structs deliberately. Inside this crate, the private
    // `.0` field of `Res` / `ResMut` is visible — so tuple-struct
    // resources would let `r.0` resolve to the wrapper's inner Ref
    // instead of the user value. External users (other crates, doc
    // tests) don't see the inner field and can use tuple structs
    // freely; we just have to be tidy here.
    use super::*;
    use crate::Resource;
    use std::rc::Rc;

    #[derive(Resource)]
    struct A {
        n: u32,
    }
    #[derive(Resource)]
    struct B {
        s: &'static str,
    }

    #[test]
    fn zero_arg_system_runs() {
        // FnMut closures with captured state prove the wrapper threads
        // state across invocations; a `Cell` is `'static` and cheap.
        let world = World::new();
        let counter = Rc::new(std::cell::Cell::new(0_u32));
        let counter_clone = counter.clone();
        let mut sys = IntoSystem::into_system(move || {
            counter_clone.set(counter_clone.get() + 1);
        });
        sys(&world);
        sys(&world);
        assert_eq!(counter.get(), 2);
    }

    #[test]
    fn one_resmut_param_visible_after_run() {
        let mut world = World::new();
        world.add_resource(A { n: 0 });
        fn bump(mut a: ResMut<A>) {
            a.n += 1;
        }
        let mut sys = IntoSystem::into_system(bump);
        sys(&world);
        sys(&world);
        assert_eq!(world.resource::<A>().n, 2);
    }

    #[test]
    fn two_param_mix_res_and_resmut() {
        let mut world = World::new();
        world.add_resource(A { n: 3 });
        world.add_resource(B { s: "" });
        fn copy(a: Res<A>, mut b: ResMut<B>) {
            b.s = if a.n == 3 { "ok" } else { "nope" };
        }
        let mut sys = IntoSystem::into_system(copy);
        sys(&world);
        assert_eq!(world.resource::<B>().s, "ok");
    }

    #[test]
    fn different_types_resmut_coexist_in_one_system() {
        let mut world = World::new();
        world.add_resource(A { n: 1 });
        world.add_resource(B { s: "x" });
        fn both(mut a: ResMut<A>, mut b: ResMut<B>) {
            a.n += 1;
            b.s = "y";
        }
        let mut sys = IntoSystem::into_system(both);
        sys(&world);
        assert_eq!(world.resource::<A>().n, 2);
        assert_eq!(world.resource::<B>().s, "y");
    }

    #[test]
    #[should_panic(expected = "already borrowed")]
    fn same_type_double_resmut_panics_at_runtime() {
        // Bypass `IntoSystem` here: identical-typed params trip Rust's
        // higher-ranked inference (it can't unify the two independent
        // lifetimes in a `fn` item back to a single `'w`). We exercise
        // the underlying runtime check directly — that's what the
        // panic-on-double-borrow guarantee actually rests on. M4's
        // scheduler will catch this at registration time and turn it
        // into a compile-friendlier error.
        let mut world = World::new();
        world.add_resource(A { n: 1 });
        let _a1 = <ResMut<'_, A> as SystemParam>::fetch(&world);
        let _a2 = <ResMut<'_, A> as SystemParam>::fetch(&world);
    }
}
