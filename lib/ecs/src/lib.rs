#![doc = include_str!("../README.md")]

// Lets the derive-generated `::spark_ecs::Component` / `::spark_ecs::Resource`
// paths resolve when the derives are used *inside* this crate (its own
// unit tests). Downstream crates already see `spark_ecs` as a normal
// extern crate; this alias gives the in-crate use sites the same name.
extern crate self as spark_ecs;

mod access;
mod commands;
mod entity;
mod events;
mod filter;
mod query;
mod scheduler;
mod storage;
mod system;
mod workload;
mod world;

pub use access::{Access, ConflictKind, QueryAccess};
pub use commands::{CommandQueue, Commands, EntityCommands};
pub use entity::{Entity, EntityAllocator};
pub use events::{EventReader, EventWriter, Events, swap_events};
pub use filter::{Added, And, Changed, Or, QueryFilter, With, Without};
pub use query::{Query, QueryData, ReadOnlyQueryData};
pub use scheduler::{Schedule, WorkloadOrderBuilder};
pub use storage::{AnyStorage, ComponentStorage, Mut};
pub use system::{IntoSystem, Res, ResMut, SystemParam};
pub use workload::{
    IntoSystemTuple, SystemId, SystemOrderBuilder, SystemRef, WorkloadBuilder, WorkloadId,
    WorkloadLabel,
};
pub use world::{EntityMut, World};

// The trait and the derive macro deliberately share a name (`Component`
// is both `trait Component` here and the `#[derive(Component)]` macro
// re-exported below). They live in different namespaces — type vs macro
// — so one `use spark_ecs::Component;` pulls in both, exactly like
// `serde::Serialize`. `WorkloadLabel` follows the same dual-name pattern:
// the trait lives in `workload`, the derive in `spark-ecs-macros`; `Event`
// pairs the trait below with its derive.
pub use spark_ecs_macros::{Component, Event, Resource, WorkloadLabel};

/// Marker trait for types that attach to entities as components and are
/// matched by [`Query`].
///
/// # Explicit membership, by derive
///
/// A type is a component *only* if it opts in with
/// `#[derive(Component)]`. There is deliberately no blanket
/// `impl<T> Component for T` — that would make every `'static` type a
/// component, so a [`Resource`] would silently satisfy
/// [`Query`] and a stray `String` could be `insert`ed onto
/// an entity. The derive turns "this type is a component" into a
/// checked, intentional decision.
///
/// # Why `Send + Sync + 'static`
///
/// Components live in storages the M4 parallel scheduler will hand to
/// worker threads, so they must be safe to send and share across
/// threads. The bound is enforced *at the derive site*: a component
/// holding an `Rc`, `RefCell`, or raw pointer fails to compile right
/// there. That compile-time rejection is the thread-safety proof the
/// scheduler relies on — no runtime check needed. Inherently
/// non-thread-safe state belongs in a [`Resource`] instead, which
/// carries no `Send + Sync` bound.
///
/// # Examples
///
/// ```
/// use spark_ecs::Component;
///
/// #[derive(Component)]
/// struct Position {
///     x: f32,
///     y: f32,
/// }
///
/// #[derive(Component)]
/// struct Health(u32);
///
/// // Zero-sized marker components are components too.
/// #[derive(Component)]
/// struct Player;
/// ```
///
/// A type that isn't `Send + Sync` fails to derive — the bound is
/// checked right at the derive site, which is exactly the point:
///
/// ```compile_fail
/// use spark_ecs::Component;
/// use std::rc::Rc;
///
/// // `Rc<u32>` is `!Send + !Sync`, so this fails with
/// // "`Rc<u32>` cannot be sent between threads safely". Wrap
/// // non-thread-safe state in a `Resource` instead.
/// #[derive(Component)]
/// struct Bad(Rc<u32>);
/// ```
pub trait Component: Send + Sync + 'static {}

/// Marker trait for `World` singletons — one value per type, fetched
/// with [`Res<T>`](crate::Res) / [`ResMut<T>`](crate::ResMut).
///
/// # Explicit membership, by derive
///
/// Like [`Component`], a type is a resource only if it opts in with
/// `#[derive(Resource)]`. The two traits are distinct on purpose: a
/// component can't be passed to
/// [`add_resource`](crate::World::add_resource) and a resource can't be
/// `insert`ed onto an entity, so the two storage regions can't be
/// confused for one another.
///
/// # Why only `'static` — no `Send + Sync`
///
/// Resources are the engine's home for inherently non-thread-safe
/// state: a `wgpu` surface, an OS window handle, an `Rc`-based cache.
/// Forcing `Send + Sync` here would lock those out of the `World`
/// entirely. So `Resource` requires only `'static`, and parallel-safety
/// is enforced where it actually bites — at the scheduler. When the M4
/// parallel executor lands it will, like Bevy's `Resource` / `NonSend`
/// split, keep any system that touches a non-`Send` resource on the
/// main thread rather than scheduling it onto a worker. Until that
/// scheduler exists there is nothing to enforce, and adding the bound
/// now would only reject resources the single-threaded engine handles
/// perfectly well.
///
/// # Examples
///
/// ```
/// use spark_ecs::Resource;
///
/// #[derive(Resource)]
/// struct FrameCount(u64);
///
/// #[derive(Resource)]
/// struct GameTime {
///     dt: f32,
///     elapsed: f32,
/// }
/// ```
pub trait Resource: 'static {}

/// Marker trait for types that flow through an [`Events<T>`] queue and are
/// read/written with [`EventReader<T>`] / [`EventWriter<T>`].
///
/// # Explicit membership, by derive
///
/// Like [`Component`] and [`Resource`], a type is an event only if it opts
/// in with `#[derive(Event)]` — there is no blanket impl.
///
/// # Why `Send + Sync + 'static`
///
/// The same bound as [`Component`], and for the same reason: event buffers
/// live in storages the M4 parallel executor will hand to worker threads,
/// so events must be safe to send and share. Requiring `Send + Sync` now —
/// when it costs event authors essentially nothing — means M4 needs no
/// breaking change; relaxing the bound later would instead break every
/// event type at once. (A [`Resource`] is only `'static`; an `Events<T>`
/// being stricter than the resource it is stored as is fine.)
///
/// # Examples
///
/// ```
/// use spark_ecs::Event;
///
/// #[derive(Event)]
/// struct TileClicked {
///     x: i32,
///     y: i32,
/// }
///
/// // Zero-sized "something happened" events are events too.
/// #[derive(Event)]
/// struct ConstructionCompleted;
/// ```
pub trait Event: Send + Sync + 'static {}
