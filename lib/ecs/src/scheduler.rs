//! The sequential scheduler: a [`Schedule`] groups systems into
//! conflict-free batches and runs them.
//!
//! A [`Schedule`] owns a list of registered systems. At run time it
//! partitions them into **batches** — each batch a set of systems whose
//! [`Access`] sets are pairwise disjoint, so nothing in a batch reads
//! what another member writes. Today the executor walks the batches one
//! system at a time on the calling thread; the batch *structure* is the
//! plan the M4 parallel executor will hand to Rayon unchanged. Building
//! and testing that structure now, under a sequential walk, means M4 only
//! swaps the innermost loop (`for sys` → `rayon::scope`) — the analysis
//! that makes it sound is already proven.
//!
//! One caveat to that promise: [`Commands`](crate::Commands) declares no
//! access, so two command-queuing systems may share a batch. Running
//! *those* concurrently is not made safe by this analysis — it needs
//! per-system command buffers merged after the batch, a committed M4 task
//! in its own right. Every other parameter's safety does follow from the
//! batch structure alone.
//!
//! Ordering is **access-derived** only: with no explicit
//! `.before()`/`.after()` yet (that lands with workloads), two systems
//! that conflict are serialised in *registration order* — the earlier
//! `add_system` call runs first. See [`Schedule`] for how the batching
//! turns that rule into layers.

use crate::access::Access;
use crate::system::IntoSystem;
use crate::world::World;

/// Identifies a system within a [`Schedule`] — its registration index.
///
/// Returned inside the batch lists from [`Schedule::batches`]. The inner
/// value is a position in the schedule's system list, so [`usize`] (the
/// natural index type) avoids any cast on the hot path.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Schedule, World};
///
/// fn noop() {}
/// let mut schedule = Schedule::new();
/// schedule.add_system(noop);
/// let first = schedule.batches()[0][0];
/// assert_eq!(first, first);   // `SystemId` is comparable
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SystemId(usize);

/// A registered system: its erased run closure, its declared [`Access`],
/// and its name (the fn's type path, for diagnostics).
struct BoxedSystem {
    name: &'static str,
    access: Access,
    run: Box<dyn FnMut(&World) + 'static>,
}

/// Partitions systems into access-disjoint batches via **ASAP layering**.
///
/// # Logic
///
/// Each system gets a *rank*. Walking systems in registration order, a
/// system's rank is one more than the highest rank of any **earlier**
/// system it conflicts with, or `0` if it conflicts with none:
///
/// ```text
/// rank[i] = max( rank[j] + 1 ) over all j < i where i and j conflict
///         = 0 if no such j
/// ```
///
/// Systems sharing a rank become one batch, in registration order.
///
/// # Memory layout
///
/// ```text
/// registration:  S0(write A)  S1(write B)  S2(read A)  S3(read B)
/// conflicts:      S2↔S0 (A)    S3↔S1 (B)
/// rank:           0            0            1           1
/// batches:        [ [S0, S1], [S2, S3] ]
///                   rank 0      rank 1
/// ```
///
/// S1 and S3 touch only B, so they slot into the *earliest* batch that
/// holds no conflicting predecessor — not a later one just because they
/// were registered late.
///
/// # Why it works
///
/// Two systems land in the same batch only if they have equal rank. If
/// they conflicted, the later-registered one would have taken
/// `rank+1` — so equal rank *proves* disjoint access. That is the exact
/// invariant the M4 parallel executor needs: every member of a batch can
/// run on its own thread because none aliases another's writes. And
/// because a conflict always raises the later system's rank, conflicting
/// pairs keep registration order; only provably-independent systems are
/// ever reordered, and reordering independent work changes no observable
/// result.
///
/// # Cost
///
/// Ranking compares every system against every earlier one — `O(n²)`
/// access checks for `n` systems, run once per [`Schedule`] rebuild (not
/// per frame). At the tens-to-hundreds of systems a stage holds, that is
/// negligible; an indexed-by-`TypeId` scheme would only pay off at a
/// scale this engine will not reach.
fn build_batches(systems: &[BoxedSystem]) -> Vec<Vec<SystemId>> {
    let mut rank = vec![0usize; systems.len()];
    for i in 0..systems.len() {
        for j in 0..i {
            if !systems[j].access.compatible_with(&systems[i].access) {
                rank[i] = rank[i].max(rank[j] + 1);
            }
        }
    }

    let batch_count = rank.iter().copied().max().map_or(0, |max| max + 1);
    let mut batches = vec![Vec::new(); batch_count];
    for (i, &r) in rank.iter().enumerate() {
        batches[r].push(SystemId(i));
    }

    // The whole point of the rank scheme is "same batch ⟹ disjoint
    // access" — the proof the M4 parallel executor leans on. Assert it
    // directly in debug/test builds rather than trusting the argument:
    // any future tweak (e.g. an explicit-ordering pass) that breaks it
    // trips here instead of racing silently later.
    #[cfg(debug_assertions)]
    for batch in &batches {
        for (pos, &SystemId(a)) in batch.iter().enumerate() {
            for &SystemId(b) in &batch[pos + 1..] {
                assert!(
                    systems[a].access.compatible_with(&systems[b].access),
                    "build_batches invariant violated: `{}` and `{}` share a batch but conflict",
                    systems[a].name,
                    systems[b].name,
                );
            }
        }
    }

    batches
}

/// A runnable group of systems, batched by access conflicts.
///
/// # Logic
///
/// [`add_system`](Self::add_system) records each system's run closure and
/// its declared [`Access`] (read straight off the parameter types) and
/// invalidates the cached batch plan. The plan is rebuilt lazily on the
/// next [`run`](Self::run) or [`batches`](Self::batches) call by
/// [`build_batches`] — an *as-soon-as-possible* layering where each batch
/// holds only systems with pairwise-disjoint access.
///
/// # Memory layout
///
/// ```text
/// Schedule
/// ├── systems: [ S0, S1, S2, … ]          ← registration order
/// └── batches: Some([ [S0, S1], [S2] ])   ← None until first run, then cached
/// ```
///
/// # Why batches exist before parallelism does
///
/// The executor walks batches **sequentially** today — one thread, one
/// system at a time. The batches still earn their keep: they are the
/// safety proof for M4 lockless parallelism. Same-batch systems have
/// disjoint writes (see [`build_batches`]), so M4 can spawn each batch
/// across Rayon workers with only a shared `&World` and no data race. The
/// risky analysis ships and is tested now; M4 swaps the walk, not the
/// proof.
///
/// # How NOT to use
///
/// - Don't read `Schedule::run` order as a contract between
///   *non-conflicting* systems — independent systems may be reordered
///   into earlier batches. Order is guaranteed only between systems that
///   share state (and there it follows registration order).
/// - Don't expect commands to apply mid-run: queued [`Commands`] flush
///   **once**, after every batch has run (see [`run`](Self::run)).
///
/// [`Commands`]: crate::Commands
///
/// # Examples
///
/// ```
/// use spark_ecs::{ResMut, Resource, Schedule, World};
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
///
/// let mut schedule = Schedule::new();
/// schedule.add_system(tick);
/// schedule.run(&mut world);
/// schedule.run(&mut world);
/// assert_eq!(world.resource::<Counter>().0, 2);
/// ```
#[derive(Default)]
pub struct Schedule {
    systems: Vec<BoxedSystem>,
    /// `None` means "stale" — rebuilt lazily so a burst of `add_system`
    /// calls costs one rebuild, not one per call.
    batches: Option<Vec<Vec<SystemId>>>,
}

impl Schedule {
    /// Creates an empty schedule.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Schedule;
    ///
    /// let schedule = Schedule::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers `system`, collecting its [`Access`] from its parameter
    /// types and marking the batch plan stale.
    ///
    /// Accepts any function whose parameters are all
    /// [`SystemParam`](crate::SystemParam) — the same `IntoSystem` bound
    /// the rest of the engine uses, so call sites need no turbofish.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Res, Resource, Schedule};
    ///
    /// #[derive(Resource)]
    /// struct Config(u32);
    ///
    /// fn read_config(_c: Res<Config>) {}
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_system(read_config);
    /// ```
    pub fn add_system<S, Marker>(&mut self, system: S) -> &mut Self
    where
        S: IntoSystem<Marker>,
    {
        self.systems.push(BoxedSystem {
            name: std::any::type_name::<S>(),
            access: <S as IntoSystem<Marker>>::access(),
            run: system.into_system(),
        });
        self.batches = None;
        self
    }

    /// Returns the batch plan, rebuilding it if a system was added since
    /// the last call.
    ///
    /// Each inner slice is one batch — systems with pairwise-disjoint
    /// [`Access`], safe to run together. The outer order is execution
    /// order. Primarily a window for tests and (later) the editor's
    /// schedule view; [`run`](Self::run) uses the same plan internally.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{ResMut, Resource, Schedule};
    ///
    /// #[derive(Resource)]
    /// struct Score(u32);
    ///
    /// fn write_a(mut s: ResMut<Score>) { s.0 += 1; }
    /// fn write_b(mut s: ResMut<Score>) { s.0 += 1; }
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_system(write_a);
    /// schedule.add_system(write_b);
    /// // Both write Score, so they cannot share a batch.
    /// assert_eq!(schedule.batches().len(), 2);
    /// ```
    pub fn batches(&mut self) -> &[Vec<SystemId>] {
        self.batches
            .get_or_insert_with(|| build_batches(&self.systems))
            .as_slice()
    }

    /// Names of the registered systems, in registration order.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Schedule;
    ///
    /// fn my_system() {}
    /// let mut schedule = Schedule::new();
    /// schedule.add_system(my_system);
    /// assert!(schedule.system_names().next().unwrap().ends_with("my_system"));
    /// ```
    pub fn system_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.systems.iter().map(|s| s.name)
    }

    /// Runs every system once, batch by batch, then flushes commands.
    ///
    /// Batches run in order; within a batch, systems run in registration
    /// order (sequentially — M4 makes a batch parallel). After the last
    /// system, [`World::flush_commands`] applies everything queued via
    /// [`Commands`](crate::Commands) — **once** for the whole run, the
    /// same per-stage boundary [`Application::run_stage`] uses.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Commands, Component, Query, Schedule, World};
    ///
    /// #[derive(Component)]
    /// struct Spawned;
    ///
    /// fn spawn_one(mut commands: Commands) {
    ///     commands.spawn().insert(Spawned);
    /// }
    ///
    /// let mut world = World::new();
    /// let mut schedule = Schedule::new();
    /// schedule.add_system(spawn_one);
    /// schedule.run(&mut world);
    /// // The deferred spawn was flushed at the end of the run.
    /// assert_eq!(Query::<&Spawned>::from_world(&world).iter().count(), 1);
    /// ```
    ///
    /// [`Application::run_stage`]: https://docs.rs/spark-core
    pub fn run(&mut self, world: &mut World) {
        // Take the cached plan out (building it if stale) so the loop can
        // borrow `self.systems` mutably without a simultaneous borrow of
        // `self.batches`; the owned `batches` local holds no borrow of
        // `self`. Restored right after — the plan is moved out and back,
        // never reallocated when the cache is warm.
        let batches = self
            .batches
            .take()
            .unwrap_or_else(|| build_batches(&self.systems));
        for batch in &batches {
            for &SystemId(idx) in batch {
                // `world` is `&mut World`; it coerces to the `&World` the
                // run closure expects, leaving it usable mutably for the
                // flush below.
                (self.systems[idx].run)(world);
            }
        }
        self.batches = Some(batches);
        world.flush_commands();
    }
}

#[cfg(test)]
#[allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    reason = "test fns live next to their assertions; system fns take \
              SystemParam values like Res<T> by value to match how plugins \
              write systems."
)]
mod tests {
    use super::*;
    use crate::{Commands, Component, Query, Res, ResMut, Resource, With};

    #[derive(Resource)]
    struct Score {
        value: u32,
    }
    #[derive(Resource)]
    struct Frame {
        n: u32,
    }
    #[derive(Resource)]
    struct Log {
        order: Vec<&'static str>,
    }
    #[derive(Component)]
    struct Position {
        x: f32,
    }
    #[derive(Component)]
    struct Velocity {
        x: f32,
    }

    #[test]
    fn empty_schedule_runs_and_flushes() {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule.run(&mut world); // no panic, no systems
        assert_eq!(schedule.batches().len(), 0);
    }

    #[test]
    fn disjoint_systems_share_one_batch() {
        fn touch_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn touch_frame(mut f: ResMut<Frame>) {
            f.n += 1;
        }
        let mut schedule = Schedule::new();
        schedule.add_system(touch_score);
        schedule.add_system(touch_frame);
        // Different resources → no conflict → one batch of two.
        assert_eq!(schedule.batches().len(), 1);
        assert_eq!(schedule.batches()[0].len(), 2);
    }

    #[test]
    fn conflicting_systems_split_into_separate_batches() {
        fn writer(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn reader(s: Res<Score>) {
            let _ = s.value;
        }
        let mut schedule = Schedule::new();
        schedule.add_system(writer);
        schedule.add_system(reader);
        let batches = schedule.batches();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![SystemId(0)]); // writer first (registration order)
        assert_eq!(batches[1], vec![SystemId(1)]); // reader after
    }

    #[test]
    fn independent_system_packs_into_earliest_batch() {
        // S0 writes Score, S1 reads Score (conflict → rank 1),
        // S2 reads Frame (conflicts with neither → rank 0).
        fn write_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn read_score(s: Res<Score>) {
            let _ = s.value;
        }
        fn read_frame(f: Res<Frame>) {
            let _ = f.n;
        }
        let mut schedule = Schedule::new();
        schedule.add_system(write_score);
        schedule.add_system(read_score);
        schedule.add_system(read_frame);
        let batches = schedule.batches();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![SystemId(0), SystemId(2)]); // S2 joins S0's batch
        assert_eq!(batches[1], vec![SystemId(1)]);
    }

    #[test]
    fn component_query_conflicts_feed_batching() {
        // One system writes Position; another reads it → conflict.
        fn move_pos(mut q: Query<&mut Position>) {
            for p in q.iter_mut() {
                p.x += 1.0;
            }
        }
        fn read_pos(q: Query<&Position>) {
            for p in q.iter() {
                let _ = p.x;
            }
        }
        let mut schedule = Schedule::new();
        schedule.add_system(move_pos);
        schedule.add_system(read_pos);
        assert_eq!(schedule.batches().len(), 2);
    }

    #[test]
    fn query_over_disjoint_components_shares_a_batch() {
        fn move_pos(mut q: Query<&mut Position>) {
            for p in q.iter_mut() {
                p.x += 1.0;
            }
        }
        fn move_vel(mut q: Query<&mut Velocity>) {
            for v in q.iter_mut() {
                v.x += 1.0;
            }
        }
        let mut schedule = Schedule::new();
        schedule.add_system(move_pos);
        schedule.add_system(move_vel);
        assert_eq!(schedule.batches().len(), 1);
    }

    #[test]
    fn access_aggregates_across_multiple_params() {
        // `mover`'s access is the union of all four params: a resource
        // read (Frame), a resource write (Score), a component write
        // (Position), and Commands (no access). If aggregation folded only
        // the first param — or Commands wrongly cleared the rest — the
        // conflicts below would vanish and the batch layout would change.
        fn mover(_f: Res<Frame>, mut s: ResMut<Score>, mut q: Query<&mut Position>, _c: Commands) {
            s.value += 1;
            for p in q.iter_mut() {
                p.x += 1.0;
            }
        }
        fn reads_pos(q: Query<&Position>) {
            for p in q.iter() {
                let _ = p.x;
            }
        }
        fn reads_score(s: Res<Score>) {
            let _ = s.value;
        }
        fn moves_vel(mut q: Query<&mut Velocity>) {
            for v in q.iter_mut() {
                v.x += 1.0;
            }
        }

        let mut schedule = Schedule::new();
        schedule.add_system(mover); // 0
        schedule.add_system(reads_pos); // 1 — conflicts mover on Position (component)
        schedule.add_system(reads_score); // 2 — conflicts mover on Score (resource)
        schedule.add_system(moves_vel); // 3 — disjoint from all

        let batches = schedule.batches();
        assert_eq!(batches.len(), 2);
        // mover writes both Position and Score, so it conflicts with the
        // Position reader AND the Score reader — proof both writes landed
        // in its access. `moves_vel` is disjoint and packs into batch 0.
        assert_eq!(batches[0], vec![SystemId(0), SystemId(3)]);
        assert_eq!(batches[1], vec![SystemId(1), SystemId(2)]);
    }

    #[test]
    fn filter_read_access_feeds_batching() {
        // `With<Velocity>` makes `filtered` *read* Velocity even though it
        // only yields `&Position` — so it conflicts with a Velocity writer.
        // `plain_pos` has no filter, reads only Position, and stays
        // conflict-free: the filter is the sole reason `filtered` is held
        // back a batch.
        fn writes_vel(mut q: Query<&mut Velocity>) {
            for v in q.iter_mut() {
                v.x += 1.0;
            }
        }
        fn filtered(q: Query<&Position, With<Velocity>>) {
            for p in q.iter() {
                let _ = p.x;
            }
        }
        fn plain_pos(q: Query<&Position>) {
            for p in q.iter() {
                let _ = p.x;
            }
        }

        let mut schedule = Schedule::new();
        schedule.add_system(writes_vel); // 0 — writes Velocity
        schedule.add_system(filtered); // 1 — reads Position + (via filter) Velocity → conflict
        schedule.add_system(plain_pos); // 2 — reads Position only → disjoint

        let batches = schedule.batches();
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![SystemId(0), SystemId(2)]);
        assert_eq!(batches[1], vec![SystemId(1)]);
    }

    #[test]
    fn conflict_chain_layers_into_three_batches() {
        // A transitive chain forces ranks 0 → 1 → 2: S0 writes Score, S1
        // reads Score (after S0) and writes Frame, S2 reads Frame (after
        // S1). S2's rank can only reach 2 by inheriting S1's rank — the
        // recursive `max(rank[j] + 1)` step a non-recursive splitter misses.
        fn write_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn score_to_frame(s: Res<Score>, mut f: ResMut<Frame>) {
            f.n += s.value;
        }
        fn read_frame(f: Res<Frame>) {
            let _ = f.n;
        }

        let mut schedule = Schedule::new();
        schedule.add_system(write_score); // 0 — rank 0
        schedule.add_system(score_to_frame); // 1 — conflicts S0 on Score → rank 1
        schedule.add_system(read_frame); // 2 — conflicts S1 on Frame → rank 2

        let batches = schedule.batches();
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0], vec![SystemId(0)]);
        assert_eq!(batches[1], vec![SystemId(1)]);
        assert_eq!(batches[2], vec![SystemId(2)]);
    }

    #[test]
    fn run_executes_every_system_once() {
        fn bump_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn bump_frame(mut f: ResMut<Frame>) {
            f.n += 10;
        }
        let mut world = World::new();
        world.add_resource(Score { value: 0 });
        world.add_resource(Frame { n: 0 });
        let mut schedule = Schedule::new();
        schedule.add_system(bump_score);
        schedule.add_system(bump_frame);
        schedule.run(&mut world);
        assert_eq!(world.resource::<Score>().value, 1);
        assert_eq!(world.resource::<Frame>().n, 10);
    }

    #[test]
    fn conflicting_systems_run_in_registration_order() {
        // Both touch Log (write), so they are serialised; the earlier
        // registration must run first.
        fn first(mut log: ResMut<Log>) {
            log.order.push("first");
        }
        fn second(mut log: ResMut<Log>) {
            log.order.push("second");
        }
        let mut world = World::new();
        world.add_resource(Log { order: Vec::new() });
        let mut schedule = Schedule::new();
        schedule.add_system(first);
        schedule.add_system(second);
        schedule.run(&mut world);
        assert_eq!(world.resource::<Log>().order, vec!["first", "second"]);
    }

    #[test]
    fn run_flushes_commands_once_at_the_end() {
        #[derive(Component)]
        struct Tag;
        fn spawn_two(mut commands: Commands) {
            commands.spawn().insert(Tag);
            commands.spawn().insert(Tag);
        }
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule.add_system(spawn_two);
        schedule.run(&mut world);
        assert_eq!(Query::<&Tag>::from_world(&world).iter().count(), 2);
    }

    #[test]
    fn adding_a_system_rebuilds_the_batch_plan() {
        fn a(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn b(mut f: ResMut<Frame>) {
            f.n += 1;
        }
        let mut schedule = Schedule::new();
        schedule.add_system(a);
        assert_eq!(schedule.batches().len(), 1); // builds + caches
        schedule.add_system(b); // invalidates cache
        assert_eq!(schedule.batches()[0].len(), 2); // rebuilt with both
    }

    #[test]
    fn system_names_track_registration() {
        fn alpha() {}
        fn beta() {}
        let mut schedule = Schedule::new();
        schedule.add_system(alpha);
        schedule.add_system(beta);
        let names: Vec<&str> = schedule.system_names().collect();
        assert_eq!(names.len(), 2);
        assert!(names[0].ends_with("alpha"));
        assert!(names[1].ends_with("beta"));
    }
}
