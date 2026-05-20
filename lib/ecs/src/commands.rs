//! `Commands` — system-param for deferred entity / component mutations.
//!
//! Systems can't mutate the [`World`] structurally while iterating it.
//! If a system iterating `Query<&mut Position>` decides to spawn a new
//! entity (which means pushing into the `Position` storage), it would
//! be modifying the storage it's currently walking — the same family
//! of bug as mutating a [`Vec`] while iterating it.
//!
//! The fix is to *defer*. [`Commands`] queues every structural change
//! into a [`CommandQueue`]; the queue is flushed *between* systems —
//! at the stage boundary — when no system holds borrows. Writes become
//! visible at predictable points, never mid-iteration.
//!
//! # Why `spawn` is the exception
//!
//! [`Commands::spawn`] returns an [`Entity`] *synchronously*. Allocating
//! a fresh slot is a counter bump on the [`EntityAllocator`] — no
//! component storage is touched, so there's nothing to defer. The
//! component-insert operations the caller chains after `spawn` are the
//! deferred parts; the [`Entity`] handle itself is real immediately
//! and usable inside the same system (`commands.spawn().id()` works,
//! and the id is valid for [`Commands::despawn`] in the same frame).
//!
//! # Borrow choreography
//!
//! [`Commands`] borrows two cells on the world — `entities` and
//! `pending` — both *disjoint* from every component storage. That's
//! why a system can take both a `Query<&mut T>` (which borrows
//! storage<T>) and `Commands` (which borrows entities + pending) at the
//! same time without [`std::cell::RefCell`] panicking.

use std::cell::RefCell;

use crate::Component;
use crate::entity::{Entity, EntityAllocator};
use crate::system::SystemParam;
use crate::world::World;

/// Boxed one-shot mutation applied to the [`World`] during
/// [`CommandQueue::flush`].
///
/// `FnOnce` because each op runs exactly once; `+ 'static` because the
/// closure outlives the system that queued it (it lives in the queue
/// until the next stage boundary).
pub(crate) type DeferredOp = Box<dyn FnOnce(&mut World) + 'static>;

/// FIFO queue of [deferred ops](DeferredOp) drained at stage
/// boundaries by [`Application::run_stage`](../../spark_core/struct.Application.html#method.run_stage).
///
/// Lives inside the [`World`] as a single [`RefCell<CommandQueue>`];
/// [`Commands`] is the public way to enqueue, [`flush`](Self::flush) is
/// the only way to drain. The queue is empty between flushes — every
/// op pushed during a stage is consumed before the next stage starts.
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
    /// Ops may themselves enqueue more ops (e.g. by constructing a
    /// fresh [`Commands`] mid-flush); this method loops until the queue
    /// settles, so callers don't need a second `flush` to catch
    /// cascading work.
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

/// System parameter that queues structural mutations on the [`World`].
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
/// 2. The system queues spawns / inserts / despawns. Component writes
///    are *not yet visible* to other systems.
/// 3. After every system in the stage finishes,
///    [`World::flush_commands`] drains the queue into the world — the
///    queued writes land all at once. The next stage's systems see
///    them.
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
    /// don't show up until the next stage's flush.
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
    /// [`World::despawn`] at the next stage flush.
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
}

/// Builder returned by [`Commands::spawn`]. Chains
/// `.insert(component)` queues and exposes the synchronously-allocated
/// [`Entity`] via [`id`](Self::id).
///
/// Holds a reference to the parent [`CommandQueue`] cell so its
/// `insert` calls reach the same queue every other [`Commands`]
/// operation uses.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Commands, Component, IntoSystem, World};
///
/// #[derive(Component)]
/// struct Position(f32, f32);
/// #[derive(Component)]
/// struct Velocity(f32, f32);
///
/// fn spawn_one(mut commands: Commands) {
///     commands.spawn()
///         .insert(Position(0.0, 0.0))
///         .insert(Velocity(1.0, 0.0));
/// }
///
/// let mut world = World::new();
/// let mut sys = IntoSystem::into_system(spawn_one);
/// sys(&world);
/// world.flush_commands();
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

    /// Returns the [`Entity`] handle this builder names.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, IntoSystem, World};
    ///
    /// fn spawn_and_capture(mut commands: Commands) {
    ///     let id = commands.spawn().id();
    ///     // `id` is a real Entity right now — usable inside this system.
    ///     let _ = id;
    /// }
    ///
    /// let mut world = World::new();
    /// let mut sys = IntoSystem::into_system(spawn_and_capture);
    /// sys(&world);
    /// world.flush_commands();
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
}

#[cfg(test)]
#[allow(
    clippy::needless_pass_by_value,
    reason = "test fns mirror real systems, which take SystemParam by value"
)]
mod tests {
    use super::*;
    use crate::Component;
    use crate::system::IntoSystem;

    #[derive(Debug, PartialEq, Component)]
    struct Position(i32, i32);

    #[derive(Debug, PartialEq, Component)]
    struct Velocity(i32, i32);

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
            crate::Query::<&Position>::from_world(&world).iter().count(),
            0,
            "insert must not land before flush"
        );
        world.flush_commands();
        assert_eq!(
            crate::Query::<&Position>::from_world(&world).iter().count(),
            1
        );
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

        let mut sys =
            IntoSystem::into_system(|q: crate::Query<&Position>, mut commands: Commands| {
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
        assert_eq!(
            crate::Query::<&Position>::from_world(&world).iter().count(),
            4
        );
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
        assert_eq!(
            crate::Query::<&Position>::from_world(&world).iter().count(),
            0
        );
    }
}
