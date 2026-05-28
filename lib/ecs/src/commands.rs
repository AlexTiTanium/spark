//! `Commands` — system-param for deferred entity / component / resource
//! mutations.
//!
//! Systems can't mutate the [`World`] structurally while iterating it.
//! If a system iterating `Query<&mut Position>` decides to spawn a new
//! entity (which means pushing into the `Position` storage), it would
//! be modifying the storage it's currently walking — the same family
//! of bug as mutating a [`Vec`] while iterating it.
//!
//! The fix is to *defer*. [`Commands`] queues every structural change
//! into a [`CommandQueue`]; the queue is flushed at fixed points where no
//! system holds borrows — after a stage's sequential systems, and at every
//! workload boundary. Writes become visible at those points, never
//! mid-iteration.
//!
//! Resource ops ([`Commands::insert_resource`] /
//! [`Commands::update_resource`]) ride the same queue, but for a different
//! reason: they touch resource storage, not component storage, so there's no
//! mutate-while-iterating hazard to dodge. They're deferred for *ordering and
//! visibility* — landing at the flush boundary alongside the structural edits.
//!
//! # Why `spawn` is the exception
//!
//! [`Commands::spawn`] allocates a fresh [`Entity`] *synchronously* and
//! returns an [`EntityCommands`] builder. Allocating a slot is a counter
//! bump on the [`EntityAllocator`] — no component storage is touched, so
//! there's nothing to defer. The component-insert operations the caller
//! chains after `spawn` are the deferred parts; the [`Entity`] handle
//! itself (reachable via [`id`](EntityCommands::id)) is real immediately
//! and usable inside the same system (`commands.spawn().id()` works,
//! and the id is valid for [`Commands::despawn`] in the same frame).
//!
//! [`Commands::entity`] is the complement: no allocation, no synchronous
//! world touch — it is a pure builder wrapping an existing handle, so all
//! of its `insert` / `remove` ops are deferred to the next flush.
//!
//! # Borrow choreography
//!
//! [`Commands`] borrows two cells on the world — `entities` and
//! `pending` — both *disjoint* from every component storage. That's
//! why a system can take both a `Query<&mut T>` (which borrows
//! `storage<T>`) and `Commands` (which borrows entities + pending) at the
//! same time without [`std::cell::RefCell`] panicking.

use std::cell::RefCell;

use crate::access::Access;
use crate::entity::{Entity, EntityAllocator};
use crate::system::SystemParam;
use crate::world::World;
use crate::{Component, Resource};

/// Boxed one-shot mutation applied to the [`World`] during
/// [`CommandQueue::flush`].
///
/// `FnOnce` because each op runs exactly once; `+ 'static` because the
/// closure outlives the system that queued it (it lives in the queue
/// until the next flush point).
pub(crate) type DeferredOp = Box<dyn FnOnce(&mut World) + 'static>;

/// FIFO queue of `DeferredOp` closures drained at each flush point by
/// [`Application::run_stage`](../../spark_core/struct.Application.html#method.run_stage)
/// — after a stage's sequential systems, and at every workload boundary.
///
/// Lives inside the [`World`] as a single [`RefCell<CommandQueue>`];
/// [`Commands`] is the public way to enqueue, [`flush`](Self::flush) is
/// the only way to drain. The queue is empty between flushes — every op
/// pushed since the last flush is consumed at the next.
///
/// # Examples
///
/// ```
/// use spark_ecs::{CommandQueue, Component, World};
///
/// #[derive(Component)]
/// struct Tag;
///
/// let mut world = World::new();
/// let entity = world.spawn().id();
///
/// let mut queue = CommandQueue::new();
/// queue.push(Box::new(move |w| { w.insert(entity, Tag); }));
/// assert!(!queue.is_empty());
///
/// queue.flush(&mut world);
/// assert!(queue.is_empty());
/// assert!(world.get::<Tag>(entity).is_some());
/// ```
#[derive(Default)]
pub struct CommandQueue {
    ops: Vec<DeferredOp>,
}

impl CommandQueue {
    /// Creates an empty queue.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::CommandQueue;
    ///
    /// let queue = CommandQueue::new();
    /// assert!(queue.is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes a deferred op onto the queue. Ops fire in push order
    /// during [`flush`](Self::flush).
    ///
    /// Most callers go through [`Commands`] / [`EntityCommands`] rather
    /// than constructing ops by hand; this method is the escape hatch
    /// for direct users of [`CommandQueue`] in tests and tools.
    pub fn push(&mut self, op: DeferredOp) {
        self.ops.push(op);
    }

    /// Returns `true` iff no ops are pending.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::CommandQueue;
    ///
    /// let queue = CommandQueue::new();
    /// assert!(queue.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// Drains every queued op into `world`, in push order.
    ///
    /// The loop is a safety net, not a hot path: a [`DeferredOp`] receives
    /// only `&mut World`, never a handle to *this* queue, so nothing an op
    /// does can repopulate `self.ops` — in practice the loop always exits
    /// after one pass. (It would catch a direct [`push`](Self::push) from a
    /// future op type that gained queue access.) Ops enqueued through a
    /// system's [`Commands`] parameter mid-flush land in the world's own
    /// pending queue instead, and are drained by [`World::flush_commands`]'s
    /// outer loop (see the inline note below).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{CommandQueue, Component, World};
    ///
    /// #[derive(Component)]
    /// struct Position(i32, i32);
    ///
    /// let mut world = World::new();
    /// let entity = world.spawn().id();
    /// let mut queue = CommandQueue::new();
    /// queue.push(Box::new(move |w| { w.insert(entity, Position(1, 2)); }));
    ///
    /// queue.flush(&mut world);
    /// assert_eq!(world.get::<Position>(entity).unwrap().0, 1);
    /// ```
    pub fn flush(&mut self, world: &mut World) {
        loop {
            let ops = std::mem::take(&mut self.ops);
            if ops.is_empty() {
                return;
            }
            for op in ops {
                op(world);
            }
            // Re-enter the loop only if an op enqueued more ops back
            // into *us*. Ops queued via `Commands` mid-flush land on
            // `world.pending` instead — those are the responsibility
            // of `World::flush_commands`' outer loop, not this one.
        }
    }
}

/// System parameter that queues deferred mutations on the [`World`].
///
/// `Commands` borrows two cells — the entity allocator (mutably, for
/// `spawn`) and the pending [`CommandQueue`] (mutably, for every
/// queued op). Both are *disjoint* from any component storage, so a
/// system can take `Commands` alongside `Query<&mut T>` for any `T`
/// without [`std::cell::RefCell`] panicking.
///
/// # Lifecycle
///
/// 1. The runner calls [`SystemParam::fetch`] — `Commands` captures
///    references to the two cells, no data is moved.
/// 2. The system queues spawns / inserts / removes / despawns, plus
///    resource inserts / updates. These deferred changes are *not yet
///    visible* to other systems.
/// 3. At the next flush point — after the stage's sequential systems, or
///    at a workload boundary — [`World::flush_commands`] drains the queue
///    into the world; the queued writes land all at once. Systems that run
///    after that flush see them (a same-stage workload, a later workload,
///    or any later stage).
///
/// # Examples
///
/// ```
/// use spark_ecs::{Commands, Component, IntoSystem, Query, World};
///
/// #[derive(Component)]
/// struct Position { x: f32, y: f32 }
/// #[derive(Component)]
/// struct Velocity { x: f32, y: f32 }
///
/// fn spawn_two(mut commands: Commands) {
///     commands.spawn()
///         .insert(Position { x: 0.0, y: 0.0 })
///         .insert(Velocity { x: 1.0, y: 0.0 });
///     commands.spawn()
///         .insert(Position { x: 5.0, y: 5.0 })
///         .insert(Velocity { x: -1.0, y: 0.0 });
/// }
///
/// let mut world = World::new();
/// let mut sys = IntoSystem::into_system(spawn_two);
/// sys(&world);
/// world.flush_commands();
///
/// let q = Query::<(&Position, &Velocity)>::from_world(&world);
/// assert_eq!(q.iter().count(), 2);
/// ```
pub struct Commands<'w> {
    entities: &'w RefCell<EntityAllocator>,
    queue: &'w RefCell<CommandQueue>,
}

impl Commands<'_> {
    /// Allocates a fresh [`Entity`] synchronously and returns an
    /// [`EntityCommands`] builder so the caller can chain
    /// `.insert(component)` queues.
    ///
    /// The returned handle is real *now* — call `.id()` to capture it
    /// and use it within the same system, or pass it to
    /// [`despawn`](Self::despawn). Components inserted via the builder
    /// don't show up until the next flush point (the post-sequential
    /// flush, or the next workload boundary).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, IntoSystem, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// fn build_chain(mut commands: Commands) {
    ///     // Sync id — the entity exists immediately.
    ///     let parent = commands.spawn().insert(Position(0.0, 0.0)).id();
    ///     // The handle is usable inside the same system.
    ///     let _ = parent;
    /// }
    ///
    /// let mut world = World::new();
    /// let mut sys = IntoSystem::into_system(build_chain);
    /// sys(&world);
    /// world.flush_commands();
    /// ```
    pub fn spawn(&mut self) -> EntityCommands<'_> {
        let entity = self.entities.borrow_mut().allocate();
        EntityCommands {
            entity,
            queue: self.queue,
        }
    }

    /// Queues a despawn for `entity`. Equivalent to
    /// [`World::despawn`] at the next flush point.
    ///
    /// Despawn is deferred — the entity is still alive (and its
    /// components readable) until the queue drains. Repeated
    /// `despawn(e)` calls for the same `e` collapse harmlessly: the
    /// second [`World::despawn`] is a no-op on a stale handle.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, IntoSystem, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// let mut world = World::new();
    /// let doomed = world.spawn().insert(Position(0.0, 0.0)).id();
    ///
    /// let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
    ///     commands.despawn(doomed);
    /// });
    /// sys(&world);
    /// assert!(world.is_alive(doomed));  // not yet — flush is what cleans it.
    ///
    /// world.flush_commands();
    /// assert!(!world.is_alive(doomed));
    /// ```
    pub fn despawn(&mut self, entity: Entity) {
        self.queue.borrow_mut().push(Box::new(move |world| {
            world.despawn(entity);
        }));
    }

    /// Returns an [`EntityCommands`] builder bound to an *existing*
    /// `entity`, so callers can queue `insert` / `remove` ops on an
    /// entity spawned earlier — in a prior frame, a prior system, or
    /// earlier in this same system via [`spawn`](Self::spawn).
    ///
    /// Unlike [`spawn`](Self::spawn), this allocates nothing and touches
    /// no world state at call time — it just wraps the handle. `entity`
    /// is not validated for liveness here; validation happens at flush,
    /// inside each queued op.
    ///
    /// # Despawn-then-mutate
    ///
    /// Ops queued against an entity that is no longer live when the queue
    /// flushes are **dropped silently** — no panic, no slot resurrection.
    /// This falls out of the [`World`] mutators, not bespoke logic:
    /// [`World::insert`] and [`World::remove`] both check liveness and
    /// return early on a stale handle. Because the queue is FIFO, a
    /// [`despawn`](Self::despawn) queued *before* a mutation on the same
    /// entity always fires first, so the later op finds a dead handle and
    /// dissolves.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, IntoSystem, World};
    ///
    /// #[derive(Component)]
    /// struct Frozen;
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().id();
    ///
    /// let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
    ///     commands.entity(e).insert(Frozen);
    /// });
    /// sys(&world);
    /// world.flush_commands();
    /// assert!(world.get::<Frozen>(e).is_some());
    /// ```
    pub fn entity(&mut self, entity: Entity) -> EntityCommands<'_> {
        EntityCommands {
            entity,
            queue: self.queue,
        }
    }

    /// Queues an `add_resource::<R>(value)` for the next flush — the deferred
    /// way to *create* (or replace) a resource from inside a system.
    ///
    /// A system can't reach `&mut World`, and [`ResMut<T>`](crate::ResMut)
    /// only borrows a resource that already exists. `insert_resource` is the
    /// one path to introduce a brand-new resource mid-frame; it lands at the
    /// flush point alongside every other queued command.
    ///
    /// Overwrite semantics match [`World::add_resource`]: queuing a resource
    /// of a type that already exists at flush **replaces** the old value.
    /// Last write wins — if two ops insert the same `R`, the later push wins
    /// (the queue is FIFO).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, IntoSystem, Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// let mut sys = IntoSystem::into_system(|mut commands: Commands| {
    ///     commands.insert_resource(Score(7));
    /// });
    /// sys(&world);
    /// assert!(world.get_resource::<Score>().is_none()); // deferred — not yet
    /// world.flush_commands();
    /// assert_eq!(world.resource::<Score>().0, 7);
    /// ```
    pub fn insert_resource<R: Resource>(&mut self, value: R) {
        self.queue.borrow_mut().push(Box::new(move |world| {
            world.add_resource(value);
        }));
    }

    /// Queues a mutation `f` to run against the resource of type `R` at the
    /// next flush, where `f` receives `&mut R`.
    ///
    /// Useful when a system can't take [`ResMut<R>`](crate::ResMut) directly —
    /// today, because it already holds a borrow that would collide with a live
    /// `ResMut<R>` (a second same-type `ResMut` panics with "already
    /// borrowed"); under the M4 scheduler, because its access set would
    /// conflict with another param. Either way the closure runs at the flush
    /// boundary, after the offending borrows are gone.
    ///
    /// # Panics
    ///
    /// Panics **at flush** if no resource of type `R` exists when `f` would
    /// run, matching [`World::resource_mut`]. `update_resource` mutates; it
    /// never creates — use [`insert_resource`](Self::insert_resource) to bring
    /// a resource into existence first.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, IntoSystem, Resource, World};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// let mut world = World::new();
    /// world.add_resource(Score(1));
    /// let mut sys = IntoSystem::into_system(|mut commands: Commands| {
    ///     commands.update_resource::<Score>(|s| s.0 += 10);
    /// });
    /// sys(&world);
    /// assert_eq!(world.resource::<Score>().0, 1); // deferred — not yet
    /// world.flush_commands();
    /// assert_eq!(world.resource::<Score>().0, 11);
    /// ```
    pub fn update_resource<R: Resource>(&mut self, f: impl FnOnce(&mut R) + 'static) {
        self.queue.borrow_mut().push(Box::new(move |world| {
            f(&mut world.resource_mut::<R>());
        }));
    }
}

/// Builder returned by [`Commands::spawn`] (freshly-allocated entity) and
/// [`Commands::entity`] (existing entity). Chains `.insert(component)` /
/// `.remove::<T>()` queues and exposes the bound [`Entity`] via
/// [`id`](Self::id).
///
/// # Examples
///
/// Both builder sources at work — `spawn()` for a fresh entity, and
/// `entity(e)` to mutate an existing one:
///
/// ```
/// use spark_ecs::{Commands, Component, IntoSystem, World};
///
/// #[derive(Component)]
/// struct Position(f32, f32);
/// #[derive(Component)]
/// struct Velocity(f32, f32);
///
/// let mut world = World::new();
/// // An entity that already exists before the system runs.
/// let existing = world.spawn().insert(Position(0.0, 0.0)).insert(Velocity(1.0, 0.0)).id();
///
/// let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
///     commands.spawn().insert(Position(5.0, 5.0)); // fresh entity
///     commands.entity(existing).remove::<Velocity>(); // mutate existing
/// });
/// sys(&world);
/// world.flush_commands();
///
/// // The freshly-spawned entity plus the original both carry Position.
/// assert_eq!(spark_ecs::Query::<&Position>::from_world(&world).iter().count(), 2);
/// // The queued remove landed: `existing` no longer has Velocity.
/// assert!(world.get::<Velocity>(existing).is_none());
/// ```
pub struct EntityCommands<'a> {
    entity: Entity,
    queue: &'a RefCell<CommandQueue>,
}

impl EntityCommands<'_> {
    /// Queues an `insert<T>(entity, value)` for the next flush.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, IntoSystem, Query, World};
    ///
    /// #[derive(Component)]
    /// struct Position(f32, f32);
    ///
    /// fn spawn_one(mut commands: Commands) {
    ///     commands.spawn().insert(Position(1.0, 2.0));
    /// }
    ///
    /// let mut world = World::new();
    /// let mut sys = IntoSystem::into_system(spawn_one);
    /// sys(&world);
    /// world.flush_commands();
    /// assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 1);
    /// ```
    // Builder-chain ergonomics: the common pattern is
    // `commands.spawn().insert(A).insert(B);` — discardable mid-chain,
    // so `#[must_use]` would just force a stray `let _ =`.
    #[allow(
        clippy::return_self_not_must_use,
        reason = "builder is deliberately discardable mid-chain"
    )]
    pub fn insert<T: Component>(self, value: T) -> Self {
        let entity = self.entity;
        self.queue.borrow_mut().push(Box::new(move |world| {
            world.insert(entity, value);
        }));
        self
    }

    /// Queues a `remove::<T>(entity)` for the next flush.
    ///
    /// Idempotent: at flush time, removing a `T` the entity doesn't have
    /// — never inserted, already removed, or the entity itself despawned
    /// — is a silent no-op. [`World::remove`] returns `None` on a missing
    /// component or a stale handle, and that `None` is discarded here:
    /// deferred ops can't hand a value back to the queuing system. A
    /// caller who needs the removed value must call [`World::remove`]
    /// directly against `&mut World`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, IntoSystem, World};
    ///
    /// #[derive(Component)]
    /// struct Stunned;
    ///
    /// let mut world = World::new();
    /// let e = world.spawn().insert(Stunned).id();
    ///
    /// let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
    ///     commands.entity(e).remove::<Stunned>();
    /// });
    /// sys(&world);
    /// world.flush_commands();
    /// assert!(world.get::<Stunned>(e).is_none());
    /// ```
    // Builder-chain ergonomics: discardable mid-chain, same rationale as
    // `insert` — `#[must_use]` would just force a stray `let _ =`. `remove`
    // also trips `must_use_candidate` (unlike `insert`) because it takes no
    // by-value component arg, so clippy reads it as a query worth keeping;
    // it's the same discardable builder, so we waive that too.
    #[allow(
        clippy::return_self_not_must_use,
        clippy::must_use_candidate,
        reason = "builder is deliberately discardable mid-chain"
    )]
    pub fn remove<T: Component>(self) -> Self {
        let entity = self.entity;
        self.queue.borrow_mut().push(Box::new(move |world| {
            world.remove::<T>(entity);
        }));
        self
    }

    /// Returns the [`Entity`] handle this builder names.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::cell::Cell;
    /// use std::rc::Rc;
    ///
    /// use spark_ecs::{Commands, IntoSystem, World};
    ///
    /// let captured: Rc<Cell<Option<_>>> = Rc::new(Cell::new(None));
    /// let sink = captured.clone();
    /// let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
    ///     // `.id()` is synchronous — the handle is real inside this system.
    ///     sink.set(Some(commands.spawn().id()));
    /// });
    ///
    /// let world = World::new();
    /// sys(&world);
    /// let id = captured.get().expect("spawn allocates before flush");
    /// // Live now, no `flush_commands()` needed for the entity itself.
    /// assert!(world.is_alive(id));
    /// ```
    #[must_use]
    pub fn id(&self) -> Entity {
        self.entity
    }
}

impl<'a> SystemParam for Commands<'a> {
    type Item<'w>
        = Commands<'w>
    where
        'a: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        'a: 'w,
    {
        Commands {
            entities: world.entities_cell(),
            queue: world.pending_cell(),
        }
    }
    fn collect_access(_access: &mut Access) {
        // Deliberately empty. `Commands` records *deferred* structural
        // edits into a queue (and the entity allocator) that `World`
        // applies at flush time — it never reads or writes component or
        // resource storage directly. So it conflicts with no system and
        // can share a batch with anything. The per-system command buffers
        // that make this race-free under M4 parallelism are a later
        // concern; the access model has nothing to record here.
    }
}

#[cfg(test)]
#[allow(
    clippy::needless_pass_by_value,
    reason = "test fns mirror real systems, which take SystemParam by value"
)]
mod tests {
    use super::*;
    use crate::system::IntoSystem;
    use crate::{Component, Query, ResMut, Resource};

    #[derive(Debug, PartialEq, Component)]
    struct Position(i32, i32);

    #[derive(Debug, PartialEq, Component)]
    struct Velocity(i32, i32);

    #[derive(Debug, PartialEq, Resource)]
    struct Score(u32);

    #[test]
    fn spawn_allocates_entity_synchronously() {
        // `spawn().id()` must return a usable Entity *before* flush.
        let world = World::new();
        let id_cell = std::rc::Rc::new(std::cell::Cell::new(None));
        let id_cell_clone = id_cell.clone();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            let id = commands.spawn().id();
            id_cell_clone.set(Some(id));
        });
        sys(&world);
        let id = id_cell.get().expect("spawn must allocate before flush");
        // Liveness is true *immediately* — allocator bumped, no flush
        // needed for the entity itself.
        assert!(world.is_alive(id));
    }

    #[test]
    fn insert_is_deferred_until_flush() {
        let mut world = World::new();
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.spawn().insert(Position(1, 2));
        });
        sys(&world);
        // Pre-flush: the entity is alive (sync allocate), but its
        // component isn't reachable yet — still queued.
        assert_eq!(
            Query::<&Position>::from_world(&world).iter().count(),
            0,
            "insert must not land before flush"
        );
        world.flush_commands();
        assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 1);
    }

    #[test]
    fn despawn_is_deferred_until_flush() {
        let mut world = World::new();
        let e = world.spawn().insert(Position(7, 7)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.despawn(e);
        });
        sys(&world);
        // Pre-flush: still alive.
        assert!(world.is_alive(e));
        world.flush_commands();
        // Post-flush: cleaned up.
        assert!(!world.is_alive(e));
        assert!(world.get::<Position>(e).is_none());
    }

    #[test]
    fn commands_coexists_with_query_in_one_system() {
        // Disjoint cells: Query<&Position> borrows the Position
        // storage; Commands borrows entities + pending. No RefCell
        // collision at runtime.
        let mut world = World::new();
        world.spawn().insert(Position(0, 0));
        world.spawn().insert(Position(10, 10));

        let mut sys = IntoSystem::into_system(|q: Query<&Position>, mut commands: Commands| {
            // Iterate and queue a sibling for every entity we see.
            // The original storage isn't mutated; new entities
            // appear after flush.
            let count = q.iter().count();
            for _ in 0..count {
                commands.spawn().insert(Position(99, 99));
            }
        });
        sys(&world);
        world.flush_commands();
        // Original 2 + 2 sibling spawns = 4.
        assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 4);
    }

    #[test]
    fn chained_inserts_all_flush_together() {
        let mut world = World::new();
        let id_cell = std::rc::Rc::new(std::cell::Cell::new(None));
        let id_cell_clone = id_cell.clone();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            let id = commands
                .spawn()
                .insert(Position(1, 2))
                .insert(Velocity(3, 4))
                .id();
            id_cell_clone.set(Some(id));
        });
        sys(&world);
        world.flush_commands();
        let id = id_cell.get().unwrap();
        assert_eq!(*world.get::<Position>(id).unwrap(), Position(1, 2));
        assert_eq!(*world.get::<Velocity>(id).unwrap(), Velocity(3, 4));
    }

    #[test]
    fn spawn_then_despawn_in_same_frame_round_trips() {
        // Sync spawn returns an id usable for an immediate despawn.
        let mut world = World::new();
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            let id = commands.spawn().insert(Position(0, 0)).id();
            commands.despawn(id);
        });
        sys(&world);
        world.flush_commands();
        // No entity survived the round trip.
        assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 0);
    }

    #[test]
    fn entity_insert_and_remove_on_existing_entity() {
        // `entity(e)` binds to a pre-existing entity; insert + remove both
        // land at flush, uniformly with the spawn()-fresh case.
        let mut world = World::new();
        let e = world.spawn().insert(Position(1, 2)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands
                .entity(e)
                .insert(Velocity(3, 4))
                .remove::<Position>();
        });
        sys(&world);
        // Deferred: pre-flush the original component is untouched.
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(1, 2));
        world.flush_commands();
        assert!(world.get::<Position>(e).is_none());
        assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(3, 4));
    }

    #[test]
    fn remove_on_entity_lacking_component_is_noop() {
        // `remove::<T>()` on an entity that lacks `T` is idempotent — no
        // panic, the entity and its other components survive. The `T`
        // storage *exists* (another entity has it), so this exercises the
        // storage-level "entity not present" path, not the missing-storage
        // path (see `remove_with_no_storage_ever_created_is_noop`).
        let mut world = World::new();
        let e = world.spawn().insert(Position(0, 0)).id();
        world.spawn().insert(Velocity(7, 7)); // forces Velocity storage to exist
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(e).remove::<Velocity>();
        });
        sys(&world);
        world.flush_commands();
        assert!(world.is_alive(e));
        assert!(world.get::<Position>(e).is_some());
    }

    #[test]
    fn despawn_then_mutate_drops_silently() {
        // FIFO: despawn fires first, so the later insert finds a dead
        // handle and dissolves — World::insert no-ops on a stale handle.
        let mut world = World::new();
        let e = world.spawn().insert(Position(5, 5)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.despawn(e);
            commands.entity(e).insert(Velocity(1, 1));
        });
        sys(&world);
        world.flush_commands();
        assert!(!world.is_alive(e));
        assert!(world.get::<Velocity>(e).is_none());
    }

    #[test]
    fn chained_insert_insert_remove() {
        // Ops fire in push order: insert(Position) -> insert(Velocity) ->
        // remove::<Position>() nets to "no Position"; Velocity survives.
        let mut world = World::new();
        let id_cell = std::rc::Rc::new(std::cell::Cell::new(None));
        let id_cell_clone = id_cell.clone();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            let id = commands
                .spawn()
                .insert(Position(1, 2))
                .insert(Velocity(3, 4))
                .remove::<Position>()
                .id();
            id_cell_clone.set(Some(id));
        });
        sys(&world);
        world.flush_commands();
        let id = id_cell.get().unwrap();
        assert!(world.get::<Position>(id).is_none());
        assert_eq!(*world.get::<Velocity>(id).unwrap(), Velocity(3, 4));
    }

    #[test]
    fn insert_then_despawn_sweeps_the_component() {
        // The reverse order from `despawn_then_mutate_drops_silently`:
        // the insert *does* fire (entity still live), then despawn's
        // storage sweep removes it. The component is never observable on
        // a dead entity — there is no window where it persists.
        let mut world = World::new();
        let e = world.spawn().insert(Position(5, 5)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(e).insert(Velocity(9, 9));
            commands.despawn(e);
        });
        sys(&world);
        world.flush_commands();
        assert!(!world.is_alive(e));
        assert!(world.get::<Velocity>(e).is_none());
    }

    #[test]
    fn queued_despawn_does_not_reuse_slot_for_same_frame_spawn() {
        // `despawn` is deferred, so `old`'s slot is still live (not on the
        // free-list) when a same-system `spawn` allocates — the new entity
        // therefore gets a *distinct* handle, never `old`'s slot. This
        // closes the same-frame half of the slot-reuse hazard: there is no
        // window where a queued op against `old` could collide with a fresh
        // tenant, because no fresh tenant can take `old`'s slot this frame.
        // (The cross-reuse case — a despawned slot genuinely reallocated,
        // then hit by a stale handle — is rejected by the generation check
        // and is covered at the `World` layer by
        // `stale_handle_after_slot_reuse_is_rejected`.)
        let mut world = World::new();
        let old = world.spawn().insert(Position(1, 1)).id();
        let id_cell = std::rc::Rc::new(std::cell::Cell::new(None));
        let id_cell_clone = id_cell.clone();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.despawn(old);
            // Sync allocate — must not reuse `old`'s still-live slot.
            let fresh = commands.spawn().id();
            assert_ne!(fresh.index, old.index, "fresh must not take old's slot");
            // A mutation queued against the dead-to-be `old` must drop, not
            // resurrect it.
            commands.entity(old).insert(Velocity(99, 99));
            id_cell_clone.set(Some(fresh));
        });
        sys(&world);
        world.flush_commands();
        let fresh = id_cell.get().unwrap();
        assert!(!world.is_alive(old), "old despawned");
        assert!(world.is_alive(fresh), "fresh survives");
        // The stale insert against `old` dropped silently.
        assert!(world.get::<Velocity>(old).is_none());
    }

    #[test]
    fn remove_with_no_storage_ever_created_is_noop() {
        // `remove::<T>()` where no entity ever had a `T` (the
        // ComponentStorage<T> doesn't exist) returns cleanly via the `?`
        // on the storage lookup — no panic, no storage materialised.
        let mut world = World::new();
        let e = world.spawn().insert(Position(0, 0)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(e).remove::<Velocity>();
        });
        sys(&world);
        world.flush_commands();
        assert!(world.is_alive(e));
        assert!(world.get::<Position>(e).is_some());
    }

    #[test]
    fn remove_then_reinsert_same_type_nets_to_new_value() {
        // The reverse FIFO of `entity_insert_and_remove_on_existing_entity`:
        // remove fires first (clears the old `Position`), then insert fires
        // and reattaches a new value. Last write wins.
        let mut world = World::new();
        let e = world.spawn().insert(Position(1, 2)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(e).remove::<Position>();
            commands.entity(e).insert(Position(9, 9));
        });
        sys(&world);
        world.flush_commands();
        assert_eq!(*world.get::<Position>(e).unwrap(), Position(9, 9));
    }

    #[test]
    fn despawn_then_remove_drops_silently() {
        // Companion to `despawn_then_mutate_drops_silently`, which covers
        // the *insert* path: this exercises the *remove* path's own
        // `World::remove` is_alive guard against a stale handle.
        let mut world = World::new();
        let e = world.spawn().insert(Position(5, 5)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.despawn(e);
            commands.entity(e).remove::<Position>();
        });
        sys(&world);
        world.flush_commands(); // must not panic
        assert!(!world.is_alive(e));
    }

    #[test]
    fn entity_on_never_allocated_handle_drops_silently() {
        // `entity()` does not validate liveness — a fabricated handle the
        // allocator never issued must still flush cleanly, with every
        // queued op dropped by the World mutators' is_alive guards.
        let mut world = World::new();
        // Fields are `pub(crate)`, so a fabricated handle is constructible
        // from inside the crate's test module.
        let garbage = Entity {
            index: 9999,
            generation: 42,
        };
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(garbage).insert(Position(1, 1));
            commands.entity(garbage).remove::<Position>();
        });
        sys(&world);
        world.flush_commands(); // must not panic
        assert!(!world.is_alive(garbage));
        assert!(world.get::<Position>(garbage).is_none());
    }

    #[test]
    fn entity_stale_after_cross_frame_slot_reuse_drops_silently() {
        // The cross-frame companion to
        // `queued_despawn_does_not_reuse_slot_for_same_frame_spawn`: once a
        // despawn has actually flushed, the freed slot CAN be reallocated to
        // a new entity with a bumped generation. A later `commands.entity`
        // call on the *stale* handle must drop via the generation check in
        // `World::insert` / `World::remove` — never corrupting the new
        // tenant. (The bare-`World` form of this is
        // `stale_handle_after_slot_reuse_is_rejected` in world.rs; this pins
        // the deferred `Commands` path.)
        let mut world = World::new();
        let old = world.spawn().insert(Position(1, 1)).id();
        world.despawn(old);
        // Slot is now free; reallocate it for a fresh entity.
        let fresh = world.spawn().insert(Velocity(9, 9)).id();
        assert_eq!(old.index, fresh.index, "fresh reuses old's slot");
        assert_ne!(old, fresh, "but with a bumped generation");

        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(old).insert(Position(99, 99)); // stale — must drop
            commands.entity(old).remove::<Velocity>(); // stale — must drop
        });
        sys(&world);
        world.flush_commands(); // must not panic, must not corrupt `fresh`

        assert!(world.is_alive(fresh));
        // `fresh`'s Velocity is untouched by the stale remove.
        assert_eq!(*world.get::<Velocity>(fresh).unwrap(), Velocity(9, 9));
        // The stale insert did not land on the reused slot.
        assert!(world.get::<Position>(fresh).is_none());
    }

    #[test]
    fn remove_is_deferred_until_flush() {
        // Mirrors `insert_is_deferred_until_flush` / `despawn_is_deferred_
        // until_flush`: a standalone `remove` is queued, not applied, until
        // the flush point — the component stays readable in between.
        let mut world = World::new();
        let e = world.spawn().insert(Position(3, 4)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(e).remove::<Position>();
        });
        sys(&world);
        // Pre-flush: still queued, component readable.
        assert!(
            world.get::<Position>(e).is_some(),
            "remove must not land before flush"
        );
        world.flush_commands();
        assert!(world.get::<Position>(e).is_none());
    }

    #[test]
    fn remove_then_despawn_entity_is_dead_post_flush() {
        // Mirror of `insert_then_despawn_sweeps_the_component` and the
        // reverse of `despawn_then_remove_drops_silently`: the remove fires
        // first (strips the component), then despawn fires and must still
        // kill the live entity, sweeping the already-absent storage entry
        // without trouble.
        let mut world = World::new();
        let e = world.spawn().insert(Position(5, 5)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.entity(e).remove::<Position>();
            commands.despawn(e);
        });
        sys(&world);
        world.flush_commands();
        assert!(!world.is_alive(e));
        assert!(world.get::<Position>(e).is_none());
    }

    #[test]
    fn insert_resource_is_deferred_until_flush() {
        // Mirrors `insert_is_deferred_until_flush`: the resource is created
        // by `World::add_resource` at the flush point, not when queued.
        let mut world = World::new();
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.insert_resource(Score(7));
        });
        sys(&world);
        assert!(
            world.get_resource::<Score>().is_none(),
            "insert_resource must not land before flush"
        );
        world.flush_commands();
        assert_eq!(*world.resource::<Score>(), Score(7));
    }

    #[test]
    fn insert_resource_overwrites_existing() {
        // Overwrite semantics inherited from `World::add_resource`: a queued
        // insert of a type that already exists replaces the old value.
        let mut world = World::new();
        world.add_resource(Score(1));
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.insert_resource(Score(99));
        });
        sys(&world);
        world.flush_commands();
        assert_eq!(*world.resource::<Score>(), Score(99));
    }

    #[test]
    fn update_resource_runs_at_flush() {
        // `f` receives `&mut R` and runs at the flush boundary.
        let mut world = World::new();
        world.add_resource(Score(1));
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.update_resource::<Score>(|s| s.0 += 10);
        });
        sys(&world);
        assert_eq!(
            *world.resource::<Score>(),
            Score(1),
            "update_resource must not land before flush"
        );
        world.flush_commands();
        assert_eq!(*world.resource::<Score>(), Score(11));
    }

    #[test]
    #[should_panic(expected = "has not been inserted")]
    fn update_resource_on_absent_resource_panics_at_flush() {
        // Missing-resource contract: `update_resource` mutates, never creates.
        // At flush, `World::resource_mut` panics on the absent `Score`.
        let mut world = World::new();
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.update_resource::<Score>(|s| s.0 += 1);
        });
        sys(&world);
        world.flush_commands(); // panics here, not at queue time
    }

    #[test]
    fn two_queued_inserts_of_same_resource_last_write_wins() {
        // Doc contract (insert_resource): "if two ops insert the same `R`,
        // the later push wins (the queue is FIFO)." Pin it directly — the
        // overwrite test only queues one op, so it can't catch a future
        // same-type coalesce/first-wins regression.
        let mut world = World::new();
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.insert_resource(Score(1));
            commands.insert_resource(Score(2));
        });
        sys(&world);
        world.flush_commands();
        assert_eq!(*world.resource::<Score>(), Score(2));
    }

    #[test]
    fn update_resource_dodges_live_resmut_borrow() {
        // The method's raison d'être: a system holding a live `ResMut<Score>`
        // can still queue an update of the *same* type without tripping the
        // RefCell "already borrowed" panic — because the closure calls
        // `resource_mut` only inside the deferred op, which runs at flush
        // after the `ResMut` guard has dropped. Guards against a regression
        // that eagerly borrows at queue time.
        let mut world = World::new();
        world.add_resource(Score(1));
        let mut sys = IntoSystem::into_system(|mut r: ResMut<Score>, mut commands: Commands| {
            r.0 = 5; // eager write through the live borrow
            commands.update_resource::<Score>(|s| s.0 += 100); // deferred, same type
        });
        sys(&world); // must NOT panic with "already borrowed"
        assert_eq!(
            *world.resource::<Score>(),
            Score(5),
            "eager ResMut write lands immediately"
        );
        world.flush_commands();
        assert_eq!(
            *world.resource::<Score>(),
            Score(105),
            "deferred update applies against the flushed value"
        );
    }

    #[test]
    fn resource_and_structural_ops_share_one_flush() {
        // The module header's headline: resource ops "ride the same queue ...
        // alongside the structural edits." Pin that a single drain applies
        // both kinds, in push order.
        let mut world = World::new();
        world.add_resource(Score(1));
        let doomed = world.spawn().insert(Position(0, 0)).id();
        let mut sys = IntoSystem::into_system(move |mut commands: Commands| {
            commands.spawn().insert(Position(1, 2));
            commands.insert_resource(Score(5));
            commands.update_resource::<Score>(|s| s.0 += 1);
            commands.despawn(doomed);
        });
        sys(&world);
        // Nothing landed before flush.
        assert!(world.is_alive(doomed));
        assert_eq!(*world.resource::<Score>(), Score(1));
        world.flush_commands();
        // Structural ops applied: doomed gone, fresh entity present.
        assert!(!world.is_alive(doomed));
        assert_eq!(Query::<&Position>::from_world(&world).iter().count(), 1);
        // Resource ops applied in FIFO: insert(5) then +1 = 6.
        assert_eq!(*world.resource::<Score>(), Score(6));
    }

    #[test]
    fn insert_then_update_resource_in_one_system() {
        // FIFO across resource ops: insert creates `Score(5)`, then the
        // queued update sees it and bumps it — both apply at the same flush.
        let mut world = World::new();
        let mut sys = IntoSystem::into_system(|mut commands: Commands| {
            commands.insert_resource(Score(5));
            commands.update_resource::<Score>(|s| s.0 *= 2);
        });
        sys(&world);
        world.flush_commands();
        assert_eq!(*world.resource::<Score>(), Score(10));
    }
}
