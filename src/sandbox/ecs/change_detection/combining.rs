//! # Lesson 3 — react only when the *right combination* changed
//!
//! Real control systems rarely react to a single bare change. They want
//! "cities that changed **and** are on the grid", or "sites where solar
//! **or** wind moved", or "reactors where fuel **and** coolant both moved".
//! A `Changed<T>` filter is an ordinary filter, so it drops straight into
//! the same combinators you'd use with `With` / `Without`:
//!
//! - `And<(A, B)>` — every part must match.
//! - `Or<(A, B)>` — any part matches.
//! - `With<M>` / `Without<M>` — entity has / lacks a flag component.
//!
//! …and they nest and stretch:
//!
//! - `billing` / `offgrid-alert` — `Changed` combined via `And` with `With`
//!   and with `Without` (the two split every changed city into on-grid vs
//!   off-grid).
//! - `hybrid-output` — `Or` of two change sources.
//! - `safety-interlock` — `And` of two change sources.
//! - `ops-dashboard` — a nested `And<(With<…>, Or<(Changed<…>, Added<…>)>)>`.
//! - `full-flow` / `any-activity` — three-way `And` and `Or` (the
//!   combinators take 2-, 3-, and 4-part tuples).
//!
//! Naming a long filter with a `type` alias (below) keeps the query
//! readable — and reads like the sentence it stands for. Components live in
//! [`super::components`]; counts are checked by [`super::scoreboard`].

use spark_core::{Application, Stage};
use spark_ecs::{Added, And, Changed, Commands, Or, Query, Res, ResMut, With, Without};

use super::components::{
    CityDemand, Commissioned, Connected, Coolant, CoolantCirculating, FuelRods, Generator,
    HybridSite, NuclearPlant, Online, Penstock, PenstockOpen, Reporting, RodsCycling, SolarYield,
    SunExposed, Tailrace, TailraceDraining, Telemetry, Turbine, TurbineSpinning, WindExposed,
    WindYield,
};
use super::scoreboard::{Frame, Scoreboard, record};

// ── billing / offgrid-alert — `Changed` AND a `With` / `Without` flag ────

/// Grid-connected cities whose demand moved this tick.
type ConnectedDemandChanged = And<(With<Connected>, Changed<CityDemand>)>;
/// Off-grid cities whose demand moved — the exact complement.
type OffgridDemandChanged = And<(Without<Connected>, Changed<CityDemand>)>;

/// Seeds three cities; two are grid-connected, one is off-grid.
fn seed_cities(mut commands: Commands) {
    commands.spawn().insert(CityDemand(50)).insert(Connected);
    commands.spawn().insert(CityDemand(80)).insert(Connected);
    commands.spawn().insert(CityDemand(30)); // off-grid hamlet
}

/// Every city's demand shifts each tick (a day/night load curve).
fn update_city_demand(mut q: Query<&mut CityDemand>) {
    for mut d in q.iter_mut() {
        d.0 = d.0.wrapping_add(1);
    }
}

/// `And<(With<Connected>, Changed<CityDemand>)>` — the billing run charges
/// connected cities whose demand moved. Demand moves on all three, so the
/// `With<Connected>` half does the splitting: the two connected cities.
/// Expected: 2, always.
fn bill_connected_cities(
    mut board: ResMut<Scoreboard>,
    q: Query<&CityDemand, ConnectedDemandChanged>,
) {
    record(
        &mut board,
        "billing",
        "Query<&CityDemand, And<(With<Connected>, Changed<CityDemand>)>>",
        q.iter().count(),
        2,
    );
}

/// `And<(Without<Connected>, Changed<CityDemand>)>` — the brownout monitor
/// flags off-grid cities whose demand moved: the lone off-grid city. Between
/// them the two checks partition every changed city (2 + 1 = 3). Expected:
/// 1, always.
fn alert_offgrid_cities(
    mut board: ResMut<Scoreboard>,
    q: Query<&CityDemand, OffgridDemandChanged>,
) {
    record(
        &mut board,
        "offgrid-alert",
        "Query<&CityDemand, And<(Without<Connected>, Changed<CityDemand>)>>",
        q.iter().count(),
        1,
    );
}

// ── hybrid-output — `Or` of two change sources ───────────────────────────

/// A site whose solar **or** wind yield moved since last tick.
type SolarOrWindMoved = Or<(Changed<SolarYield>, Changed<WindYield>)>;

/// Seeds three hybrid sites (all carry both yields): one sun-exposed, one in
/// shade and still air, one wind-exposed.
fn seed_hybrid_sites(mut commands: Commands) {
    commands
        .spawn()
        .insert(HybridSite)
        .insert(SolarYield(10))
        .insert(WindYield(10))
        .insert(SunExposed);
    commands
        .spawn()
        .insert(HybridSite)
        .insert(SolarYield(20))
        .insert(WindYield(20)); // exposed to neither
    commands
        .spawn()
        .insert(HybridSite)
        .insert(SolarYield(30))
        .insert(WindYield(30))
        .insert(WindExposed);
}

/// Solar yield updates only on sun-exposed sites.
fn update_solar_yield(mut q: Query<&mut SolarYield, With<SunExposed>>) {
    for mut y in q.iter_mut() {
        y.0 = y.0.wrapping_add(1);
    }
}

/// Wind yield updates only on wind-exposed sites.
fn update_wind_yield(mut q: Query<&mut WindYield, With<WindExposed>>) {
    for mut y in q.iter_mut() {
        y.0 = y.0.wrapping_add(1);
    }
}

/// `Query<&HybridSite, Or<(Changed<SolarYield>, Changed<WindYield>)>>` — the
/// dispatch recomputes a site's net output if *either* source moved. Frame 1
/// sees all three (first look at the seeded yields); from frame 2 only the
/// sun-exposed and wind-exposed sites keep moving, so the becalmed, shaded
/// site drops out. Expected: `{1 → 3, else → 2}`.
fn recompute_hybrid_output(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&HybridSite, SolarOrWindMoved>,
) {
    let expected = if frame.0 == 1 { 3 } else { 2 };
    record(
        &mut board,
        "hybrid-output",
        "Query<&HybridSite, Or<(Changed<SolarYield>, Changed<WindYield>)>>",
        q.iter().count(),
        expected,
    );
}

// ── safety-interlock — `And` of two change sources ───────────────────────

/// A plant whose fuel rods **and** coolant both moved since last tick.
type FuelAndCoolantMoved = And<(Changed<FuelRods>, Changed<Coolant>)>;

/// Seeds three reactors (all carry both readings): one only cycling rods,
/// one doing both, one only circulating coolant.
fn seed_reactors(mut commands: Commands) {
    commands
        .spawn()
        .insert(NuclearPlant)
        .insert(FuelRods(1))
        .insert(Coolant(1))
        .insert(RodsCycling);
    commands
        .spawn()
        .insert(NuclearPlant)
        .insert(FuelRods(2))
        .insert(Coolant(2))
        .insert(RodsCycling)
        .insert(CoolantCirculating);
    commands
        .spawn()
        .insert(NuclearPlant)
        .insert(FuelRods(3))
        .insert(Coolant(3))
        .insert(CoolantCirculating);
}

/// Steps fuel rods on plants whose rods are cycling.
fn cycle_fuel_rods(mut q: Query<&mut FuelRods, With<RodsCycling>>) {
    for mut r in q.iter_mut() {
        r.0 = r.0.wrapping_add(1);
    }
}

/// Steps coolant flow on plants whose loop is circulating.
fn circulate_coolant(mut q: Query<&mut Coolant, With<CoolantCirculating>>) {
    for mut c in q.iter_mut() {
        c.0 = c.0.wrapping_add(1);
    }
}

/// `Query<&NuclearPlant, And<(Changed<FuelRods>, Changed<Coolant>)>>` — the
/// safety interlock re-runs only when *both* the rods and the coolant moved
/// this tick. The mirror image of `hybrid-output`: swap `Or` for `And` and
/// "either moved" becomes "both moved". Frame 1 sees all three (both seeded
/// readings are fresh); from frame 2 only the plant doing both still
/// satisfies the `And`. Expected: `{1 → 3, else → 1}`.
fn run_safety_interlock(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&NuclearPlant, FuelAndCoolantMoved>,
) {
    let expected = if frame.0 == 1 { 3 } else { 1 };
    record(
        &mut board,
        "safety-interlock",
        "Query<&NuclearPlant, And<(Changed<FuelRods>, Changed<Coolant>)>>",
        q.iter().count(),
        expected,
    );
}

// ── ops-dashboard — a nested combination ─────────────────────────────────

/// Online generators that either just commissioned **or** are streaming
/// fresh telemetry: `With` combined via `And` with an `Or` of a `Changed`
/// and an `Added`.
type OnlineAndLively = And<(With<Online>, Or<(Changed<Telemetry>, Added<Commissioned>)>)>;

/// Seeds four generators: two online + reporting, one online but silent, one
/// offline. All carry `Telemetry` and `Commissioned`.
fn seed_generators(mut commands: Commands) {
    commands
        .spawn()
        .insert(Generator)
        .insert(Telemetry(1))
        .insert(Commissioned)
        .insert(Online)
        .insert(Reporting);
    commands
        .spawn()
        .insert(Generator)
        .insert(Telemetry(2))
        .insert(Commissioned)
        .insert(Online)
        .insert(Reporting);
    commands
        .spawn()
        .insert(Generator)
        .insert(Telemetry(3))
        .insert(Commissioned)
        .insert(Online); // online, silent
    commands
        .spawn()
        .insert(Generator)
        .insert(Telemetry(4))
        .insert(Commissioned); // offline
}

/// Streams fresh telemetry from reporting generators.
fn stream_telemetry(mut q: Query<&mut Telemetry, With<Reporting>>) {
    for mut t in q.iter_mut() {
        t.0 = t.0.wrapping_add(1);
    }
}

/// `Query<&Generator, And<(With<Online>, Or<(Changed<Telemetry>, Added<Commissioned>)>)>>`.
/// The dashboard shows online generators that are "lively" — either just
/// commissioned, or streaming. Frame 1: all three online generators light up
/// (each just commissioned and/or has fresh telemetry); the offline one is
/// gated out by `With<Online>`. From frame 2 the one-shot `Added` half is
/// spent, so the silent online generator drops off, leaving the two still
/// reporting. This is the whole lesson in one query: flags, `Or`, `Changed`,
/// and `Added` nested together. Expected: `{1 → 3, else → 2}`.
fn update_ops_dashboard(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&Generator, OnlineAndLively>,
) {
    let expected = if frame.0 == 1 { 3 } else { 2 };
    record(
        &mut board,
        "ops-dashboard",
        "Query<&Generator, And<(With<Online>, Or<(Changed<Telemetry>, Added<Commissioned>)>)>>",
        q.iter().count(),
        expected,
    );
}

// ── full-flow / any-activity — three-part `And` and `Or` ─────────────────

/// All three of a dam's flows moved this tick (full generation).
type AllFlowsMoved = And<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>;
/// Any of a dam's flows moved this tick (some activity).
type AnyFlowMoved = Or<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>;

/// Seeds four hydro dams (all carry all three flows): only the second has
/// every flow active; the fourth is fully idle.
fn seed_hydro_dams(mut commands: Commands) {
    commands
        .spawn()
        .insert(Penstock(1))
        .insert(Turbine(1))
        .insert(Tailrace(1))
        .insert(PenstockOpen);
    commands
        .spawn()
        .insert(Penstock(2))
        .insert(Turbine(2))
        .insert(Tailrace(2))
        .insert(PenstockOpen)
        .insert(TurbineSpinning)
        .insert(TailraceDraining);
    commands
        .spawn()
        .insert(Penstock(3))
        .insert(Turbine(3))
        .insert(Tailrace(3))
        .insert(TurbineSpinning);
    commands
        .spawn()
        .insert(Penstock(4))
        .insert(Turbine(4))
        .insert(Tailrace(4)); // idle
}

/// Steps penstock pressure on dams whose penstock is open.
fn flow_penstock(mut q: Query<&mut Penstock, With<PenstockOpen>>) {
    for mut p in q.iter_mut() {
        p.0 = p.0.wrapping_add(1);
    }
}

/// Steps turbine RPM on dams whose turbine is spinning.
fn spin_turbine(mut q: Query<&mut Turbine, With<TurbineSpinning>>) {
    for mut t in q.iter_mut() {
        t.0 = t.0.wrapping_add(1);
    }
}

/// Steps tailrace flow on dams whose tailrace is draining.
fn drain_tailrace(mut q: Query<&mut Tailrace, With<TailraceDraining>>) {
    for mut t in q.iter_mut() {
        t.0 = t.0.wrapping_add(1);
    }
}

/// `And<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>` — the
/// three-part conjunction; the full-generation light needs *all three* flows
/// moving. Frame 1 sees all four (every flow is fresh); from frame 2 only the
/// fully-active second dam keeps all three moving. Expected: `{1 → 4, else → 1}`.
fn check_full_flow(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&Penstock, AllFlowsMoved>,
) {
    let expected = if frame.0 == 1 { 4 } else { 1 };
    record(
        &mut board,
        "full-flow",
        "Query<&Penstock, And<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>>",
        q.iter().count(),
        expected,
    );
}

/// `Or<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>` — the
/// three-part disjunction over the same dams; the activity indicator needs
/// *any one* flow moving. Frame 1 sees all four; from frame 2 the three
/// active dams light up and the idle fourth stays dark. Expected:
/// `{1 → 4, else → 3}`.
fn check_any_activity(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&Penstock, AnyFlowMoved>,
) {
    let expected = if frame.0 == 1 { 4 } else { 3 };
    record(
        &mut board,
        "any-activity",
        "Query<&Penstock, Or<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>>",
        q.iter().count(),
        expected,
    );
}

/// Registers Lesson 3, each writer ahead of its observer(s).
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed_cities)
        .add_system(Stage::Update, update_city_demand)
        .add_system(Stage::Update, bill_connected_cities)
        .add_system(Stage::Update, alert_offgrid_cities);

    app.add_system(Stage::Startup, seed_hybrid_sites)
        .add_system(Stage::Update, update_solar_yield)
        .add_system(Stage::Update, update_wind_yield)
        .add_system(Stage::Update, recompute_hybrid_output);

    app.add_system(Stage::Startup, seed_reactors)
        .add_system(Stage::Update, cycle_fuel_rods)
        .add_system(Stage::Update, circulate_coolant)
        .add_system(Stage::Update, run_safety_interlock);

    app.add_system(Stage::Startup, seed_generators)
        .add_system(Stage::Update, stream_telemetry)
        .add_system(Stage::Update, update_ops_dashboard);

    app.add_system(Stage::Startup, seed_hydro_dams)
        .add_system(Stage::Update, flow_penstock)
        .add_system(Stage::Update, spin_turbine)
        .add_system(Stage::Update, drain_tailrace)
        .add_system(Stage::Update, check_full_flow)
        .add_system(Stage::Update, check_any_activity);
}
