//! A **workload** — a parallel-capable group of systems — and the
//! within-group ordering it carries.
//!
//! Spark registers work two ways, by intent, through two separate
//! mechanisms that share only the frame `Stage` they sit in. **Sequential**
//! systems live on `spark_core::Application` (`app.add_system(stage, fn)`):
//! they run in the calling thread, in registration order, and never reach
//! this module. A **parallel-capable group** is a *workload*:
//! [`add_workload(label, |w| …)`](crate::Schedule::add_workload) on a
//! [`Schedule`](crate::Schedule) collects systems the scheduler partitions
//! into access-disjoint batches. This module owns everything *inside* a
//! workload, leaving [`Schedule`](crate::Schedule) only the cross-workload
//! orchestration:
//!
//! - [`WorkloadLabel`] + [`WorkloadId`] — the stable identity a
//!   `#[derive(WorkloadLabel)]` enum hands the scheduler.
//! - [`WorkloadBuilder`] / [`SystemRef`] — the `|w| { … }` closure API for
//!   adding systems and ordering them against each other by handle.
//! - [`System`] / [`SystemId`] — a workload's stored systems and the
//!   index that names one; [`build_batches`] groups them into
//!   access-disjoint batches.
//! - The graph primitives ([`topo_sort`], [`reachable_in`],
//!   [`successors_of`]) that turn `.after`/`.before` edges into an
//!   execution order.
//! - The conflict-policy checks that turn an *undeclared* write-overlap
//!   into a registration error.
//!
//! # The two levels, one verb
//!
//! Ordering reads the same at both levels — `.after` / `.before` — only
//! the argument differs. **Systems** order against a [`SystemRef`] handle:
//! [`add_system`](WorkloadBuilder::add_system) hands one back directly (as
//! a [`SystemOrderBuilder`] that *is* the handle), so a later system says
//! `.after(that_handle)` with no extra step. **Workloads** order against a
//! [`WorkloadLabel`] (the
//! [`WorkloadOrderBuilder`](crate::WorkloadOrderBuilder) returned by
//! [`Schedule::add_workload`](crate::Schedule::add_workload)). Handles, not
//! function items, because the same `fn` registered twice must stay two
//! distinct systems.

use std::any::TypeId;
use std::cell::RefCell;
use std::collections::VecDeque;

use crate::access::{Access, ConflictKind};
use crate::system::IntoSystem;
use crate::world::World;

/// Stable identity for a workload label — an enum type plus a variant
/// index, the pair `#[derive(WorkloadLabel)]` emits per variant.
///
/// The [`TypeId`] names *which* label enum (so `Grid::Tick` and
/// `Workers::Tick` never collide even at the same variant index); the
/// `variant` is the 0-based declaration position. Both fields together are
/// a globally unique, allocation-free key the scheduler stores in
/// ordering edges and resolves to a workload at build time.
///
/// You never build one by hand — the derive does, via the hidden
/// [`new`](Self::new) constructor.
///
/// # Examples
///
/// ```
/// use spark_ecs::WorkloadLabel;
///
/// #[derive(WorkloadLabel)]
/// enum Grid { Supply, Distribute }
///
/// // Same variant, different enums ⇒ different ids.
/// assert_ne!(Grid::Supply.id(), Grid::Distribute.id());
/// assert_eq!(Grid::Supply.id(), Grid::Supply.id());
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WorkloadId {
    type_id: TypeId,
    variant: usize,
}

impl WorkloadId {
    /// Builds an id from a label enum's [`TypeId`] and a variant index.
    ///
    /// Hidden because only `#[derive(WorkloadLabel)]` should call it — it
    /// is generated code's constructor, not a public API. Visible across
    /// crates so the derive's `::spark_ecs::WorkloadId::new(…)` resolves.
    #[doc(hidden)]
    #[must_use]
    pub fn new(type_id: TypeId, variant: usize) -> Self {
        Self { type_id, variant }
    }
}

/// Stable identity + display name for a workload, one per variant of a
/// `#[derive(WorkloadLabel)]` enum.
///
/// Mirrors the shape a future `StageLabel` would take: an [`id`](Self::id)
/// the scheduler orders against and a [`name`](Self::name) for
/// diagnostics. The derive matches over the enum's unit variants to
/// generate both — which is why it applies to an enum (one variant = one
/// label), not a unit struct.
///
/// # Examples
///
/// ```
/// use spark_ecs::WorkloadLabel;
///
/// #[derive(WorkloadLabel)]
/// enum Sim { Input, Physics }
///
/// assert_eq!(Sim::Physics.name(), "Sim::Physics");
/// ```
///
/// Deriving on anything but an enum is a compile error — there is no
/// single label to generate from a struct:
///
/// ```compile_fail
/// use spark_ecs::WorkloadLabel;
///
/// #[derive(WorkloadLabel)]
/// struct NotAnEnum;
/// ```
pub trait WorkloadLabel: 'static {
    /// This label's stable identity — `(enum TypeId, variant index)`.
    fn id(&self) -> WorkloadId;

    /// This label's qualified name, e.g. `"Grid::Distribute"`. Used in
    /// conflict and cycle diagnostics, so it carries the enum name too.
    fn name(&self) -> &'static str;
}

/// A handle to a system registered inside a [`WorkloadBuilder`] closure —
/// what `.after` / `.before` order against.
///
/// [`WorkloadBuilder::add_system`] hands one back directly, wrapped in a
/// [`SystemOrderBuilder`] that derefs and converts to this `SystemRef` (so
/// the builder *is* the handle — no `.id()` step). It is a position within
/// *that one workload*: ordering against the same `fn` added twice yields
/// two distinct handles, which is the whole reason ordering keys off
/// handles rather than function items.
///
/// # How NOT to use
///
/// A `SystemRef` is meaningful only inside the workload closure that
/// produced it — its index points into *that* workload's system list.
/// Feeding one to a different workload's `.after`/`.before` would order
/// against an unrelated system. In debug builds a stamped workload id
/// catches exactly that: the ordering methods assert the handle came from
/// the workload they belong to, turning a silent mis-order into a panic.
/// (The check is a `debug_assert`, compiled out of release; the handle is
/// a transient build-time value, so the stamped id costs nothing that
/// matters.)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SystemRef {
    idx: usize,
    /// The workload this handle was minted in. The ordering methods
    /// `debug_assert` against it, so a cross-workload handle panics in
    /// debug instead of silently aliasing the wrong system; the check is
    /// compiled out in release.
    workload: WorkloadId,
}

/// Identifies a system within a workload — its registration index.
///
/// Returned inside the batch lists from [`Schedule::batches`](crate::Schedule::batches).
/// The inner value is a position in that workload's system list, so
/// [`usize`] (the natural index type) avoids any cast on the hot path.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Schedule, WorkloadLabel};
///
/// #[derive(WorkloadLabel)]
/// enum W { Tick }
///
/// fn noop() {}
/// let mut schedule = Schedule::new();
/// schedule.add_workload(W::Tick, |w| {
///     w.add_system(noop);
/// });
/// let first = schedule.batches(W::Tick)[0][0];
/// assert_eq!(first, first); // `SystemId` is comparable
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SystemId(pub(crate) usize);

/// A registered, runnable system: its erased run closure, its declared
/// [`Access`], its name (the fn's type path, for diagnostics —
/// overridable with [`SystemOrderBuilder::label`]), and its per-component
/// change-detection baselines.
///
/// This is the **one** concrete system record both scheduling paths store:
/// the workload scheduler keeps a `Vec<System>` per workload and batches
/// them by [`Access`]; `spark_core`'s sequential `Application` keeps a
/// `Vec<System>` per stage and runs them in registration order. Both build
/// it with [`from_system`](Self::from_system) and drive it with
/// [`run`](Self::run), so the access-extraction, self-conflict check, and
/// change-detection wiring live here once rather than duplicated per
/// scheduler.
///
/// `last_seen` records, per component this system accesses, the tick that
/// component's clock read when the system last ran. [`run`](Self::run)
/// feeds it to [`World::run_system`](crate::World::run_system), which
/// parks it as the [`Changed`](crate::Changed) / [`Added`](crate::Added)
/// baseline and refreshes it. Starts empty, so a system's first run sees
/// every prior-existing component (baseline defaults to 0).
///
/// # Examples
///
/// ```
/// use spark_ecs::{ResMut, Resource, System, World};
///
/// #[derive(Resource)]
/// struct Score(u32);
///
/// let mut world = World::new();
/// world.add_resource(Score(0));
///
/// let mut system = System::from_system(|mut s: ResMut<Score>| s.0 += 1);
/// system.run(&mut world);
/// system.run(&mut world);
/// assert_eq!(world.resource::<Score>().0, 2);
/// ```
pub struct System {
    pub(crate) name: &'static str,
    pub(crate) access: Access,
    run: Box<dyn FnMut(&World) + 'static>,
    last_seen: Vec<(TypeId, u32)>,
}

impl System {
    /// Boxes a system fn, capturing its name and declared [`Access`] and
    /// refusing a self-conflicting system up front.
    ///
    /// The single construction path for every registered system — the
    /// workload builder ([`WorkloadBuilder::add_system`] /
    /// [`add_systems`](WorkloadBuilder::add_systems)) and `spark_core`'s
    /// sequential `Application::add_system` all call it, so each rejects
    /// conflicts and wires up change detection identically.
    ///
    /// # Panics
    ///
    /// Panics if the system's own parameters conflict — two writing the
    /// same component/resource, or one writing what another reads. See
    /// [`Access::assert_no_self_conflict`].
    pub fn from_system<S, Marker>(system: S) -> Self
    where
        S: IntoSystem<Marker>,
    {
        let access = <S as IntoSystem<Marker>>::access();
        access.assert_no_self_conflict();
        Self {
            name: std::any::type_name::<S>(),
            access,
            run: system.into_system(),
            last_seen: Vec::new(),
        }
    }

    /// Runs this system against `world` with per-component change
    /// detection, updating its `last_seen` baselines. Thin wrapper over
    /// [`World::run_system`](crate::World::run_system) — the tick dance
    /// lives there, shared by every caller of this method.
    pub fn run(&mut self, world: &mut World) {
        world.run_system(&self.access, &mut self.last_seen, &mut *self.run);
    }
}

/// Whether an ordering edge runs its target *after* or *before* the
/// subject workload. Resolved to a directed `(before, after)` edge at
/// build time, once labels map to indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// Subject runs after the target: edge `target → subject`.
    After,
    /// Subject runs before the target: edge `subject → target`.
    Before,
}

/// One workload's contents: its systems and the intra-workload ordering
/// the builder collected.
///
/// Every workload carries a `label` (its [`WorkloadId`]) and a `name` (the
/// label's qualified text, for diagnostics). `edges` are explicit
/// `(before, after)` pairs by system index; `any_order` are acknowledged
/// don't-care pairs in canonical `(min, max)` order. `aggregate_access` is
/// the union of every member's access, used for cross-workload conflict
/// detection. `batches` caches the built plan.
///
/// `pub` only so it can appear in [`IntoSystemTuple`]'s (public) plumbing
/// method; `#[doc(hidden)]` with `pub(crate)` fields keeps it fully opaque
/// — there is nothing a downstream crate can build or read.
#[doc(hidden)]
pub struct WorkloadData {
    pub(crate) label: WorkloadId,
    pub(crate) name: &'static str,
    pub(crate) systems: Vec<System>,
    pub(crate) edges: Vec<(usize, usize)>,
    pub(crate) any_order: Vec<(usize, usize)>,
    pub(crate) aggregate_access: Access,
    pub(crate) batches: Vec<Vec<SystemId>>,
}

impl WorkloadData {
    /// Creates an empty workload with the given label and display name.
    pub(crate) fn new(label: WorkloadId, name: &'static str) -> Self {
        Self {
            label,
            name,
            systems: Vec::new(),
            edges: Vec::new(),
            any_order: Vec::new(),
            aggregate_access: Access::new(),
            batches: Vec::new(),
        }
    }

    /// Appends `boxed`, folding its access into the workload aggregate.
    /// The shared sink for [`WorkloadBuilder::add_system`] and
    /// [`add_systems`](WorkloadBuilder::add_systems).
    pub(crate) fn push(&mut self, boxed: System) -> usize {
        self.aggregate_access.extend(&boxed.access);
        let idx = self.systems.len();
        self.systems.push(boxed);
        idx
    }

    /// Validates this workload's systems against the conflict policy, then
    /// computes and caches its batch plan.
    ///
    /// The within-workload half of a schedule build: [`Schedule`] handles
    /// cross-workload ordering and calls this once per workload. Keeping
    /// the per-workload work here makes a workload self-contained — it
    /// owns its systems, its ordering, and the plan they produce.
    ///
    /// [`Schedule`]: crate::Schedule
    ///
    /// # Panics
    ///
    /// On an undeclared system conflict (see [`validate_system_conflicts`])
    /// or a cycle in this workload's `.after`/`.before` edges (see
    /// [`build_batches`]).
    pub(crate) fn build(&mut self) {
        validate_system_conflicts(self);
        self.batches = build_batches(&self.systems, &self.edges);
    }
}

/// The `w` in `add_workload(label, |w| { … })` — collects a workload's
/// systems and the ordering between them.
///
/// [`add_system`](Self::add_system) registers one system and hands back a
/// [`SystemOrderBuilder`] for `.after` / `.before` / `.any_order_with` /
/// `.label`; [`add_systems`](Self::add_systems) registers an unordered
/// tuple. The builder is consumed by the closure — its `WorkloadData` is
/// taken out afterwards by
/// [`Schedule::add_workload`](crate::Schedule::add_workload).
///
/// # Why interior mutability
///
/// Every method takes `&self`, not `&mut self`, and mutates through a
/// [`RefCell`]. That is the price of the ergonomic the directive fixes —
/// `add_system` returning a usable handle with no `.id()` step:
///
/// ```
/// # use spark_ecs::{Schedule, WorkloadLabel};
/// # #[derive(WorkloadLabel)] enum W { A }
/// # fn setup() {} fn render() {} fn cleanup() {}
/// # let mut schedule = Schedule::new();
/// schedule.add_workload(W::A, |w| {
///     let setup  = w.add_system(setup);              // store the handle…
///     let render = w.add_system(render).after(setup); // …and reuse it later
///     w.add_system(cleanup).after(render);
/// });
/// ```
///
/// If `add_system` borrowed `&mut self`, the stored `setup` would hold an
/// *exclusive* borrow of `w` for as long as it lived — and the next
/// `w.add_system(render)` could not borrow `w` again. With a `RefCell`,
/// each [`SystemOrderBuilder`] holds only a *shared* `&WorkloadBuilder`, so
/// any number of handles coexist and still record edges. The whole builder
/// lives for one `add_workload` call at startup, so the runtime borrow
/// check costs nothing that matters and can never alias (each method does
/// one short `borrow_mut` and releases it before returning).
///
/// # Examples
///
/// ```
/// use spark_ecs::{Schedule, WorkloadLabel};
///
/// #[derive(WorkloadLabel)]
/// enum Boot { Load }
///
/// fn read_files() {}
/// fn parse() {}
///
/// let mut schedule = Schedule::new();
/// schedule.add_workload(Boot::Load, |w| {
///     let files = w.add_system(read_files);
///     w.add_system(parse).after(files);
/// });
/// ```
pub struct WorkloadBuilder {
    /// The workload under construction, behind a [`RefCell`] so the
    /// `&self` ordering methods can record into it while shared
    /// [`SystemOrderBuilder`] handles are live (see *Why interior
    /// mutability* above).
    data: RefCell<WorkloadData>,
    /// The label this builder is for, stamped into every [`SystemRef`] it
    /// hands out so the ordering methods can reject a handle from a
    /// different workload. Duplicated here (it also lives in `data.label`)
    /// so it sits *outside* the `RefCell` — the `assert_same_workload` debug
    /// guard reads it without taking a borrow.
    id: WorkloadId,
}

impl WorkloadBuilder {
    /// Starts an empty builder for a labelled workload.
    pub(crate) fn new(label: WorkloadId, name: &'static str) -> Self {
        Self {
            data: RefCell::new(WorkloadData::new(label, name)),
            id: label,
        }
    }

    /// Registers `system` and hands back a [`SystemOrderBuilder`] — which
    /// is both the chainable `.after(handle)` / `.before(handle)` /
    /// `.any_order_with(handle)` / `.label(name)` builder *and*, directly,
    /// the [`SystemRef`] handle later systems order against (it derefs and
    /// converts to one — no `.id()` step).
    ///
    /// # Panics
    ///
    /// Panics if `system`'s own parameters conflict (two writing the same
    /// component/resource, or one writing what another reads) — refused
    /// here at registration, naming the type, the same as
    /// [`add_systems`](Self::add_systems).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Schedule, WorkloadLabel};
    ///
    /// #[derive(WorkloadLabel)]
    /// enum Startup { Init }
    ///
    /// fn load_config() {}
    /// fn spawn_player() {}
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_workload(Startup::Init, |w| {
    ///     let config = w.add_system(load_config);
    ///     w.add_system(spawn_player).after(config);
    /// });
    /// ```
    pub fn add_system<S, Marker>(&self, system: S) -> SystemOrderBuilder<'_>
    where
        S: IntoSystem<Marker>,
    {
        let idx = self.data.borrow_mut().push(System::from_system(system));
        SystemOrderBuilder {
            system: SystemRef {
                idx,
                workload: self.id,
            },
            builder: self,
        }
    }

    /// Registers a tuple of systems as an **unordered** group — no
    /// ordering is implied between them, and no handles are returned.
    ///
    /// If two systems in the tuple have a write-overlap, that is a
    /// conflict the build will reject (decision (a)) just as if they were
    /// added separately; reach for separate [`add_system`](Self::add_system)
    /// calls when you need to order or acknowledge them.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_ecs::{Schedule, WorkloadLabel};
    ///
    /// #[derive(WorkloadLabel)]
    /// enum Frame { Tick }
    ///
    /// fn step_ai() {}
    /// fn animate_sprites() {}
    ///
    /// let mut schedule = Schedule::new();
    /// schedule.add_workload(Frame::Tick, |w| {
    ///     w.add_systems((step_ai, animate_sprites));   // run together, order unspecified
    /// });
    /// ```
    pub fn add_systems<T, Marker>(&self, systems: T)
    where
        T: IntoSystemTuple<Marker>,
    {
        systems.register_into(&mut self.data.borrow_mut());
    }

    /// Takes the built workload out of the consumed builder.
    pub(crate) fn into_data(self) -> WorkloadData {
        self.data.into_inner()
    }
}

/// What [`WorkloadBuilder::add_system`] returns: a system's [`SystemRef`]
/// handle plus the `.after` / `.before` / `.any_order_with` / `.label`
/// methods to order it — the two roles in one value.
///
/// It is [`Copy`] and holds only a *shared* `&WorkloadBuilder`, so it can
/// be stored in a `let` and reused as a handle while later `add_system`
/// calls run (see [`WorkloadBuilder`]'s *Why interior mutability*). It
/// [`Deref`](std::ops::Deref)s to `SystemRef` and converts via
/// `Into<SystemRef>`, which is how `.after(handle)` accepts it with no
/// `.id()` step. The ordering methods take `self` and return `Self`: each
/// records its edge eagerly and yields the same handle, so a chain like
/// `add_system(f).after(a).before(b)` reads left to right and its result is
/// still a usable handle.
///
/// `.after` / `.before` **accumulate** — `.after(a).after(b)` means "after
/// both". `.any_order_with(h)` acknowledges an intentional don't-care with
/// a conflicting peer (silencing the conflict-policy error for that pair).
/// `.label(name)` overrides the system's diagnostic name — useful for a
/// closure (whose `type_name` is noise) or a `fn` added twice.
///
/// # Examples
///
/// ```
/// use spark_ecs::{Schedule, WorkloadLabel};
///
/// #[derive(WorkloadLabel)]
/// enum Assets { Load }
///
/// fn read_files() {}
/// fn parse_meshes() {}
/// fn parse_textures() {}
/// fn upload() {}
///
/// let mut schedule = Schedule::new();
/// schedule.add_workload(Assets::Load, |w| {
///     let files  = w.add_system(read_files);
///     let meshes = w.add_system(parse_meshes).after(files);
///     let texs   = w.add_system(parse_textures).after(files);
///     w.add_system(upload).after(meshes).after(texs);   // waits for both
/// });
/// ```
#[derive(Clone, Copy)]
pub struct SystemOrderBuilder<'b> {
    /// The handle this builder represents — also what `Deref` / `From`
    /// expose so the builder *is* the system's [`SystemRef`].
    system: SystemRef,
    /// Shared borrow of the parent builder, used to record edges. Shared
    /// (not `&mut`) is what lets several of these coexist as live handles.
    builder: &'b WorkloadBuilder,
}

#[allow(
    clippy::return_self_not_must_use,
    clippy::must_use_candidate,
    reason = "these methods record their edge into the workload as a side \
              effect and return the handle only so a chain can continue; the \
              result is meant to be dropped at the end of a chain \
              (`w.add_system(f).after(a);`), so #[must_use] would wrongly warn \
              on every terminal call."
)]
impl SystemOrderBuilder<'_> {
    /// Orders this system **after** `other` (edge `other → self`).
    /// Accumulates: call repeatedly to depend on several systems.
    ///
    /// Takes any handle (`other: impl Into<SystemRef>`) — a stored
    /// [`SystemRef`] or another [`SystemOrderBuilder`] — and returns this
    /// one for further chaining.
    pub fn after(self, other: impl Into<SystemRef>) -> Self {
        let other = other.into();
        self.assert_same_workload(other);
        self.builder
            .data
            .borrow_mut()
            .edges
            .push((other.idx, self.system.idx));
        self
    }

    /// Orders this system **before** `other` (edge `self → other`).
    /// Accumulates.
    pub fn before(self, other: impl Into<SystemRef>) -> Self {
        let other = other.into();
        self.assert_same_workload(other);
        self.builder
            .data
            .borrow_mut()
            .edges
            .push((self.system.idx, other.idx));
        self
    }

    /// Asserts that this system and `other` may run in **any order** — the
    /// observable result is independent of which runs first.
    ///
    /// They conflict (share a write), so they still land in separate
    /// batches; this only waives the *requirement to declare which comes
    /// first*, silencing the conflict-policy error for exactly this pair.
    /// With no edge between them, their relative batch order falls out of
    /// their topo positions (registration index, here).
    ///
    /// The scheduler **cannot verify** the commutativity you are asserting
    /// — it does not see the system bodies. If the two are not actually
    /// commutative under the conflict they share, you are introducing
    /// nondeterminism the type system cannot catch. Reach for `.after` /
    /// `.before` instead when in doubt.
    pub fn any_order_with(self, other: impl Into<SystemRef>) -> Self {
        let other = other.into();
        self.assert_same_workload(other);
        self.builder
            .data
            .borrow_mut()
            .any_order
            .push(canonical(self.system.idx, other.idx));
        self
    }

    /// Overrides this system's diagnostic name (for a closure or a `fn`
    /// added twice). Without it, the name is the function's `type_name`.
    pub fn label(self, name: &'static str) -> Self {
        self.builder.data.borrow_mut().systems[self.system.idx].name = name;
        self
    }

    /// Guards against a handle minted in a different workload (a stashed
    /// `SystemRef` fed to the wrong builder). The check is a `debug_assert`,
    /// so it costs nothing in release.
    fn assert_same_workload(&self, other: SystemRef) {
        debug_assert_eq!(
            other.workload, self.builder.id,
            "a SystemRef was used to order a system in a different workload than the \
             one that produced it"
        );
    }
}

/// The builder *is* its system's handle: `*w.add_system(f)` is the
/// [`SystemRef`], and a `&SystemOrderBuilder` coerces to `&SystemRef`
/// wherever one is wanted.
impl std::ops::Deref for SystemOrderBuilder<'_> {
    type Target = SystemRef;
    fn deref(&self) -> &SystemRef {
        &self.system
    }
}

/// Lets `.after(handle)` / `.before(handle)` / `.any_order_with(handle)`
/// accept a builder straight off `add_system`, and lets a builder be
/// stored as a plain [`SystemRef`] (`let h: SystemRef = w.add_system(f).into();`)
/// to escape the closure — the form the cross-workload guard test relies on.
impl From<SystemOrderBuilder<'_>> for SystemRef {
    fn from(builder: SystemOrderBuilder<'_>) -> Self {
        builder.system
    }
}

/// A tuple of systems that [`add_systems`](WorkloadBuilder::add_systems)
/// can register in one call.
///
/// `Marker` is a tuple of each system's own `IntoSystem` marker, carried
/// in the trait so the per-element marker type parameters stay
/// constrained — the same trick [`IntoSystem`] uses for arity.
/// Implemented for tuples of 1..=8 systems; longer tuples need another
/// `impl_into_system_tuple!` row.
///
/// **Sealed.** It is `pub` only because it is a bound on the public
/// [`add_systems`](WorkloadBuilder::add_systems) (a `pub(crate)` trait there
/// would trip the `private_bounds` lint, and the crate re-exports it). Its
/// one method takes the opaque, `#[doc(hidden)]` [`WorkloadData`], so
/// downstream crates cannot implement it — they only ever *use* the
/// provided tuple impls.
pub trait IntoSystemTuple<Marker> {
    /// Pushes every system in the tuple into `data`, unordered.
    ///
    /// Plumbing the builders call — hidden because `WorkloadData` is an
    /// opaque internal type, not something a caller constructs.
    #[doc(hidden)]
    fn register_into(self, data: &mut WorkloadData);
}

/// Emits one [`IntoSystemTuple`] impl per arity: destructure the tuple,
/// box each system, push it. See [`IntoSystem`]'s `impl_into_system!` for
/// the matching marker-tuple pattern.
//
// Codegen census: this is one of the crate's declarative-macro families;
// see the variant manifest at the top of `query/tuple_codegen.rs`.
macro_rules! impl_into_system_tuple {
    ($(($S:ident, $M:ident)),+) => {
        impl<$($S, $M),+> IntoSystemTuple<($($M,)+)> for ($($S,)+)
        where
            $($S: IntoSystem<$M>,)+
        {
            #[allow(non_snake_case, clippy::allow_attributes)]
            fn register_into(self, data: &mut WorkloadData) {
                let ($($S,)+) = self;
                $( data.push(System::from_system($S)); )+
            }
        }
    };
}

impl_into_system_tuple!((S1, M1));
impl_into_system_tuple!((S1, M1), (S2, M2));
impl_into_system_tuple!((S1, M1), (S2, M2), (S3, M3));
impl_into_system_tuple!((S1, M1), (S2, M2), (S3, M3), (S4, M4));
impl_into_system_tuple!((S1, M1), (S2, M2), (S3, M3), (S4, M4), (S5, M5));
impl_into_system_tuple!((S1, M1), (S2, M2), (S3, M3), (S4, M4), (S5, M5), (S6, M6));
impl_into_system_tuple!(
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7)
);
impl_into_system_tuple!(
    (S1, M1),
    (S2, M2),
    (S3, M3),
    (S4, M4),
    (S5, M5),
    (S6, M6),
    (S7, M7),
    (S8, M8)
);

/// Orders a pair into `(min, max)` so a don't-care acknowledgement reads
/// the same whichever system declared it.
pub(crate) fn canonical(a: usize, b: usize) -> (usize, usize) {
    if a <= b { (a, b) } else { (b, a) }
}

/// Builds the adjacency for `n` nodes from directed `(before, after)`
/// edges: `successors[node]` lists the nodes `node` points at. The shared
/// front half of both graph walks here — [`topo_sort`] and [`reachable_in`].
///
/// Duplicate edges are kept as-is (a node may appear in a successor list
/// more than once); both walks tolerate that.
pub(crate) fn successors_of(n: usize, edges: &[(usize, usize)]) -> Vec<Vec<usize>> {
    let mut successors = vec![Vec::new(); n];
    for &(before, after) in edges {
        successors[before].push(after);
    }
    successors
}

/// `true` if a directed path runs from `from` to `to` over a prebuilt
/// adjacency (`successors[node]` lists the nodes `node` points at).
///
/// A plain BFS. Callers build `successors` once with [`successors_of`] and
/// reuse it across many pair queries. Used to ask "did the programmer
/// already pin the relative order of these two systems?" — where a
/// *transitive* path counts: if `a → b → c` is declared, `a` and `c` have
/// a declared order even with no direct `a → c` edge.
pub(crate) fn reachable_in(from: usize, to: usize, successors: &[Vec<usize>]) -> bool {
    if from == to {
        return true;
    }
    let mut seen = vec![false; successors.len()];
    let mut queue = VecDeque::from([from]);
    seen[from] = true;
    while let Some(node) = queue.pop_front() {
        for &next in &successors[node] {
            if next == to {
                return true;
            }
            if !seen[next] {
                seen[next] = true;
                queue.push_back(next);
            }
        }
    }
    false
}

/// `true` if the user declared *any* relative order between `a` and `b`
/// over `successors` — a directed path either way. The exact question the
/// conflict policy asks of a clashing pair: "did you already say which
/// runs first?" Shared by the system- and workload-level validators.
pub(crate) fn has_declared_order(a: usize, b: usize, successors: &[Vec<usize>]) -> bool {
    reachable_in(a, b, successors) || reachable_in(b, a, successors)
}

/// Topologically orders `n` nodes given directed `(before, after)` edges.
///
/// # Logic
///
/// Kahn's algorithm: repeatedly emit a node with no remaining
/// predecessors, decrementing its successors' in-degrees. The result is
/// deterministic — the initial no-predecessor set is drained in ascending
/// index order, and nodes freed mid-walk are appended in edge order — so
/// for the same input the same order comes out every time. (The exact
/// ranks `build_batches` derives are independent of which valid topo order
/// this picks: longest-path depth is unique per node.)
///
/// # Why it works
///
/// A node is emitted only once every edge into it has been satisfied, so
/// the output respects every `(before, after)` constraint. If a cycle
/// exists, its nodes never reach in-degree zero and are never emitted —
/// so `processed < n` exactly when the graph is cyclic. Those leftover
/// nodes are returned as `Err` for the caller to render into a cycle path.
///
/// # Errors
///
/// Returns `Err(leftover)` — the nodes that could not be ordered — when
/// the edges contain a cycle. `Ok(order)` otherwise.
pub(crate) fn topo_sort(n: usize, edges: &[(usize, usize)]) -> Result<Vec<usize>, Vec<usize>> {
    let successors = successors_of(n, edges);
    let mut indegree = vec![0usize; n];
    for &(_, after) in edges {
        indegree[after] += 1;
    }

    let mut ready: VecDeque<usize> = (0..n).filter(|&i| indegree[i] == 0).collect();
    let mut order = Vec::with_capacity(n);
    while let Some(node) = ready.pop_front() {
        order.push(node);
        for &next in &successors[node] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                ready.push_back(next);
            }
        }
    }

    if order.len() == n {
        Ok(order)
    } else {
        Err((0..n).filter(|&i| indegree[i] > 0).collect())
    }
}

/// Reconstructs an actual cycle as a node sequence, for an error message.
///
/// Given the `leftover` nodes [`topo_sort`] could not order, follows
/// outgoing edges from the first leftover until it revisits a node, then
/// returns that loop closed back on itself (`a → b → c → a`). Best-effort:
/// it finds *one* cycle, which is all a diagnostic needs.
pub(crate) fn cycle_path(leftover: &[usize], edges: &[(usize, usize)]) -> Vec<usize> {
    let start = leftover[0];
    let mut path = vec![start];
    let mut current = start;
    loop {
        // Step to the next leftover node along an edge out of `current`.
        let next = edges
            .iter()
            .find(|&&(before, after)| before == current && leftover.contains(&after))
            .map(|&(_, after)| after);
        let Some(next) = next else { break };
        if let Some(pos) = path.iter().position(|&n| n == next) {
            // Closed the loop — return from where it re-entered.
            let mut cycle = path[pos..].to_vec();
            cycle.push(next);
            return cycle;
        }
        path.push(next);
        current = next;
    }
    path
}

/// Partitions a workload's systems into access-disjoint batches,
/// honouring its explicit ordering edges.
///
/// # Logic
///
/// One pass over a single ordering:
///
/// 1. **Linearise** the explicit `(before, after)` edges with
///    [`topo_sort`]. This is the *only* graph a cycle can come from —
///    `.any_order_with` adds no edges — so it is the one honest place to
///    report "you declared a cyclic order".
/// 2. **Rank** each system in that topological order. A system sits one
///    batch later than the latest of: its explicit predecessors, and any
///    *conflicting* system that comes earlier in the order. Walking in
///    topo order guarantees every such predecessor is already ranked, so a
///    single forward sweep computes the longest-path depth.
///
/// ```text
/// systems:      S0(write A)  S1(write B)  S2(read A, .after S0)
/// explicit:     S0 → S2
/// topo order:   S0, S1, S2
/// rank:  S0 → 0           (nothing earlier blocks it)
///        S1 → 0           (B is disjoint from A; no edge)
///        S2 → 1           (explicit pred S0, and reads A that S0 writes)
/// batches:      [ [S0, S1], [S2] ]
/// ```
///
/// # Why it works
///
/// Two systems share a batch only at equal rank. If they conflicted, the
/// one later in topo order would have counted the earlier as a blocking
/// predecessor and taken `rank + 1` — so equal rank *proves* disjoint
/// access, exactly the invariant the M4 parallel executor needs to hand a
/// batch to Rayon. Explicit edges are respected because topo order places
/// every predecessor first, and the rank sweep lifts each system past
/// them. `.any_order_with` adds no edge and plays no part here — it only
/// silences the conflict-policy error in [`validate_system_conflicts`]. An
/// acknowledged pair therefore reaches this point unordered, so topo order
/// places them by registration index; the access-conflict arm of the rank
/// sweep then separates them into different batches, exactly as it would
/// any conflicting pair.
///
/// # Panics
///
/// On a cycle in the explicit edges, with the system-ordering cycle
/// message (the cycle is genuinely user-declared — conflicts add no edges).
pub(crate) fn build_batches(systems: &[System], edges: &[(usize, usize)]) -> Vec<Vec<SystemId>> {
    let n = systems.len();

    // Linearise the user-declared order. A cycle here is a real
    // contradiction in the `.after`/`.before` declarations.
    let order = topo_sort(n, edges).unwrap_or_else(|leftover| {
        let path = cycle_path(&leftover, edges);
        let names: Vec<&str> = path.iter().map(|&i| systems[i].name).collect();
        panic!("{}", cycle_message("system", &names));
    });

    // Each system's *direct* explicit predecessors — the reverse of the
    // forward edges `topo_sort` walked. "Direct" (not transitive): the rank
    // sweep only asks "is `p` immediately required before `s`?"; transitive
    // ordering falls out of the sweep itself.
    let mut direct_preds = vec![Vec::new(); n];
    for &(before, after) in edges {
        direct_preds[after].push(before);
    }

    // Longest-path rank in topo order: each system lands one batch after
    // the latest system that must precede it — an explicit predecessor, or
    // a conflicting system positioned earlier. Every such predecessor is
    // already ranked because we walk in topo order.
    let mut rank = vec![0usize; n];
    for (position, &s) in order.iter().enumerate() {
        for &p in &order[..position] {
            let blocks = direct_preds[s].contains(&p)
                || !systems[p].access.compatible_with(&systems[s].access);
            if blocks {
                rank[s] = rank[s].max(rank[p] + 1);
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
    // directly in debug/test builds rather than trusting the argument.
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

/// Panics if any access-conflicting pair of systems in `data` has neither
/// a declared order (a transitive `.after`/`.before` path) nor an
/// `.any_order_with` acknowledgement — conflict policy (a) at the system
/// level.
///
/// Runs at schedule-build time, where every system and edge is known.
/// `Commands` declares no access, so command-only systems never trip this
/// — only real component/resource overlaps do.
///
/// # Panics
///
/// With the system-conflict message, naming both systems and the clashing
/// type.
pub(crate) fn validate_system_conflicts(data: &WorkloadData) {
    let n = data.systems.len();
    // Build the explicit-edge adjacency once, then reuse it for every
    // pair's reachability test rather than rebuilding it per pair.
    let successors = successors_of(n, &data.edges);
    for i in 0..n {
        for j in (i + 1)..n {
            // Gate on the cheap boolean predicate: compatible pairs need
            // nothing declared, so most pairs short-circuit here with no work.
            if data.systems[i]
                .access
                .compatible_with(&data.systems[j].access)
            {
                continue;
            }
            // They conflict — either an order is declared (transitively) or
            // it is acknowledged, or it is a registration error.
            if has_declared_order(i, j, &successors) || data.any_order.contains(&canonical(i, j)) {
                continue;
            }
            // The error path, and the only place the named conflict detail
            // (and its allocation) is needed.
            let (type_id, kind) = first_conflict(&data.systems[i].access, &data.systems[j].access);
            panic!("{}", system_conflict_message(data, i, j, type_id, kind));
        }
    }
}

/// The first `(TypeId, ConflictKind)` between two incompatible accesses —
/// the conflict detail a diagnostic names. A thin wrapper over
/// [`Access::find_conflicts`] used only on an error path, so its allocation
/// never touches the common case. Shared by the system- and
/// workload-level validators.
///
/// # Panics
///
/// Panics if the two accesses are actually compatible — callers gate on
/// [`Access::compatible_with`] first, so reaching here with no conflict is
/// a logic error, not a user error.
pub(crate) fn first_conflict(a: &Access, b: &Access) -> (TypeId, ConflictKind) {
    a.find_conflicts(b)
        .into_iter()
        .next()
        .expect("incompatible accesses always have at least one conflict")
}

/// Builds the system-level conflict message, pinning the `.after(handle)` /
/// `.any_order_with(handle)` advice.
fn system_conflict_message(
    data: &WorkloadData,
    i: usize,
    j: usize,
    type_id: TypeId,
    kind: ConflictKind,
) -> String {
    let a = data.systems[i].name;
    let b = data.systems[j].name;
    let (type_name, domain) = data.systems[i]
        .access
        .describe(type_id)
        .or_else(|| data.systems[j].access.describe(type_id))
        .unwrap_or(("<unknown>", "value"));
    let clash = match kind {
        ConflictKind::WriteWrite => format!("both write {domain} `{type_name}`"),
        ConflictKind::WriteRead => {
            format!("conflict on {domain} `{type_name}` (one writes what the other reads)")
        }
    };
    format!(
        "Systems `{a}` and `{b}` {clash} but no order is declared. \
         Add .after(handle) to one, or .any_order_with(handle) if order \
         is intentionally undefined."
    )
}

/// Builds the workload-level conflict message — pinned in #34.
pub(crate) fn workload_conflict_message(
    a: &str,
    b: &str,
    type_name: &str,
    kind: ConflictKind,
) -> String {
    let kind_str = match kind {
        ConflictKind::WriteWrite => "write/write",
        ConflictKind::WriteRead => "write/read",
    };
    format!(
        "Workloads `{a}` and `{b}` conflict on {kind_str} of `{type_name}` but no order \
         is declared between them. Add .after(WorkloadLabel) or .any_order_with(WorkloadLabel)."
    )
}

/// Builds the cycle message — pinned in #34. `level` is `"workload"` or
/// `"system"`; `path` is the node names closed back on the first.
pub(crate) fn cycle_message(level: &str, path: &[&str]) -> String {
    format!(
        "Cycle detected in {level} ordering: {}. Break the cycle by removing one of \
         the .after declarations.",
        path.join(" → ")
    )
}

/// Builds the unknown-label message — a workload was ordered against a
/// label no `add_workload` in this schedule registered. Surfaces at build,
/// because labels resolve lazily (forward references are allowed).
pub(crate) fn unknown_label_message(name: &str) -> String {
    format!(
        "Unknown workload label `{name}` referenced by .after/.before — no workload \
         with that label was registered in this schedule."
    )
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
    #[derive(Component)]
    struct Position {
        x: f32,
    }
    #[derive(Component)]
    struct Velocity {
        x: f32,
    }

    /// Boxes a system fn the same way registration does — lets the
    /// batcher's pure-layering tests build inputs directly, without a
    /// `Schedule` (and so without the conflict policy `Schedule` enforces).
    fn boxed<S, Marker>(system: S) -> System
    where
        S: IntoSystem<Marker>,
    {
        System::from_system(system)
    }

    // ── build_batches: pure access-conflict layering ───────────────────

    #[test]
    fn conflicting_systems_split_into_separate_batches() {
        fn writer(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn reader(s: Res<Score>) {
            let _ = s.value;
        }
        let batches = build_batches(&[boxed(writer), boxed(reader)], &[]);
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
        let batches = build_batches(
            &[boxed(write_score), boxed(read_score), boxed(read_frame)],
            &[],
        );
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![SystemId(0), SystemId(2)]); // S2 joins S0's batch
        assert_eq!(batches[1], vec![SystemId(1)]);
    }

    #[test]
    fn component_query_conflicts_feed_batching() {
        fn move_pos(mut q: Query<&mut Position>) {
            for mut p in q.iter_mut() {
                p.x += 1.0;
            }
        }
        fn read_pos(q: Query<&Position>) {
            for p in q.iter() {
                let _ = p.x;
            }
        }
        assert_eq!(
            build_batches(&[boxed(move_pos), boxed(read_pos)], &[]).len(),
            2
        );
    }

    #[test]
    fn query_over_disjoint_components_shares_a_batch() {
        fn move_pos(mut q: Query<&mut Position>) {
            for mut p in q.iter_mut() {
                p.x += 1.0;
            }
        }
        fn move_vel(mut q: Query<&mut Velocity>) {
            for mut v in q.iter_mut() {
                v.x += 1.0;
            }
        }
        assert_eq!(
            build_batches(&[boxed(move_pos), boxed(move_vel)], &[]).len(),
            1
        );
    }

    #[test]
    fn access_aggregates_across_multiple_params() {
        // `mover`'s access is the union of all four params. If aggregation
        // folded only the first — or Commands wrongly cleared the rest —
        // the conflicts below would vanish and the batch layout change.
        fn mover(_f: Res<Frame>, mut s: ResMut<Score>, mut q: Query<&mut Position>, _c: Commands) {
            s.value += 1;
            for mut p in q.iter_mut() {
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
            for mut v in q.iter_mut() {
                v.x += 1.0;
            }
        }
        let batches = build_batches(
            &[
                boxed(mover),
                boxed(reads_pos),
                boxed(reads_score),
                boxed(moves_vel),
            ],
            &[],
        );
        assert_eq!(batches.len(), 2);
        // mover writes Position AND Score → conflicts both readers; moves_vel
        // is disjoint and packs into batch 0.
        assert_eq!(batches[0], vec![SystemId(0), SystemId(3)]);
        assert_eq!(batches[1], vec![SystemId(1), SystemId(2)]);
    }

    #[test]
    fn filter_read_access_feeds_batching() {
        // `With<Velocity>` makes `filtered` read Velocity even though it
        // yields only `&Position`, so it conflicts with a Velocity writer.
        fn writes_vel(mut q: Query<&mut Velocity>) {
            for mut v in q.iter_mut() {
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
        let batches = build_batches(&[boxed(writes_vel), boxed(filtered), boxed(plain_pos)], &[]);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![SystemId(0), SystemId(2)]);
        assert_eq!(batches[1], vec![SystemId(1)]);
    }

    #[test]
    fn conflict_chain_layers_into_three_batches() {
        // Transitive chain forces ranks 0 → 1 → 2.
        fn write_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn score_to_frame(s: Res<Score>, mut f: ResMut<Frame>) {
            f.n += s.value;
        }
        fn read_frame(f: Res<Frame>) {
            let _ = f.n;
        }
        let batches = build_batches(
            &[boxed(write_score), boxed(score_to_frame), boxed(read_frame)],
            &[],
        );
        assert_eq!(batches.len(), 3);
        assert_eq!(batches[0], vec![SystemId(0)]);
        assert_eq!(batches[1], vec![SystemId(1)]);
        assert_eq!(batches[2], vec![SystemId(2)]);
    }

    // ── build_batches: explicit ordering edges ──────────────────────────

    #[test]
    fn explicit_edge_orders_compatible_systems() {
        // Two disjoint systems would share a batch — an explicit edge
        // forces the second into a later batch.
        fn touch_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn touch_frame(mut f: ResMut<Frame>) {
            f.n += 1;
        }
        let unordered = build_batches(&[boxed(touch_score), boxed(touch_frame)], &[]);
        assert_eq!(unordered.len(), 1);
        let ordered = build_batches(&[boxed(touch_score), boxed(touch_frame)], &[(0, 1)]);
        assert_eq!(ordered.len(), 2);
        assert_eq!(ordered[0], vec![SystemId(0)]);
        assert_eq!(ordered[1], vec![SystemId(1)]);
    }

    #[test]
    fn explicit_backward_edge_is_honoured() {
        // Edge (1, 0): system 1 must run before system 0, despite the
        // registration order — the whole point of handle-based ordering.
        fn touch_score(mut s: ResMut<Score>) {
            s.value += 1;
        }
        fn touch_frame(mut f: ResMut<Frame>) {
            f.n += 1;
        }
        let batches = build_batches(&[boxed(touch_score), boxed(touch_frame)], &[(1, 0)]);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0], vec![SystemId(1)]);
        assert_eq!(batches[1], vec![SystemId(0)]);
    }

    // ── graph primitives ────────────────────────────────────────────────

    #[test]
    fn topo_sort_orders_a_chain() {
        // 0 → 1 → 2
        let order = topo_sort(3, &[(0, 1), (1, 2)]).expect("acyclic");
        assert_eq!(order, vec![0, 1, 2]);
    }

    #[test]
    fn topo_sort_breaks_ties_by_index() {
        // No edges → registration (index) order.
        assert_eq!(topo_sort(3, &[]).expect("acyclic"), vec![0, 1, 2]);
    }

    #[test]
    fn topo_sort_honours_backward_edges() {
        // 2 must come before 0 even though it was "registered" later.
        let order = topo_sort(3, &[(2, 0)]).expect("acyclic");
        let pos = |x| order.iter().position(|&n| n == x).unwrap();
        assert!(pos(2) < pos(0));
    }

    #[test]
    fn topo_sort_reports_a_cycle() {
        let leftover = topo_sort(3, &[(0, 1), (1, 2), (2, 0)]).expect_err("cyclic");
        assert_eq!(leftover.len(), 3);
    }

    #[test]
    fn reachable_follows_transitive_paths() {
        // 0 → 1 → 2
        let successors = successors_of(3, &[(0, 1), (1, 2)]);
        assert!(reachable_in(0, 2, &successors));
        assert!(!reachable_in(2, 0, &successors));
    }

    #[test]
    fn cycle_path_closes_the_loop() {
        let path = cycle_path(&[0, 1, 2], &[(0, 1), (1, 2), (2, 0)]);
        assert_eq!(path.first(), path.last());
        assert!(path.len() >= 4); // a → b → c → a
    }

    #[test]
    fn canonical_is_order_independent() {
        assert_eq!(canonical(3, 1), canonical(1, 3));
        assert_eq!(canonical(1, 3), (1, 3));
    }
}
