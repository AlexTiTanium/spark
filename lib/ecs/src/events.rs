//! Double-buffered, frame-decoupled messaging between systems.
//!
//! An *event* is a value one system sends and others read a frame later.
//! [`Events<T>`] is a plain [`Resource`] holding two buffers: writers push
//! to `current`, readers iterate `previous`, and a per-type swap system
//! ([`swap_events`]) rotates the two once per frame on
//! [`Stage::Input`](../../spark_core/enum.Stage.html). [`EventWriter<T>`]
//! and [`EventReader<T>`] are thin [`SystemParam`]s over
//! [`ResMut`]/[`Res`] — the events feature adds no new scheduler machinery.
//!
//! # Why double-buffer
//!
//! One shared buffer races: a reader scheduled before a writer misses the
//! event, and clearing the buffer drops events other readers haven't seen
//! yet. Two buffers plus one fixed swap point give every reader a stable
//! frame in which to observe every event exactly once. The price is one
//! frame of latency, paid deliberately for determinism — see [`Events`].

use crate::{Access, Event, Res, ResMut, Resource, SystemParam, World};

/// Double-buffered event queue for type `T`, stored as a [`Resource`].
///
/// # Logic
///
/// `current` collects this frame's [`send`](Self::send)s; `previous` is the
/// frozen snapshot every [`EventReader`] sees. [`swap`](Self::swap) rotates
/// the two at a single fixed point each frame (the
/// [`swap_events`] system on [`Stage::Input`](../../spark_core/enum.Stage.html)),
/// so an event written on frame N is read on frame N+1 and dropped on N+2.
///
/// # Memory layout
///
/// ```text
/// frame N    writer.send(e)  -> current  = [e]   // not yet visible
/// frame N+1  Input swap      -> previous = [e]    // rotates in
///            reader.read()   -> previous = [e]    // every reader sees e, once
/// frame N+2  Input swap      -> previous = []     // e dropped
/// ```
///
/// # Latency
///
/// A send is visible **the frame after** it is written: the next
/// [`Stage::Input`](../../spark_core/enum.Stage.html) swap rotates it into
/// `previous`, where it stays for exactly one frame. **Every reader in a
/// frame sees the same `previous` snapshot** — intra-frame system ordering
/// never changes what is read, so a `FixedUpdate × N` burst reads the
/// identical events on every step. That order-independence is the
/// determinism guarantee; treat events as next-frame.
///
/// The lone exception is a writer that runs *earlier in `Stage::Input` than
/// this type's [`swap_events`] system*: its send lands in `current` before
/// the swap, so it is rotated in and read the same frame. That hinges on
/// registration order relative to the swap, so don't design around it.
///
/// # Why it works
///
/// Same-frame models make visibility depend on intra-frame system order;
/// read-`previous` removes that variable entirely, so a save/replay run
/// reproduces bit-for-bit regardless of how systems within a frame are
/// scheduled. Each event lives for exactly one frame, so any reader that
/// runs once per frame sees it exactly once — no per-reader cursor needed.
///
/// # How NOT to use
///
/// - Not a consume-once work queue — every reader sees every event, and
///   events vanish after one frame whether read or not. For a drainable
///   queue use a `Resource` wrapping a `VecDeque<T>`.
/// - Not for same-frame, intra-frame messaging — the one-frame latency is
///   inherent. Order the systems within a stage and share a `Resource`
///   instead.
/// - Not for retained "is-key-held" state — that is the input crate's
///   resources, not an event stream.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Event, Events};
///
/// #[derive(Event)]
/// struct Damage(u32);
///
/// let mut events = Events::<Damage>::default();
/// events.send(Damage(10));
/// // Not visible until a swap rotates `current` into `previous`.
/// assert_eq!(events.iter_previous().count(), 0);
///
/// events.swap();
/// assert_eq!(events.iter_previous().map(|d| d.0).sum::<u32>(), 10);
///
/// events.swap(); // next frame: the event has aged out
/// assert_eq!(events.iter_previous().count(), 0);
/// ```
pub struct Events<T: Event> {
    /// Last frame's sends — the frozen snapshot readers iterate.
    previous: Vec<T>,
    /// This frame's sends — invisible to readers until the next swap.
    current: Vec<T>,
}

impl<T: Event> Default for Events<T> {
    /// An empty queue: both buffers start empty. Hand-written rather than
    /// derived so it never demands `T: Default` (`Vec::new` does not). The
    /// struct-level example shows a freshly defaulted queue reads as empty.
    fn default() -> Self {
        Self {
            previous: Vec::new(),
            current: Vec::new(),
        }
    }
}

// `T: Event` is `Send + Sync + 'static`, so `Events<T>` is `'static` —
// satisfying `Resource`'s sole bound. Hand-written (not derived) because
// the type is generic and the derive would inject no useful bound.
impl<T: Event> Resource for Events<T> {}

impl<T: Event> Events<T> {
    /// Queues `event` into `current`, where it stays invisible to readers
    /// until the next [`swap`](Self::swap) rotates it into `previous`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Event, Events};
    ///
    /// #[derive(Event)]
    /// struct Spawned;
    ///
    /// let mut events = Events::<Spawned>::default();
    /// events.send(Spawned);
    /// events.swap();
    /// assert_eq!(events.iter_previous().count(), 1);
    /// ```
    pub fn send(&mut self, event: T) {
        self.current.push(event);
    }

    /// Iterates last frame's sends — the snapshot every [`EventReader`]
    /// observes this frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Event, Events};
    ///
    /// #[derive(Event)]
    /// struct Score(u32);
    ///
    /// let mut events = Events::<Score>::default();
    /// events.send(Score(3));
    /// events.send(Score(4));
    /// events.swap();
    /// assert_eq!(events.iter_previous().map(|s| s.0).sum::<u32>(), 7);
    /// ```
    pub fn iter_previous(&self) -> impl Iterator<Item = &T> {
        self.previous.iter()
    }

    /// Frame-boundary rotation: makes `current` the new `previous` and
    /// empties `current` for the coming frame.
    ///
    /// [`mem::swap`](std::mem::swap) followed by `clear` reuses *both*
    /// buffers' heap allocations — no per-frame reallocation. The swap moves
    /// the two-frames-old batch (what `previous` held *before* the swap) into
    /// `current`, and `clear` drops it there. Called once per frame per event
    /// type by [`swap_events`]; calling it twice in one frame would leave
    /// `previous` empty before any reader runs and silently drop that frame's
    /// events.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Event, Events};
    ///
    /// #[derive(Event)]
    /// struct Beep;
    ///
    /// let mut events = Events::<Beep>::default();
    /// events.send(Beep);
    /// events.swap();
    /// assert_eq!(events.iter_previous().count(), 1); // visible this frame
    /// events.swap();
    /// assert_eq!(events.iter_previous().count(), 0); // aged out next frame
    /// ```
    pub fn swap(&mut self) {
        std::mem::swap(&mut self.previous, &mut self.current);
        self.current.clear();
    }
}

/// Per-type swap system: rotates the [`Events<T>`] buffers once per frame.
///
/// [`add_event`](../../spark_core/struct.Application.html#method.add_event)
/// registers one of these on [`Stage::Input`](../../spark_core/enum.Stage.html)
/// for each event type. It is a plain [`ResMut`] system, so it is ordered
/// against every reader and writer of the same `T` by the existing access
/// model — guaranteeing the swap is the single, frame-fixed rotation point.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Event, Events, IntoSystem, World, swap_events};
///
/// #[derive(Event)]
/// struct Ping;
///
/// let mut world = World::new();
/// world.add_resource(Events::<Ping>::default());
/// world.resource_mut::<Events<Ping>>().send(Ping);
///
/// let mut swap = IntoSystem::into_system(swap_events::<Ping>);
/// swap(&world); // rotates current -> previous
/// assert_eq!(world.resource::<Events<Ping>>().iter_previous().count(), 1);
/// ```
pub fn swap_events<T: Event>(mut events: ResMut<Events<T>>) {
    events.swap();
}

/// Write side of an [`Events<T>`] queue — a [`SystemParam`] taking a
/// resource **write** borrow of `Events<T>`.
///
/// A system parameter; the runner builds it. Sends land in `current` and
/// become readable one frame later (or the same frame, if sent in
/// [`Stage::Input`](../../spark_core/enum.Stage.html) before the swap — see
/// [`Events`]).
///
/// # Examples
///
/// ```
/// use spark_ecs::{Event, Events, EventWriter, IntoSystem, World};
///
/// #[derive(Event)]
/// struct Jump;
///
/// fn emit(mut writer: EventWriter<Jump>) {
///     writer.send(Jump);
/// }
///
/// let mut world = World::new();
/// world.add_resource(Events::<Jump>::default());
/// let mut sys = IntoSystem::into_system(emit);
/// sys(&world);
///
/// world.resource_mut::<Events<Jump>>().swap(); // make the send readable
/// assert_eq!(world.resource::<Events<Jump>>().iter_previous().count(), 1);
/// ```
pub struct EventWriter<'w, T: Event> {
    events: ResMut<'w, Events<T>>,
}

impl<T: Event> EventWriter<'_, T> {
    /// Queues `event` for delivery to next frame's readers. Forwards to
    /// [`Events::send`]; the struct-level example shows it in a system.
    pub fn send(&mut self, event: T) {
        self.events.send(event);
    }
}

impl<'a, T: Event> SystemParam for EventWriter<'a, T> {
    type Item<'w>
        = EventWriter<'w, T>
    where
        'a: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        'a: 'w,
    {
        EventWriter {
            events: ResMut::from_world(world),
        }
    }
    fn collect_access(access: &mut Access) {
        access.add_resource_write::<Events<T>>();
    }
}

/// Read side of an [`Events<T>`] queue — a [`SystemParam`] taking a
/// resource **read** borrow of `Events<T>`.
///
/// **Stateless.** Unlike a Bevy-style `EventReader`, it holds no per-system
/// cursor: [`read`](Self::read) always iterates `previous`, last frame's
/// frozen snapshot. That keeps it buildable before `Local<T>` exists and
/// makes reads deterministic — every reader in a frame sees the same
/// events regardless of intra-frame ordering.
///
/// # Scheduler note
///
/// An `EventReader<T>` and an `EventWriter<T>` for the same `T` in separate
/// parallel-capable systems are *ordered* rather than parallelised, because
/// both appear as access to `Events<T>` (one read, one write). Two
/// `EventReader<T>`s batch together for free (read/read never conflicts).
/// Split-access event params (exposing `previous`/`current` separately so a
/// reader and writer can run concurrently) are an M4 follow-up.
///
/// Taking an `EventReader<T>` *and* an `EventWriter<T>` for the same `T` in
/// **one** system panics at runtime (`already borrowed`): both borrow the
/// single `Events<T>` resource — one shared, one exclusive. Read in one
/// system and write in another; with read-previous they communicate across
/// the frame boundary anyway.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Event, Events, EventReader, IntoSystem, ResMut, Resource, World};
///
/// #[derive(Event)]
/// struct Scored(u32);
/// #[derive(Resource)]
/// struct Total(u32);
///
/// fn tally(reader: EventReader<Scored>, mut total: ResMut<Total>) {
///     for ev in reader.read() {
///         total.0 += ev.0;
///     }
/// }
///
/// let mut world = World::new();
/// world.add_resource(Total(0));
/// world.add_resource(Events::<Scored>::default());
/// world.resource_mut::<Events<Scored>>().send(Scored(7));
/// world.resource_mut::<Events<Scored>>().swap(); // last frame's sends are now readable
///
/// IntoSystem::into_system(tally)(&world);
/// assert_eq!(world.resource::<Total>().0, 7);
/// ```
pub struct EventReader<'w, T: Event> {
    events: Res<'w, Events<T>>,
}

impl<T: Event> EventReader<'_, T> {
    /// Iterates the events written *last* frame. Stateless: re-reads the
    /// same `previous` snapshot every call within a frame.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Event, Events, EventReader, IntoSystem, ResMut, Resource, World};
    ///
    /// #[derive(Event)]
    /// struct Tile { x: i32 }
    /// #[derive(Resource)]
    /// struct LastX(i32);
    ///
    /// fn observe(reader: EventReader<Tile>, mut last: ResMut<LastX>) {
    ///     for ev in reader.read() {
    ///         last.0 = ev.x;
    ///     }
    /// }
    ///
    /// let mut world = World::new();
    /// world.add_resource(LastX(-1));
    /// world.add_resource(Events::<Tile>::default());
    /// world.resource_mut::<Events<Tile>>().send(Tile { x: 42 });
    /// world.resource_mut::<Events<Tile>>().swap();
    /// IntoSystem::into_system(observe)(&world);
    /// assert_eq!(world.resource::<LastX>().0, 42);
    /// ```
    pub fn read(&self) -> impl Iterator<Item = &T> {
        self.events.iter_previous()
    }
}

impl<'a, T: Event> SystemParam for EventReader<'a, T> {
    type Item<'w>
        = EventReader<'w, T>
    where
        'a: 'w;
    fn fetch<'w>(world: &'w World) -> Self::Item<'w>
    where
        'a: 'w,
    {
        EventReader {
            events: Res::from_world(world),
        }
    }
    fn collect_access(access: &mut Access) {
        access.add_resource_read::<Events<T>>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::IntoSystem;

    #[derive(Event)]
    struct TestEvent(u32);

    /// Accumulates the event payloads a reader system observed, so a test
    /// can assert what was read after running the system against a `World`.
    #[derive(Resource)]
    struct Seen(Vec<u32>);

    /// `Events<T>` must be `Send + Sync` so the M4 parallel executor can
    /// move buffers across worker threads without a breaking change. A
    /// hand-rolled compile assert keeps the guarantee with no dependency.
    #[test]
    fn events_is_send_sync() {
        const fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Events<TestEvent>>();
    }

    #[test]
    fn swap_rotates_current_into_previous_and_clears_current() {
        let mut events = Events::<TestEvent>::default();
        events.send(TestEvent(1));
        events.send(TestEvent(2));
        // Before the swap, `current` holds the sends and `previous` is empty.
        assert_eq!(events.iter_previous().count(), 0);

        events.swap();
        let read: Vec<u32> = events.iter_previous().map(|e| e.0).collect();
        assert_eq!(read, vec![1, 2]);

        // A second swap with no new sends clears `previous` — events age out.
        events.swap();
        assert_eq!(events.iter_previous().count(), 0);
    }

    #[test]
    fn swap_keeps_only_one_frame_of_events() {
        // Send on three successive frames; each frame's reader must see only
        // that-frame's prior sends, never an accumulation.
        let mut events = Events::<TestEvent>::default();
        events.send(TestEvent(1));
        events.swap();
        assert_eq!(
            events.iter_previous().map(|e| e.0).collect::<Vec<_>>(),
            vec![1]
        );

        events.send(TestEvent(2));
        events.swap();
        assert_eq!(
            events.iter_previous().map(|e| e.0).collect::<Vec<_>>(),
            vec![2]
        );

        // No send this frame: previous empties out.
        events.swap();
        assert_eq!(events.iter_previous().count(), 0);
    }

    #[test]
    fn writer_send_then_reader_read_across_a_swap() {
        let mut world = World::new();
        world.add_resource(Events::<TestEvent>::default());
        world.add_resource(Seen(Vec::new()));

        // Frame N: writer sends; nothing readable yet (previous is empty).
        let mut emit = IntoSystem::into_system(|mut w: EventWriter<TestEvent>| {
            w.send(TestEvent(9));
        });
        emit(&world);
        assert_eq!(
            world
                .resource::<Events<TestEvent>>()
                .iter_previous()
                .count(),
            0
        );

        // Frame boundary: swap rotates the send into `previous`.
        world.resource_mut::<Events<TestEvent>>().swap();

        // Frame N+1: reader observes exactly the sent event.
        let mut read =
            IntoSystem::into_system(|r: EventReader<TestEvent>, mut seen: ResMut<Seen>| {
                seen.0.extend(r.read().map(|e| e.0));
            });
        read(&world);
        assert_eq!(world.resource::<Seen>().0, vec![9]);

        // Frame N+2: another swap ages the event out; the reader sees nothing new.
        world.resource_mut::<Events<TestEvent>>().swap();
        read(&world);
        assert_eq!(world.resource::<Seen>().0, vec![9]);
    }

    #[test]
    fn two_readers_each_see_every_event_exactly_once() {
        let mut world = World::new();
        world.add_resource(Events::<TestEvent>::default());
        world.add_resource(Seen(Vec::new()));

        world.resource_mut::<Events<TestEvent>>().send(TestEvent(1));
        world.resource_mut::<Events<TestEvent>>().send(TestEvent(2));
        world.resource_mut::<Events<TestEvent>>().swap();

        // Two independent reader systems run in the same frame against the
        // same frozen `previous` snapshot. Each sees both events, once.
        let mut reader_a =
            IntoSystem::into_system(|r: EventReader<TestEvent>, mut seen: ResMut<Seen>| {
                seen.0.extend(r.read().map(|e| e.0));
            });
        let mut reader_b =
            IntoSystem::into_system(|r: EventReader<TestEvent>, mut seen: ResMut<Seen>| {
                seen.0.extend(r.read().map(|e| e.0));
            });
        reader_a(&world);
        reader_b(&world);
        assert_eq!(world.resource::<Seen>().0, vec![1, 2, 1, 2]);
    }

    #[test]
    fn reads_are_stable_across_repeated_reads_in_a_frame() {
        // Models a `FixedUpdate × N` burst: the same reader runs N times
        // within one frame (no swap between) and reads the identical
        // snapshot every step.
        let mut world = World::new();
        world.add_resource(Events::<TestEvent>::default());
        world.add_resource(Seen(Vec::new()));

        world.resource_mut::<Events<TestEvent>>().send(TestEvent(5));
        world.resource_mut::<Events<TestEvent>>().swap();

        let mut step =
            IntoSystem::into_system(|r: EventReader<TestEvent>, mut seen: ResMut<Seen>| {
                seen.0.extend(r.read().map(|e| e.0));
            });
        for _ in 0..3 {
            step(&world);
        }
        // Three steps, same snapshot each time.
        assert_eq!(world.resource::<Seen>().0, vec![5, 5, 5]);
    }

    #[test]
    fn reader_declares_resource_read_writer_declares_resource_write() {
        // The access kinds are what serialize a writer against readers and
        // batch readers together. Two readers never conflict (read/read); a
        // reader and a writer of the same event type do (write/read).
        let mut reader_access = Access::new();
        <EventReader<TestEvent> as SystemParam>::collect_access(&mut reader_access);

        let mut writer_access = Access::new();
        <EventWriter<TestEvent> as SystemParam>::collect_access(&mut writer_access);

        assert!(reader_access.compatible_with(&reader_access)); // read/read: fine
        assert!(!reader_access.compatible_with(&writer_access)); // read/write: conflict
        assert!(!writer_access.compatible_with(&writer_access)); // write/write: conflict
    }
}
