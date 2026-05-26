//! # Lesson 2 — when one thing changes, update another
//!
//! Lesson 1 watched a component and counted it. The real payoff is using a
//! change as a *trigger*: "for every plant whose fuel dropped, write a
//! refuel order." The query reads or filters one component and writes a
//! **different** one.
//!
//! That "different" matters. A `Changed<T>` filter secretly counts as
//! *reading* `T`, so `Query<&mut T, Changed<T>>` would be asking to read and
//! write the same `T` at once — the engine rejects that at startup. The fix
//! is exactly the natural design: watch one component, write a sibling. Every
//! example here does that.
//!
//! The other thing this lesson shows is that the `Changed<T>` filter clips
//! onto *any* query shape:
//!
//! - `refuel-dispatch` — write a single `&mut` component.
//! - `grid-solver` — read three components at once (an arity-3 join).
//! - `energy-toll` / `service-schedule` — read one, write another (the
//!   written one can be first or second in the tuple).
//! - `substation-heat` — write *two* components at once (a multi-mut join),
//!   still only for the entities the filter let through.
//! - `segment-loss` / `new-segment` — a two-component read, watched with
//!   `Changed` and with `Added`.
//!
//! Components live in [`super::components`]; counts are checked by
//! [`super::scoreboard`].

use spark_core::{Application, Stage};
use spark_ecs::{Added, Changed, Commands, Query, Res, ResMut, With};

use super::components::{
    BusVoltage, CableTemp, CoilTemp, Endpoint, Energised, EnergySold, Feeder, FuelLevel,
    LoadSignal, RefuelOrder, SegmentLoad, ServiceCountdown, Throughput, Transformer, WearLevel,
};
use super::scoreboard::{Frame, Scoreboard, record};

// ── refuel-dispatch — watch `FuelLevel`, write `RefuelOrder` ─────────────

/// Seeds two fuelled plants.
fn seed_fuelled_plants(mut commands: Commands) {
    commands
        .spawn()
        .insert(FuelLevel(80))
        .insert(RefuelOrder(0));
    commands
        .spawn()
        .insert(FuelLevel(60))
        .insert(RefuelOrder(0));
}

/// Combustion burns fuel down every tick.
fn burn_fuel(mut q: Query<&mut FuelLevel>) {
    for mut fuel in q.iter_mut() {
        fuel.0 = fuel.0.wrapping_sub(1);
    }
}

/// `Query<&mut RefuelOrder, Changed<FuelLevel>>` — the dispatcher bumps the
/// refuel order of every plant whose fuel moved. It **writes** `RefuelOrder`
/// and **watches** `FuelLevel` — two different components, so the
/// read-and-write-the-same-thing trap is avoided. Both plants burn fuel
/// every tick, so both get an order. Expected: 2, always.
fn dispatch_refuel_orders(
    mut board: ResMut<Scoreboard>,
    mut q: Query<&mut RefuelOrder, Changed<FuelLevel>>,
) {
    let mut n = 0;
    for mut order in q.iter_mut() {
        order.0 = order.0.wrapping_add(1);
        n += 1;
    }
    record(
        &mut board,
        "refuel-dispatch",
        "Query<&mut RefuelOrder, Changed<FuelLevel>>",
        n,
        2,
    );
}

// ── grid-solver — `Changed` on an arity-3 read join ──────────────────────

/// Seeds two complete substations (bus + transformer + feeder) plus one bare
/// bus the solver must skip even though its voltage moves.
fn seed_grid_nodes(mut commands: Commands) {
    commands
        .spawn()
        .insert(BusVoltage(230))
        .insert(Transformer)
        .insert(Feeder);
    commands
        .spawn()
        .insert(BusVoltage(110))
        .insert(Transformer)
        .insert(Feeder);
    commands.spawn().insert(BusVoltage(33)); // bare bus — no transformer/feeder
}

/// Bus voltages drift every tick.
fn fluctuate_bus_voltage(mut q: Query<&mut BusVoltage>) {
    for mut v in q.iter_mut() {
        v.0 = v.0.wrapping_add(1);
    }
}

/// `Query<(&BusVoltage, &Transformer, &Feeder), Changed<BusVoltage>>` — the
/// grid solver re-solves complete substations whose voltage moved. The
/// change filter rides a three-component read: all three buses change, but
/// only the two that *also* have a transformer and a feeder match the join;
/// the bare bus is dropped before the filter even matters. Expected: 2,
/// always.
#[allow(
    clippy::type_complexity,
    reason = "the arity-3 join plus a change filter is the combination under test"
)]
fn grid_solver_resolves_substations(
    mut board: ResMut<Scoreboard>,
    q: Query<(&BusVoltage, &Transformer, &Feeder), Changed<BusVoltage>>,
) {
    record(
        &mut board,
        "grid-solver",
        "Query<(&BusVoltage, &Transformer, &Feeder), Changed<BusVoltage>>",
        q.iter().count(),
        2,
    );
}

// ── energy-toll — read one, write another (written component second) ─────

/// Seeds two metering stations.
fn seed_metering_stations(mut commands: Commands) {
    commands
        .spawn()
        .insert(Throughput(12))
        .insert(EnergySold(0));
    commands
        .spawn()
        .insert(Throughput(20))
        .insert(EnergySold(0));
}

/// Throughput is re-read every tick.
fn update_throughput(mut q: Query<&mut Throughput>) {
    for mut t in q.iter_mut() {
        t.0 = t.0.wrapping_add(1);
    }
}

/// `Query<(&Throughput, &mut EnergySold), Changed<Throughput>>` — the toll
/// run bills each station whose throughput moved, by adding the flow onto its
/// running total. The driver reads `Throughput` (the filter reads it too —
/// two reads are fine) and writes the second element, `EnergySold`. Both
/// stations move every tick. Expected: 2, always.
fn accrue_energy_sold(
    mut board: ResMut<Scoreboard>,
    mut q: Query<(&Throughput, &mut EnergySold), Changed<Throughput>>,
) {
    let mut n = 0;
    for (flow, mut sold) in q.iter_mut() {
        sold.0 = sold.0.wrapping_add(flow.0);
        n += 1;
    }
    record(
        &mut board,
        "energy-toll",
        "Query<(&Throughput, &mut EnergySold), Changed<Throughput>>",
        n,
        2,
    );
}

// ── service-schedule — read one, write another (written component first) ─

/// Seeds two operating plants.
fn seed_operating_plants(mut commands: Commands) {
    commands
        .spawn()
        .insert(ServiceCountdown(100))
        .insert(WearLevel(0));
    commands
        .spawn()
        .insert(ServiceCountdown(100))
        .insert(WearLevel(0));
}

/// Running a plant accumulates wear every tick.
fn accumulate_wear(mut q: Query<&mut WearLevel>) {
    for mut wear in q.iter_mut() {
        wear.0 = wear.0.wrapping_add(1);
    }
}

/// `Query<(&mut ServiceCountdown, &WearLevel), Changed<WearLevel>>` — the
/// scheduler counts down the service timer of each plant whose wear moved.
/// Here the *written* component comes first and the *watched* one second —
/// the filter doesn't care about tuple order, only that it watches a
/// component the body doesn't also write. Wear rises on both plants every
/// tick. Expected: 2, always.
fn schedule_service(
    mut board: ResMut<Scoreboard>,
    mut q: Query<(&mut ServiceCountdown, &WearLevel), Changed<WearLevel>>,
) {
    let mut n = 0;
    for (mut countdown, _wear) in q.iter_mut() {
        countdown.0 = countdown.0.wrapping_sub(1);
        n += 1;
    }
    record(
        &mut board,
        "service-schedule",
        "Query<(&mut ServiceCountdown, &WearLevel), Changed<WearLevel>>",
        n,
        2,
    );
}

// ── substation-heat — write TWO components, gated by a third's change ────

/// Seeds three substations (`CableTemp` + `CoilTemp` + `LoadSignal`); two are
/// energised, one is idle.
fn seed_substations(mut commands: Commands) {
    commands
        .spawn()
        .insert(CableTemp(20))
        .insert(CoilTemp(20))
        .insert(LoadSignal(1))
        .insert(Energised);
    commands
        .spawn()
        .insert(CableTemp(20))
        .insert(CoilTemp(20))
        .insert(LoadSignal(2))
        .insert(Energised);
    commands
        .spawn()
        .insert(CableTemp(20))
        .insert(CoilTemp(20))
        .insert(LoadSignal(3)); // idle
}

/// Re-sends the load signal on energised substations each tick.
fn pulse_load_signals(mut q: Query<&mut LoadSignal, With<Energised>>) {
    for mut sig in q.iter_mut() {
        sig.0 = sig.0.wrapping_add(1);
    }
}

/// `Query<(&mut CableTemp, &mut CoilTemp), Changed<LoadSignal>>` — the
/// thermal model recomputes **both** temperatures for substations whose load
/// signal moved. Writing two components at once is the "multi-mut" shape, and
/// the change filter clips onto it unchanged. Frame 1 reacts to all three
/// (first look at the seeded signals); from frame 2 only the two energised
/// substations keep changing, and the idle one is never recomputed — the
/// filter trims the work even on a two-write join. Expected:
/// `{1 → 3, else → 2}`.
fn recompute_substation_heat(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    mut q: Query<(&mut CableTemp, &mut CoilTemp), Changed<LoadSignal>>,
) {
    let mut n = 0;
    for (mut cable, mut coil) in q.iter_mut() {
        cable.0 = cable.0.wrapping_add(1);
        coil.0 = coil.0.wrapping_add(1);
        n += 1;
    }
    let expected = if frame.0 == 1 { 3 } else { 2 };
    record(
        &mut board,
        "substation-heat",
        "Query<(&mut CableTemp, &mut CoilTemp), Changed<LoadSignal>>",
        n,
        expected,
    );
}

// ── segment-loss / new-segment — one read join, watched two ways ─────────

/// Seeds two endpoint segments plus one mid-span segment (no endpoint) the
/// join must skip.
fn seed_transmission_segments(mut commands: Commands) {
    commands.spawn().insert(SegmentLoad(30)).insert(Endpoint);
    commands.spawn().insert(SegmentLoad(45)).insert(Endpoint);
    commands.spawn().insert(SegmentLoad(60)); // mid-span — no endpoint
}

/// Segment loads update every tick.
fn update_segment_loads(mut q: Query<&mut SegmentLoad>) {
    for mut load in q.iter_mut() {
        load.0 = load.0.wrapping_add(1);
    }
}

/// `Query<(&SegmentLoad, &Endpoint), Changed<SegmentLoad>>` — the loss
/// calculator recomputes endpoint segments whose load moved. Loads change
/// every tick; only the two with an `Endpoint` are in the join. Expected: 2,
/// always.
fn recompute_segment_loss(
    mut board: ResMut<Scoreboard>,
    q: Query<(&SegmentLoad, &Endpoint), Changed<SegmentLoad>>,
) {
    record(
        &mut board,
        "segment-loss",
        "Query<(&SegmentLoad, &Endpoint), Changed<SegmentLoad>>",
        q.iter().count(),
        2,
    );
}

/// `Query<(&SegmentLoad, &Endpoint), Added<Endpoint>>` — the registrar logs
/// newly-laid endpoint segments. It watches the *same* `Endpoint` the join
/// already reads (sharing a read is allowed). `Endpoint` is attached once, so
/// this is a one-shot: the two endpoint segments on frame 1, then none — even
/// though their load keeps moving. Expected: `{1 → 2, else → 0}`.
fn register_new_segments(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<(&SegmentLoad, &Endpoint), Added<Endpoint>>,
) {
    let expected = if frame.0 == 1 { 2 } else { 0 };
    record(
        &mut board,
        "new-segment",
        "Query<(&SegmentLoad, &Endpoint), Added<Endpoint>>",
        q.iter().count(),
        expected,
    );
}

/// Registers Lesson 2, each writer ahead of the observer that reacts to it.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed_fuelled_plants)
        .add_system(Stage::Update, burn_fuel)
        .add_system(Stage::Update, dispatch_refuel_orders);

    app.add_system(Stage::Startup, seed_grid_nodes)
        .add_system(Stage::Update, fluctuate_bus_voltage)
        .add_system(Stage::Update, grid_solver_resolves_substations);

    app.add_system(Stage::Startup, seed_metering_stations)
        .add_system(Stage::Update, update_throughput)
        .add_system(Stage::Update, accrue_energy_sold);

    app.add_system(Stage::Startup, seed_operating_plants)
        .add_system(Stage::Update, accumulate_wear)
        .add_system(Stage::Update, schedule_service);

    app.add_system(Stage::Startup, seed_substations)
        .add_system(Stage::Update, pulse_load_signals)
        .add_system(Stage::Update, recompute_substation_heat);

    app.add_system(Stage::Startup, seed_transmission_segments)
        .add_system(Stage::Update, update_segment_loads)
        .add_system(Stage::Update, recompute_segment_loss)
        .add_system(Stage::Update, register_new_segments);
}
