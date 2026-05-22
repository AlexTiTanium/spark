//! The sequential scheduler: a [`Schedule`] groups systems into
//! **workloads**, orders them, and runs them batch by batch.
//!
//! A [`Schedule`] is one stage's worth of work. Inside it, systems live in
//! workloads — named bundles a plugin registers with
//! [`add_workload`](Schedule::add_workload), plus an *anonymous* workload
//! that holds bare [`add_system`](Schedule::add_system) /
//! [`add_systems`](Schedule::add_systems) calls. Workloads are ordered
//! against each other by [`WorkloadLabel`](crate::WorkloadLabel); systems
//! inside a workload are ordered against each other by handle
//! ([`SystemRef`](crate::SystemRef)). Both use the same `.after`/`.before`
//! verb — see [`crate::workload`].
//!
//! At run time each workload partitions its systems into **batches** —
//! each batch a set whose [`Access`] sets are pairwise disjoint, so
//! nothing in a batch reads what another member writes. Today the executor
//! walks the batches one system at a time; the batch *structure* is the
//! plan the M4 parallel executor will hand to Rayon unchanged.
//!
//! One caveat to that promise: [`Commands`](crate::Commands) declares no
//! access, so two command-queuing systems may share a batch. Running
//! *those* concurrently is not made safe by this analysis — it needs
//! per-system command buffers merged after the batch, a committed M4 task
//! in its own right. Every other parameter's safety does follow from the
//! batch structure alone.
//!
//! # Ordering is explicit, not registration order (decision B2)
//!
//! Two systems — or two workloads — whose access *conflicts* with no
//! order declared between them is a **registration error**
//! ([`validate_system_conflicts`](crate::workload::validate_system_conflicts)),
//! not a silent fallthrough. Declare `.after`/`.before`, or acknowledge an
//! intentional don't-care with `.ambiguous_with`. A conflicting pair is
//! always placed in separate batches; a declared edge fixes which runs
//! first, while an acknowledged pair simply takes its topological position
//! (registration index when no edge constrains it).

use crate::system::IntoSystem;
use crate::workload::{
    BoxedSystem, EdgeKind, IntoSystemTuple, SystemId, WorkloadBuilder, WorkloadData, WorkloadId,
    WorkloadLabel, cycle_message, cycle_path, first_conflict, has_declared_order, successors_of,
    topo_sort, unknown_label_message, workload_conflict_message,
};
use crate::world::World;

/// A runnable group of workloads — one stage's worth of work, batched by
/// access conflicts and ordered by explicit `.after`/`.before`.
///
/// # Logic
///
/// A fresh `Schedule` holds one **anonymous** workload at index 0.
/// [`add_system`](Self::add_system) / [`add_systems`](Self::add_systems)
/// feed it; [`add_workload`](Self::add_workload) appends a named one and
/// returns a [`WorkloadOrderBuilder`] for `.after(label)` / `.before(label)`.
/// The plan is rebuilt lazily on the next [`run`](Self::run) /
/// [`batches`](Self::batches): workloads topo-sort by their label edges,
/// then each workload's systems topo-sort by their handle edges and
/// partition into batches.
///
/// # Memory layout
///
/// ```text
/// Schedule
/// ├── workloads: [ <anonymous>, "Grid::Supply", "Grid::Distribute" ]
/// │                   │              │                 │
/// │                   └─ systems + intra-workload edges + cached batches
/// ├── workload_edges: [ WorkloadEdge { subject: 2, kind: After, target: Grid::Supply } ]
/// └── order: Some([0, 1, 2])    ← None until first run, then cached
/// ```
///
/// # How NOT to use
///
/// - Don't read [`run`](Self::run) order as a contract between
///   *non-conflicting, unordered* systems — independent systems may share
///   a batch and reorder. Order holds only where declared, or between
///   conflicting systems (which must be declared or acknowledged).
/// - Don't expect commands to apply mid-workload: queued
///   [`Commands`](crate::Commands) flush at every **workload boundary**
///   (see [`run`](Self::run)).
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
pub struct Schedule {
    /// Every workload; index 0 is always the anonymous tier-0 workload.
    workloads: Vec<WorkloadData>,
    /// Workload-level ordering edges, resolved to indices lazily at build
    /// (so forward references work).
    workload_edges: Vec<WorkloadEdge>,
    /// Acknowledged don't-care workload pairs.
    workload_ambiguous: Vec<WorkloadAmbiguity>,
    /// Topo-sorted workload execution order. `None` means "stale" — rebuilt
    /// lazily so a burst of registrations costs one rebuild, not one each.
    order: Option<Vec<usize>>,
}

/// One workload-level ordering declaration, before label resolution.
///
/// `subject` is the index of the workload that declared it; `target` is
/// the label it ordered against, resolved to an index at build time.
/// `target_name` rides along solely so an unknown-label error can name the
/// missing workload (the [`WorkloadId`] alone isn't human-readable).
struct WorkloadEdge {
    subject: usize,
    kind: EdgeKind,
    target: WorkloadId,
    target_name: &'static str,
}

/// A workload acknowledging that its conflict with another (`other`, by
/// label) has no declared order on purpose — the workload-level
/// `.ambiguous_with`.
struct WorkloadAmbiguity {
    subject: usize,
    other: WorkloadId,
}

impl Default for Schedule {
    fn default() -> Self {
        Self::new()
    }
}

impl Schedule {
    /// Creates an empty schedule — just the anonymous workload.
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
        Self {
            workloads: vec![WorkloadData::new(None, "<anonymous>")],
            workload_edges: Vec::new(),
            workload_ambiguous: Vec::new(),
            order: None,
        }
    }

    /// Registers `system` in the anonymous workload, unordered.
    ///
    /// Returns `&mut Self` for chaining — bare `add_system` hands back no
    /// handle, because tier-0 systems are not ordered. To order systems,
    /// open a workload with [`add_workload`](Self::add_workload) and use
    /// `w.add_system(..).after(handle)`.
    ///
    /// # Panics
    ///
    /// Panics if the system's own parameters conflict — see
    /// [`Access::assert_no_self_conflict`](crate::Access::assert_no_self_conflict).
    /// A *cross-system* conflict with another tier-0 system (no order
    /// possible) instead surfaces as a registration error at
    /// [`run`](Self::run) / [`batches`](Self::batches).
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
        self.workloads[0].push(BoxedSystem::from_system(system));
        self.order = None;
        self
    }

    /// Registers a tuple of systems in the anonymous workload as an
    /// **unordered** group.
    ///
    /// The plural form to [`add_system`](Self::add_system)'s singular: no
    /// order is implied between the tuple's members. If two of them
    /// conflict, that is a registration error at build (tier-0 systems
    /// can't be ordered or acknowledged) — open a workload instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Schedule;
    ///
    /// fn movement() {}
    /// fn ai() {}
    /// fn animate() {}
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_systems((movement, ai, animate));
    /// ```
    pub fn add_systems<T, Marker>(&mut self, systems: T) -> &mut Self
    where
        T: IntoSystemTuple<Marker>,
    {
        systems.register_into(&mut self.workloads[0]);
        self.order = None;
        self
    }

    /// Registers a named workload, building its contents in the closure,
    /// and returns a [`WorkloadOrderBuilder`] for ordering it against other
    /// workloads by label.
    ///
    /// Inside `build`, use [`w.add_system(..)`](WorkloadBuilder::add_system)
    /// (handle-ordered) and [`w.add_systems((..))`](WorkloadBuilder::add_systems)
    /// (unordered). The returned builder is a *statement*, not a fluent
    /// `&mut Self` — chain `.after(label)` / `.before(label)` on it:
    ///
    /// # Panics
    ///
    /// Panics if `label` is already registered in this schedule — each
    /// label names exactly one workload.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Schedule, WorkloadLabel};
    ///
    /// #[derive(WorkloadLabel)]
    /// enum Grid { Supply, Distribute }
    ///
    /// fn collect_supply() {}
    /// fn compute_demand() {}
    /// fn route_power() {}
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_workload(Grid::Supply, |w| {
    ///     w.add_system(collect_supply);
    /// });
    /// schedule
    ///     .add_workload(Grid::Distribute, |w| {
    ///         let demand = w.add_system(compute_demand).id();
    ///         w.add_system(route_power).after(demand);
    ///     })
    ///     .after(Grid::Supply); // workload ordering, by label
    /// ```
    #[allow(
        clippy::needless_pass_by_value,
        reason = "the label is a throwaway unit-enum variant passed inline \
                  (`Grid::Supply`); by-value keeps the call site ergonomic."
    )]
    pub fn add_workload<L, F>(&mut self, label: L, build: F) -> WorkloadOrderBuilder<'_>
    where
        L: WorkloadLabel,
        F: FnOnce(&mut WorkloadBuilder),
    {
        // Each label names exactly one workload; a second registration
        // would make `.after(label)` ambiguous (it resolves to the first),
        // silently leaving the duplicate unordered. Refuse it outright.
        assert!(
            self.index_of(label.id()).is_none(),
            "WorkloadLabel `{}` is registered twice in the same schedule — each label names one workload",
            label.name(),
        );
        let mut builder = WorkloadBuilder::new(label.id(), label.name());
        build(&mut builder);
        let idx = self.workloads.len();
        self.workloads.push(builder.into_data());
        self.order = None;
        WorkloadOrderBuilder {
            schedule: self,
            idx,
        }
    }

    /// Returns the **anonymous** workload's batch plan, rebuilding the
    /// schedule if a registration happened since the last call.
    ///
    /// Each inner slice is one batch — systems with pairwise-disjoint
    /// [`Access`](crate::Access), safe to run together. Named workloads run
    /// too (see [`run`](Self::run)) but are not surfaced here; this window
    /// stays focused on the tier-0 systems most call sites register.
    ///
    /// # Panics
    ///
    /// Panics if the build hits an undeclared conflict, an unknown
    /// workload label, or a cycle — see [`run`](Self::run).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Schedule;
    ///
    /// fn a() {}
    /// fn b() {}
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_systems((a, b));
    /// // Disjoint (no params) → one batch of two.
    /// assert_eq!(schedule.batches().len(), 1);
    /// assert_eq!(schedule.batches()[0].len(), 2);
    /// ```
    pub fn batches(&mut self) -> &[Vec<SystemId>] {
        if self.order.is_none() {
            self.build();
        }
        &self.workloads[0].batches
    }

    /// Names of every registered system, across all workloads, in
    /// registration order.
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
        self.workloads
            .iter()
            .flat_map(|w| w.systems.iter().map(|s| s.name))
    }

    /// Runs every workload in dependency order, each batch by batch, with
    /// a command flush at every workload boundary.
    ///
    /// Workloads run in topo order (the anonymous one first); within a
    /// workload, batches run in order and systems within a batch run
    /// sequentially (M4 makes a batch parallel). After **each** workload,
    /// [`World::flush_commands`] applies everything it queued via
    /// [`Commands`](crate::Commands) — the boundary that makes a workload
    /// the atomic unit.
    ///
    /// # Panics
    ///
    /// Panics on a registration error surfaced at build: an undeclared
    /// conflict between systems or workloads, a `.after`/`.before` against
    /// an unknown label, or a cycle in either ordering level.
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
    /// assert_eq!(Query::<&Spawned>::from_world(&world).iter().count(), 1);
    /// ```
    pub fn run(&mut self, world: &mut World) {
        if self.order.is_none() {
            self.build();
        }
        // Take the order out so the loop can borrow `self.workloads`
        // mutably (to call `run`) without a simultaneous borrow of
        // `self.order`. Restored right after.
        let order = self.order.take().expect("build populated order");
        for &widx in &order {
            // Move the batch plan out of this workload so the run closures
            // can borrow its `systems` mutably; the owned `batches` local
            // holds no borrow of `self`. Restored right after.
            let batches = std::mem::take(&mut self.workloads[widx].batches);
            for batch in &batches {
                for &SystemId(sidx) in batch {
                    (self.workloads[widx].systems[sidx].run)(world);
                }
            }
            self.workloads[widx].batches = batches;
            world.flush_commands();
        }
        self.order = Some(order);
    }

    /// Validates, orders, and batches the whole schedule — the lazy build
    /// behind [`run`](Self::run) and [`batches`](Self::batches).
    ///
    /// Resolves workload labels to indices (forward references allowed),
    /// rejects undeclared cross-workload conflicts, topo-sorts the
    /// workloads, then asks each workload to validate and batch itself.
    ///
    /// The two levels split cleanly: this method owns the *cross-workload*
    /// concerns (label resolution, workload conflicts, workload order);
    /// [`WorkloadData::build`] owns the *within-workload* concerns
    /// (system conflicts, system-ordering cycle, batches).
    ///
    /// # Panics
    ///
    /// On an unknown label, a cycle (workload or system), or an undeclared
    /// conflict — each with its pinned message.
    fn build(&mut self) {
        let resolved = self.resolve_workload_edges();
        self.validate_workload_conflicts(&resolved);

        let order = topo_sort(self.workloads.len(), &resolved).unwrap_or_else(|leftover| {
            let path = cycle_path(&leftover, &resolved);
            let names: Vec<&str> = path.iter().map(|&i| self.workloads[i].name).collect();
            panic!("{}", cycle_message("workload", &names));
        });

        for workload in &mut self.workloads {
            workload.build();
        }

        self.order = Some(order);
    }

    /// Resolves each workload-level label edge to a directed
    /// `(before, after)` index pair, normalising `Before` to its mirror.
    ///
    /// # Panics
    ///
    /// Panics with [`unknown_label_message`] if an edge names a label no
    /// workload in this schedule carries.
    fn resolve_workload_edges(&self) -> Vec<(usize, usize)> {
        let mut resolved = Vec::with_capacity(self.workload_edges.len());
        for edge in &self.workload_edges {
            let target = self
                .index_of(edge.target)
                .unwrap_or_else(|| panic!("{}", unknown_label_message(edge.target_name)));
            match edge.kind {
                EdgeKind::After => resolved.push((target, edge.subject)),
                EdgeKind::Before => resolved.push((edge.subject, target)),
            }
        }
        resolved
    }

    /// The index of the workload carrying `id`, if any.
    fn index_of(&self, id: WorkloadId) -> Option<usize> {
        self.workloads.iter().position(|w| w.label == Some(id))
    }

    /// Rejects any pair of *named* workloads whose aggregate access
    /// conflicts with no order declared and no `.ambiguous_with`.
    ///
    /// The anonymous workload is exempt: it carries no label, runs first,
    /// and cannot be ordered against, so its position is never ambiguous.
    ///
    /// # Panics
    ///
    /// With [`workload_conflict_message`], naming both workloads and the
    /// clashing type.
    fn validate_workload_conflicts(&self, resolved: &[(usize, usize)]) {
        let n = self.workloads.len();
        let successors = successors_of(n, resolved);
        for i in 0..n {
            for j in (i + 1)..n {
                let (a, b) = (&self.workloads[i], &self.workloads[j]);
                // Only named-vs-named: the anonymous workload carries no
                // label, runs first, and can't be ordered against — so its
                // position is never ambiguous.
                let (Some(id_i), Some(id_j)) = (a.label, b.label) else {
                    continue;
                };
                // Gate on the cheap predicate; disjoint workloads short-circuit.
                if a.aggregate_access.compatible_with(&b.aggregate_access) {
                    continue;
                }
                let acknowledged = self.workload_ambiguous.iter().any(|ack| {
                    (ack.subject == i && ack.other == id_j)
                        || (ack.subject == j && ack.other == id_i)
                });
                if has_declared_order(i, j, &successors) || acknowledged {
                    continue;
                }
                // Error path only — name the clashing type for the message.
                let (type_id, kind) = first_conflict(&a.aggregate_access, &b.aggregate_access);
                let (type_name, _) = a
                    .aggregate_access
                    .describe(type_id)
                    .or_else(|| b.aggregate_access.describe(type_id))
                    .unwrap_or(("<unknown>", "value"));
                panic!(
                    "{}",
                    workload_conflict_message(a.name, b.name, type_name, kind)
                );
            }
        }
    }
}

/// The ordering builder returned by [`Schedule::add_workload`] — orders
/// the just-registered workload against others by label.
///
/// `.after(label)` / `.before(label)` **accumulate** and resolve lazily at
/// build, so a workload may be ordered against one registered later.
/// `.ambiguous_with(label)` acknowledges an intentional don't-care with a
/// conflicting workload. It is deliberately *not* `&mut Schedule`: a
/// workload's position is its own statement, separate from its contents.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Schedule, WorkloadLabel};
///
/// #[derive(WorkloadLabel)]
/// enum Phase { Input, Sim, Cleanup }
///
/// fn poll() {}
/// fn step() {}
/// fn sweep() {}
///
/// let mut schedule = Schedule::new();
/// schedule.add_workload(Phase::Input, |w| { w.add_system(poll); });
/// schedule
///     .add_workload(Phase::Sim, |w| { w.add_system(step); })
///     .after(Phase::Input);
/// schedule
///     .add_workload(Phase::Cleanup, |w| { w.add_system(sweep); })
///     .after(Phase::Sim);
/// ```
pub struct WorkloadOrderBuilder<'s> {
    schedule: &'s mut Schedule,
    idx: usize,
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "a WorkloadLabel is a throwaway unit-enum variant constructed inline at \
              the call site (`.after(Grid::Supply)`); taking it by value keeps that \
              ergonomic — there is nothing worth borrowing."
)]
impl WorkloadOrderBuilder<'_> {
    /// Orders this workload **after** the workload labelled `label`.
    /// Accumulates; resolves lazily at build (forward references allowed).
    pub fn after<L: WorkloadLabel>(&mut self, label: L) -> &mut Self {
        self.schedule.workload_edges.push(WorkloadEdge {
            subject: self.idx,
            kind: EdgeKind::After,
            target: label.id(),
            target_name: label.name(),
        });
        self.schedule.order = None;
        self
    }

    /// Orders this workload **before** the workload labelled `label`.
    /// Accumulates; resolves lazily at build.
    pub fn before<L: WorkloadLabel>(&mut self, label: L) -> &mut Self {
        self.schedule.workload_edges.push(WorkloadEdge {
            subject: self.idx,
            kind: EdgeKind::Before,
            target: label.id(),
            target_name: label.name(),
        });
        self.schedule.order = None;
        self
    }

    /// Acknowledges that this workload and `label` conflict but their order
    /// is intentionally undefined — silences the workload conflict-policy
    /// error for that pair.
    pub fn ambiguous_with<L: WorkloadLabel>(&mut self, label: L) -> &mut Self {
        self.schedule.workload_ambiguous.push(WorkloadAmbiguity {
            subject: self.idx,
            other: label.id(),
        });
        self.schedule.order = None;
        self
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
    use crate::{Commands, Component, Query, Res, ResMut, Resource, WorkloadLabel};

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

    // ── Schedule: tier-0 anonymous workload ─────────────────────────────

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
        assert_eq!(schedule.batches().len(), 1);
        assert_eq!(schedule.batches()[0].len(), 2);
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
    fn run_flushes_commands_after_the_workload() {
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
    fn add_systems_registers_an_unordered_group() {
        fn a() {}
        fn b() {}
        fn c() {}
        let mut schedule = Schedule::new();
        schedule.add_systems((a, b, c));
        // No params → no conflict → one batch of three.
        assert_eq!(schedule.batches().len(), 1);
        assert_eq!(schedule.batches()[0].len(), 3);
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

    #[test]
    #[should_panic(expected = "conflicting access to component")]
    fn registering_self_conflicting_query_system_panics() {
        fn weird(mut q1: Query<&mut Position>, mut q2: Query<&mut Position>) {
            for p in q1.iter_mut() {
                p.x += 1.0;
            }
            for p in q2.iter_mut() {
                p.x += 1.0;
            }
        }
        Schedule::new().add_system(weird);
    }

    #[test]
    #[should_panic(expected = "conflicting access to resource")]
    fn registering_resource_self_conflicting_system_panics() {
        fn weird(_r: Res<Score>, mut w: ResMut<Score>) {
            w.value += 1;
        }
        Schedule::new().add_system(weird);
    }

    #[test]
    #[should_panic(expected = "no order is declared")]
    fn tier_0_conflict_is_a_registration_error() {
        // Two top-level systems conflict on Score with no way to order them
        // (tier-0 hands back no handle) — rejected at build, pointing the
        // user to a workload.
        fn writer(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn reader(s: Res<Score>) {
            let _ = s.value;
        }
        let mut world = World::new();
        world.add_resource(Score { value: 0 });
        let mut schedule = Schedule::new();
        schedule.add_system(writer);
        schedule.add_system(reader);
        schedule.run(&mut world);
    }

    // ── Schedule: named workloads ───────────────────────────────────────

    #[test]
    fn workload_systems_run_in_declared_order() {
        #[derive(WorkloadLabel)]
        enum W {
            A,
        }
        fn first(mut log: ResMut<Log>) {
            log.order.push("first");
        }
        fn second(mut log: ResMut<Log>) {
            log.order.push("second");
        }
        let mut world = World::new();
        world.add_resource(Log { order: Vec::new() });
        let mut schedule = Schedule::new();
        schedule.add_workload(W::A, |w| {
            let f = w.add_system(first).id();
            w.add_system(second).after(f);
        });
        schedule.run(&mut world);
        assert_eq!(world.resource::<Log>().order, vec!["first", "second"]);
    }

    #[test]
    fn ambiguous_with_silences_a_conflicting_system_pair() {
        #[derive(WorkloadLabel)]
        enum W {
            A,
        }
        fn sweep(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn compact(mut s: ResMut<Score>) {
            s.value += 1;
        }
        let mut world = World::new();
        world.add_resource(Score { value: 0 });
        let mut schedule = Schedule::new();
        schedule.add_workload(W::A, |w| {
            let s = w.add_system(sweep).id();
            w.add_system(compact).ambiguous_with(s); // both write Score; OK
        });
        schedule.run(&mut world); // no panic
        assert_eq!(world.resource::<Score>().value, 2);
    }

    #[test]
    fn workloads_run_in_label_declared_order() {
        #[derive(WorkloadLabel)]
        enum W {
            Supply,
            Distribute,
        }
        fn supply(mut log: ResMut<Log>) {
            log.order.push("supply");
        }
        fn distribute(mut log: ResMut<Log>) {
            log.order.push("distribute");
        }
        let mut world = World::new();
        world.add_resource(Log { order: Vec::new() });
        let mut schedule = Schedule::new();
        // Register Distribute first, but order it after Supply (forward ref).
        schedule
            .add_workload(W::Distribute, |w| {
                w.add_system(distribute);
            })
            .after(W::Supply);
        schedule.add_workload(W::Supply, |w| {
            w.add_system(supply);
        });
        schedule.run(&mut world);
        assert_eq!(world.resource::<Log>().order, vec!["supply", "distribute"]);
    }
}
