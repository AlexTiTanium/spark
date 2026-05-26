//! Read/write bookkeeping for queries and systems.
//!
//! Two layers live here. [`QueryAccess`] is what one query *claims* it
//! touches — the component [`TypeId`]s it reads and writes — and powers
//! per-query self-conflict detection at [`Query::from_world`] time, so
//! `(&mut A, &A)` panics naming the offending component instead of the
//! [`RefCell`]'s cryptic "already borrowed". [`Access`] lifts that to a
//! whole *system*: the union of every [`SystemParam`]'s reads and writes,
//! kept in a component set and a resource set, with
//! [`Access::compatible_with`] as the cross-system conflict rule the
//! scheduler batches on.
//!
//! [`Query::from_world`]: crate::Query::from_world
//! [`RefCell`]: std::cell::RefCell
//! [`SystemParam`]: crate::SystemParam

use std::any::{TypeId, type_name};

/// How two access sets clash on a shared type — the companion to the
/// boolean [`Access::compatible_with`] that *names the reason*.
///
/// [`Access::find_conflicts`] returns one of these per offending
/// [`TypeId`] so a diagnostic can say *why* two systems (or two
/// workloads) cannot run without a declared order. Read/read overlap is
/// never a conflict, so it has no variant here.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Access, ConflictKind};
///
/// struct Position(f32, f32);
///
/// let mut writer = Access::new();
/// writer.components_mut().add_write::<Position>();
/// let mut reader = Access::new();
/// reader.components_mut().add_read::<Position>();
///
/// let conflicts = writer.find_conflicts(&reader);
/// assert_eq!(conflicts.len(), 1);
/// assert_eq!(conflicts[0].1, ConflictKind::WriteRead);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictKind {
    /// Both sides write the same type — the strongest clash.
    WriteWrite,
    /// One side writes a type the other reads.
    WriteRead,
}

/// What a query reads and writes — one entry per component type
/// referenced in the data shape, identified by [`TypeId`] and carrying
/// its type name for diagnostics.
///
/// `&T` in a [`QueryData`](crate::QueryData) shape adds a read,
/// `&mut T` adds a write; tuples concatenate their elements' access in
/// shape order. Two entries with the same `TypeId` is a *self-conflict*
/// iff at least one is a write — [`assert_no_self_conflict`] catches it.
///
/// # Why both `TypeId` and a name
///
/// [`TypeId`] is what the scheduler will need (set operations); the name
/// is what makes the panic message readable. Capturing
/// [`type_name::<T>()`] at the impl site, where `T` is a real generic
/// parameter, is the only way to surface a component name in the
/// diagnostic — by the time [`assert_no_self_conflict`] runs the types
/// are erased.
///
/// # Examples
///
/// ```
/// use spark_ecs::QueryAccess;
///
/// struct Position(f32, f32);
/// struct Velocity(f32, f32);
///
/// let mut access = QueryAccess::default();
/// access.add_write::<Position>();
/// access.add_read::<Velocity>();
/// access.assert_no_self_conflict();   // distinct types, no conflict
/// ```
///
/// A self-conflict panics:
///
/// ```should_panic
/// use spark_ecs::QueryAccess;
///
/// struct Position(f32, f32);
///
/// let mut access = QueryAccess::default();
/// access.add_write::<Position>();
/// access.add_read::<Position>();
/// access.assert_no_self_conflict();   // panics naming `Position`
/// ```
///
/// [`assert_no_self_conflict`]: Self::assert_no_self_conflict
#[derive(Default)]
pub struct QueryAccess {
    reads: Vec<Entry>,
    writes: Vec<Entry>,
}

/// One `(TypeId, name)` pair — the name is captured at the impl site so a
/// conflict panic can name the offending type after the types are erased.
#[derive(Clone, Copy)]
struct Entry {
    id: TypeId,
    name: &'static str,
}

impl Entry {
    /// Records `T`'s [`TypeId`] alongside its [`type_name`] for diagnostics.
    fn of<T: 'static>() -> Self {
        Self {
            id: TypeId::of::<T>(),
            name: type_name::<T>(),
        }
    }
}

impl QueryAccess {
    /// Records that the query reads `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::QueryAccess;
    ///
    /// struct Health(u32);
    ///
    /// let mut access = QueryAccess::default();
    /// access.add_read::<Health>();
    /// access.assert_no_self_conflict();
    /// ```
    pub fn add_read<T: 'static>(&mut self) {
        self.reads.push(Entry::of::<T>());
    }

    /// Records that the query writes `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::QueryAccess;
    ///
    /// struct Health(u32);
    ///
    /// let mut access = QueryAccess::default();
    /// access.add_write::<Health>();
    /// access.assert_no_self_conflict();
    /// ```
    pub fn add_write<T: 'static>(&mut self) {
        self.writes.push(Entry::of::<T>());
    }

    /// Panics if any single component is written and either written or
    /// read elsewhere in the same query — `(&mut A, &mut A)` or
    /// `(&mut A, &A)` in either element order. Two reads of the same
    /// `T` never conflict.
    ///
    /// Runs at [`crate::Query::from_world`] time, *before* any storage
    /// borrow. Today the `RefCell` storage would also catch the same
    /// case (as "already borrowed") on the second `init_state` call;
    /// this explicit check fires first so the diagnostic names the
    /// offending component, and stays the sole guard once M4 swaps
    /// storages to `UnsafeCell`.
    ///
    /// # Panics
    ///
    /// Panics with `"query has conflicting access to component
    /// `{name}`"` where `{name}` is the offending type's
    /// [`std::any::type_name`].
    pub fn assert_no_self_conflict(&self) {
        self.assert_no_self_conflict_named("query", "component");
    }

    /// Shared self-conflict loop. `scope`/`kind` shape the message so the
    /// per-query check reads `("query", "component")` and the per-system
    /// check via [`Access`] reads `("system", "component" | "resource")`.
    fn assert_no_self_conflict_named(&self, scope: &str, kind: &str) {
        for (i, w) in self.writes.iter().enumerate() {
            assert!(
                !self.writes[i + 1..].iter().any(|other| other.id == w.id),
                "{scope} has conflicting access to {kind} `{}` (written twice)",
                w.name
            );
            assert!(
                !self.reads.iter().any(|other| other.id == w.id),
                "{scope} has conflicting access to {kind} `{}` (written and read)",
                w.name
            );
        }
    }

    /// Returns `true` if `self` and `other` can run concurrently — no
    /// component written by one is read or written by the other.
    ///
    /// The check is symmetric
    /// (`a.is_compatible_with(b) == b.is_compatible_with(a)`) and exempts
    /// read/read overlap: any number of queries may read the same
    /// component at once. A single write is what forces serialisation,
    /// because a `&mut T` aliasing any other `&T`/`&mut T` is the data
    /// race the M4 parallel executor must rule out. This is the
    /// component half of [`Access::compatible_with`].
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::QueryAccess;
    ///
    /// struct Position(f32, f32);
    /// struct Velocity(f32, f32);
    ///
    /// // One query writes Position; another only reads Velocity.
    /// let mut writer = QueryAccess::default();
    /// writer.add_write::<Position>();
    /// let mut reader = QueryAccess::default();
    /// reader.add_read::<Velocity>();
    /// assert!(writer.is_compatible_with(&reader));    // disjoint
    ///
    /// // A second reader of Position clashes with the writer.
    /// let mut pos_reader = QueryAccess::default();
    /// pos_reader.add_read::<Position>();
    /// assert!(!writer.is_compatible_with(&pos_reader)); // write vs read
    /// ```
    #[must_use]
    pub fn is_compatible_with(&self, other: &QueryAccess) -> bool {
        // write/write and self-write/other-read are the first two checks;
        // other-write/self-read is the mirror (read/read never conflicts).
        !aliases(&self.writes, &other.writes)
            && !aliases(&self.writes, &other.reads)
            && !aliases(&other.writes, &self.reads)
    }

    /// Pushes every conflicting `(TypeId, ConflictKind)` between `self`
    /// and `other` into `out` — the same three checks
    /// [`is_compatible_with`](Self::is_compatible_with) folds into a
    /// bool, but recording *which* type clashed and *how*.
    ///
    /// Write/write is pushed before write/read so a consumer that takes
    /// the first conflict reports the strongest clash for that type.
    fn collect_conflicts(&self, other: &QueryAccess, out: &mut Vec<(TypeId, ConflictKind)>) {
        for w in &self.writes {
            if other.writes.iter().any(|o| o.id == w.id) {
                out.push((w.id, ConflictKind::WriteWrite));
            }
        }
        for w in &self.writes {
            if other.reads.iter().any(|o| o.id == w.id) {
                out.push((w.id, ConflictKind::WriteRead));
            }
        }
        for w in &other.writes {
            if self.reads.iter().any(|o| o.id == w.id) {
                out.push((w.id, ConflictKind::WriteRead));
            }
        }
    }

    /// The diagnostic name recorded for `id`, if this set references it.
    /// Looks in both reads and writes — a type appears in at most one
    /// per [`assert_no_self_conflict`](Self::assert_no_self_conflict).
    fn name_of(&self, id: TypeId) -> Option<&'static str> {
        self.reads
            .iter()
            .chain(self.writes.iter())
            .find(|e| e.id == id)
            .map(|e| e.name)
    }

    /// Folds `other`'s reads and writes into `self`. Used to aggregate a
    /// workload's access from its members; entries are not deduplicated
    /// (the conflict checks tolerate repeats, and the lists stay tiny).
    fn extend(&mut self, other: &QueryAccess) {
        self.reads.extend_from_slice(&other.reads);
        self.writes.extend_from_slice(&other.writes);
    }
}

/// Returns `true` if any [`Entry`] in `writes` shares a [`TypeId`] with
/// an entry in `against`. The inner primitive behind every conflict
/// check; the lists are tiny (one entry per referenced type), so the
/// nested scan is cheaper than building a hash set.
fn aliases(writes: &[Entry], against: &[Entry]) -> bool {
    writes
        .iter()
        .any(|w| against.iter().any(|other| other.id == w.id))
}

/// The reads and writes of a whole *system* — the union of every
/// [`SystemParam`](crate::SystemParam)'s declared access, split into a
/// component set and a resource set.
///
/// # Logic
///
/// Each [`SystemParam`](crate::SystemParam) folds its access in via
/// [`collect_access`](crate::SystemParam::collect_access): `Res<T>` adds
/// a resource read, `ResMut<T>` a resource write, and `Query<D, F>` adds
/// component reads/writes by replaying `D` and `F` through
/// [`components_mut`](Self::components_mut). `Commands` adds nothing — it
/// mutates structure through a deferred queue, not component or resource
/// storage. Two systems *conflict* iff one writes something the other
/// reads or writes, in the **same** set.
///
/// # Memory layout
///
/// ```text
/// Access
/// ├── components: QueryAccess { reads: [Velocity], writes: [Position] }
/// └── resources:  QueryAccess { reads: [Time],     writes: []         }
/// ```
///
/// Both sets are [`QueryAccess`] — reused here purely as a read/write set
/// with a conflict rule; the "query" in the name is incidental for the
/// resource set. Same rule, two domains.
///
/// # Why the two sets stay separate
///
/// Components and resources live in different storages, so the *same*
/// Rust type used as both — a `Foo` that is both a `Component` and a
/// `Resource` — names two unrelated slots. A system reading resource
/// `Foo` and one writing component `Foo` touch disjoint memory and must
/// **not** be reported as conflicting. Folding both into one
/// `TypeId` set would invent that false conflict and needlessly
/// serialise them. The split is what keeps [`compatible_with`] both
/// sound (never misses a real alias) and precise (never invents one).
///
/// # Why it is the parallelism safety proof, not just a lint
///
/// The M4 executor will hand same-batch systems to worker threads with
/// only a shared `&World`. Soundness rests entirely on this: two systems
/// in one batch have disjoint writes, so no thread can observe a torn or
/// racing mutation. The sequential executor that ships today already
/// computes and trusts this proof — M4 only swaps the walk for Rayon.
///
/// [`compatible_with`]: Self::compatible_with
///
/// # Examples
///
/// ```
/// use spark_ecs::Access;
///
/// struct Position(f32, f32);   // a component
/// struct Frame(u64);           // a resource
///
/// // `movement` writes Position; `tick` writes Frame; `report` reads Frame.
/// let mut mover = Access::new();
/// mover.components_mut().add_write::<Position>();
/// let mut ticker = Access::new();
/// ticker.add_resource_write::<Frame>();
/// let mut teller = Access::new();
/// teller.add_resource_read::<Frame>();
///
/// assert!(mover.compatible_with(&ticker));    // Position vs Frame: disjoint
/// assert!(!ticker.compatible_with(&teller));  // Frame write vs Frame read
/// assert!(teller.compatible_with(&teller));   // read/read is always fine
/// ```
#[derive(Default)]
pub struct Access {
    components: QueryAccess,
    /// Resource reads/writes. Reuses [`QueryAccess`] as a plain read/write
    /// set with a conflict rule — kept separate from `components` so a type
    /// used as both a component and a resource never cross-conflicts.
    resources: QueryAccess,
}

impl Access {
    /// Creates an empty access set — reads and writes nothing, so it is
    /// compatible with every other system.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Access;
    ///
    /// let a = Access::new();
    /// assert!(a.compatible_with(&Access::new()));
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the component-access set for in-place population.
    ///
    /// The seam a `Query<D, F>` parameter writes through: it replays
    /// `D::collect_access` and `F::collect_access` (which both take a
    /// `&mut QueryAccess`) into this set, reusing the exact machinery
    /// that powers per-query self-conflict detection.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Access;
    ///
    /// struct Health(u32);
    /// let mut access = Access::new();
    /// access.components_mut().add_write::<Health>();
    /// ```
    pub fn components_mut(&mut self) -> &mut QueryAccess {
        &mut self.components
    }

    /// Records that the system reads resource `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Access;
    ///
    /// struct Config(u32);
    /// let mut access = Access::new();
    /// access.add_resource_read::<Config>();
    /// ```
    pub fn add_resource_read<T: 'static>(&mut self) {
        self.resources.add_read::<T>();
    }

    /// Records that the system writes resource `T`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Access;
    ///
    /// struct Score(u32);
    /// let mut access = Access::new();
    /// access.add_resource_write::<Score>();
    /// ```
    pub fn add_resource_write<T: 'static>(&mut self) {
        self.resources.add_write::<T>();
    }

    /// Panics if this system's own parameters conflict with each other —
    /// two parameters writing the same component or resource, or one
    /// writing what another reads (e.g. `fn(Query<&mut Pos>, Query<&mut
    /// Pos>)`, or `fn(Res<A>, ResMut<A>)`).
    ///
    /// Called at registration so the conflict is refused *there*, naming
    /// the offending type, instead of surfacing later as a `RefCell`
    /// "already borrowed" panic when the system runs and fetches its
    /// second aliasing parameter. This is the intra-system mirror of
    /// [`QueryAccess::assert_no_self_conflict`]; the cross-system rule is
    /// [`compatible_with`](Self::compatible_with), which is separate.
    ///
    /// # Panics
    ///
    /// Panics with `"system has conflicting access to {component|resource}
    /// `{name}` …"`, naming the first offending type.
    pub fn assert_no_self_conflict(&self) {
        self.components
            .assert_no_self_conflict_named("system", "component");
        self.resources
            .assert_no_self_conflict_named("system", "resource");
    }

    /// Returns `true` if `self` and `other` can run concurrently — no
    /// component **and** no resource written by one is read or written by
    /// the other.
    ///
    /// Components and resources are checked independently — the same
    /// read/write conflict rule applied to each set — so a clash in
    /// either is enough to force serialisation, while a type that happens
    /// to name both a component and a resource never cross-contaminates.
    /// Read/read overlap is always compatible.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::Access;
    ///
    /// struct Score(u32);
    /// let mut writer = Access::new();
    /// writer.add_resource_write::<Score>();
    /// let mut reader = Access::new();
    /// reader.add_resource_read::<Score>();
    /// assert!(!writer.compatible_with(&reader)); // write vs read on Score
    /// ```
    #[must_use]
    pub fn compatible_with(&self, other: &Access) -> bool {
        self.components.is_compatible_with(&other.components)
            && self.resources.is_compatible_with(&other.resources)
    }

    /// Every conflicting type between `self` and `other`, paired with the
    /// kind of clash — the diagnostic companion to
    /// [`compatible_with`](Self::compatible_with).
    ///
    /// Where `compatible_with` answers *can these run together?*,
    /// `find_conflicts` answers *which types stop them, and how?* — the
    /// input the workload layer turns into a "these two conflict on
    /// `Position`, declare an order" message. Components and resources
    /// are checked in their own domains (a type used as both never
    /// cross-conflicts), and the returned [`TypeId`]s are resolved back
    /// to names with [`describe`](Self::describe).
    ///
    /// An empty result is exactly `compatible_with(other) == true`.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Access, ConflictKind};
    ///
    /// struct Score(u32);   // a resource
    ///
    /// let mut a = Access::new();
    /// a.add_resource_write::<Score>();
    /// let mut b = Access::new();
    /// b.add_resource_write::<Score>();
    ///
    /// let conflicts = a.find_conflicts(&b);
    /// assert_eq!(conflicts[0].1, ConflictKind::WriteWrite);
    /// assert!(!a.compatible_with(&b));
    /// ```
    #[must_use]
    pub fn find_conflicts(&self, other: &Access) -> Vec<(TypeId, ConflictKind)> {
        let mut out = Vec::new();
        self.components
            .collect_conflicts(&other.components, &mut out);
        self.resources.collect_conflicts(&other.resources, &mut out);
        out
    }

    /// The name and domain word (`"component"` / `"resource"`) recorded
    /// for `id`, if this set references it.
    ///
    /// `find_conflicts` returns bare [`TypeId`]s (the scheduler's
    /// currency); this resolves one back to the human-readable pieces a
    /// conflict message needs — `"Position"` and `"component"`. Returns
    /// `None` if `id` is not in this set.
    #[must_use]
    pub fn describe(&self, id: TypeId) -> Option<(&'static str, &'static str)> {
        if let Some(name) = self.components.name_of(id) {
            Some((name, "component"))
        } else {
            self.resources.name_of(id).map(|name| (name, "resource"))
        }
    }

    /// Folds `other`'s component and resource access into `self` — the
    /// union used to build a workload's aggregate access from its member
    /// systems, which cross-workload conflict detection then compares.
    pub(crate) fn extend(&mut self, other: &Access) {
        self.components.extend(&other.components);
        self.resources.extend(&other.resources);
    }

    /// The component [`TypeId`]s this system **writes**.
    ///
    /// Drives change detection: [`World::run_system`](crate::World::run_system)
    /// advances each of these storages' clocks once before the system
    /// runs, so the system's in-place edits stamp a tick strictly past
    /// any prior observation.
    pub(crate) fn component_write_ids(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.components.writes.iter().map(|e| e.id)
    }

    /// The component [`TypeId`]s this system reads **or** writes.
    ///
    /// After the system runs, [`World::run_system`](crate::World::run_system)
    /// records each storage's current tick as this system's "last seen"
    /// baseline, so its next run's `Changed<T>` / `Added<T>` compare
    /// against where it left off. A valid system never names the same
    /// component in both reads and writes (that self-conflict panics at
    /// registration), so in practice each `TypeId` appears once; the
    /// recorder upserts regardless, so any repeat is harmless.
    pub(crate) fn component_access_ids(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.components
            .reads
            .iter()
            .chain(self.components.writes.iter())
            .map(|e| e.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct A;
    struct B;

    #[test]
    fn empty_access_has_no_conflict() {
        QueryAccess::default().assert_no_self_conflict();
    }

    #[test]
    fn two_distinct_writes_are_fine() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_write::<B>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn write_plus_disjoint_read_is_fine() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_read::<B>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn two_reads_of_same_type_are_fine() {
        let mut access = QueryAccess::default();
        access.add_read::<A>();
        access.add_read::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "written twice")]
    fn two_writes_of_same_type_panic() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_write::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "written and read")]
    fn write_then_read_of_same_type_panics() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_read::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "written and read")]
    fn read_then_write_of_same_type_panics() {
        let mut access = QueryAccess::default();
        access.add_read::<A>();
        access.add_write::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn panic_message_names_offending_type() {
        let mut access = QueryAccess::default();
        access.add_write::<A>();
        access.add_write::<A>();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            access.assert_no_self_conflict();
        }));
        let payload = result.expect_err("expected panic");
        let msg = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .unwrap_or("");
        assert!(
            msg.contains("::A") || msg.contains("`A`"),
            "panic message did not name component `A`: {msg}"
        );
    }

    #[test]
    fn query_access_two_reads_are_compatible() {
        let mut a = QueryAccess::default();
        a.add_read::<A>();
        let mut b = QueryAccess::default();
        b.add_read::<A>();
        assert!(a.is_compatible_with(&b));
    }

    #[test]
    fn query_access_write_vs_read_conflicts_either_order() {
        let mut writer = QueryAccess::default();
        writer.add_write::<A>();
        let mut reader = QueryAccess::default();
        reader.add_read::<A>();
        assert!(!writer.is_compatible_with(&reader));
        assert!(!reader.is_compatible_with(&writer)); // symmetric
    }

    #[test]
    fn query_access_two_writes_conflict() {
        let mut a = QueryAccess::default();
        a.add_write::<A>();
        let mut b = QueryAccess::default();
        b.add_write::<A>();
        assert!(!a.is_compatible_with(&b));
    }

    #[test]
    fn query_access_disjoint_types_are_compatible() {
        let mut a = QueryAccess::default();
        a.add_write::<A>();
        let mut b = QueryAccess::default();
        b.add_write::<B>();
        assert!(a.is_compatible_with(&b));
    }

    #[test]
    fn empty_access_is_compatible_with_everything() {
        let empty = Access::new();
        let mut writer = Access::new();
        writer.add_resource_write::<A>();
        assert!(empty.compatible_with(&writer));
        assert!(writer.compatible_with(&empty));
        assert!(empty.compatible_with(&empty));
    }

    #[test]
    fn access_resource_write_conflicts_with_resource_read() {
        let mut writer = Access::new();
        writer.add_resource_write::<A>();
        let mut reader = Access::new();
        reader.add_resource_read::<A>();
        assert!(!writer.compatible_with(&reader));
    }

    #[test]
    fn access_component_and_resource_of_same_type_never_conflict() {
        // The same Rust type `A` used as a component in one system and a
        // resource in another names two unrelated storages — a write to
        // one must not be reported as conflicting with the other.
        let mut component_writer = Access::new();
        component_writer.components_mut().add_write::<A>();
        let mut resource_writer = Access::new();
        resource_writer.add_resource_write::<A>();
        assert!(component_writer.compatible_with(&resource_writer));
    }

    #[test]
    fn access_component_write_conflicts_with_component_read() {
        let mut writer = Access::new();
        writer.components_mut().add_write::<A>();
        let mut reader = Access::new();
        reader.components_mut().add_read::<A>();
        assert!(!writer.compatible_with(&reader));
    }

    #[test]
    #[should_panic(expected = "system has conflicting access to component")]
    fn access_self_conflict_component_written_twice_panics() {
        // Two parameters both writing component `A` — the system-level
        // mirror of a per-query self-conflict.
        let mut access = Access::new();
        access.components_mut().add_write::<A>();
        access.components_mut().add_write::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    #[should_panic(expected = "system has conflicting access to resource")]
    fn access_self_conflict_resource_written_and_read_panics() {
        let mut access = Access::new();
        access.add_resource_write::<A>();
        access.add_resource_read::<A>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn access_disjoint_params_have_no_self_conflict() {
        // Component `A` and resource `A` are different domains, so writing
        // both is not a self-conflict; distinct resource reads are fine too.
        let mut access = Access::new();
        access.components_mut().add_write::<A>();
        access.add_resource_write::<A>();
        access.add_resource_read::<B>();
        access.assert_no_self_conflict();
    }

    #[test]
    fn find_conflicts_reports_write_write() {
        let mut a = Access::new();
        a.components_mut().add_write::<A>();
        let mut b = Access::new();
        b.components_mut().add_write::<A>();
        let conflicts = a.find_conflicts(&b);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, TypeId::of::<A>());
        assert_eq!(conflicts[0].1, ConflictKind::WriteWrite);
    }

    #[test]
    fn find_conflicts_reports_write_read_in_either_order() {
        // self writes, other reads.
        let mut writer = Access::new();
        writer.components_mut().add_write::<A>();
        let mut reader = Access::new();
        reader.components_mut().add_read::<A>();
        assert_eq!(writer.find_conflicts(&reader)[0].1, ConflictKind::WriteRead);
        // Mirror: other writes, self reads — still WriteRead.
        assert_eq!(reader.find_conflicts(&writer)[0].1, ConflictKind::WriteRead);
    }

    #[test]
    fn find_conflicts_empty_iff_compatible() {
        let mut a = Access::new();
        a.components_mut().add_write::<A>();
        let mut b = Access::new();
        b.components_mut().add_write::<B>();
        assert!(a.find_conflicts(&b).is_empty());
        assert!(a.compatible_with(&b));
    }

    #[test]
    fn find_conflicts_keeps_component_and_resource_domains_apart() {
        // Component `A` vs resource `A`: different storages, no conflict.
        let mut component_writer = Access::new();
        component_writer.components_mut().add_write::<A>();
        let mut resource_writer = Access::new();
        resource_writer.add_resource_write::<A>();
        assert!(component_writer.find_conflicts(&resource_writer).is_empty());
    }

    #[test]
    fn describe_resolves_name_and_domain() {
        let mut access = Access::new();
        access.components_mut().add_write::<A>();
        access.add_resource_read::<B>();
        let (name, domain) = access.describe(TypeId::of::<A>()).expect("A is present");
        assert!(name.ends_with("::A") || name == "A");
        assert_eq!(domain, "component");
        assert_eq!(
            access.describe(TypeId::of::<B>()).map(|(_, d)| d),
            Some("resource")
        );
    }
}
