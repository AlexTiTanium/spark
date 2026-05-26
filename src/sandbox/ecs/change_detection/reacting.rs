//! # Lesson 1 — react only to what changed
//!
//! The big idea: instead of re-checking *every* entity every tick, a system
//! can ask the query "give me only the ones whose `T` was written **since I
//! last ran**" by adding a `Changed<T>` filter — or "only the ones that
//! **just got** a `T`" with `Added<T>`. That's change detection.
//!
//! Three things to take away, each shown below on the simplest possible
//! query (`&T`, one component):
//!
//! 1. **It's relative to the reader.** `line-telemetry` rewrites its data
//!    every tick, so `Changed` keeps firing forever. `survey-cache` writes
//!    its data once and never again, so `Changed` fires the first time the
//!    reader looks (it hadn't seen the starting value yet) and then goes
//!    quiet. Same filter, opposite behaviour — the difference is whether
//!    anyone keeps writing.
//! 2. **`Added` is a one-shot.** `plant-output` and `plant-commission` watch
//!    the *same* plants: the meter overwrites their output every tick, so
//!    `Changed` reacts every tick, but `Added` fires exactly once — when the
//!    plant first appears — and never again, no matter how much the value
//!    moves afterwards.
//! 3. **Marking is precise.** `battery-bank` only tops up cells below full.
//!    A cell it skips is *not* marked changed, so the "still charging" count
//!    falls 5 → 3 → 2 → 1 → 0 as the bank fills. Change detection tracks the
//!    writes you actually make, not every entity you looked at.
//!
//! Components used live in [`super::components`]; the per-frame counts are
//! checked by [`super::scoreboard`].

use spark_core::{Application, Stage};
use spark_ecs::{Added, Changed, Commands, Query, Res, ResMut};

use super::components::{BatteryCharge, LineLoad, Output, Surveyed};
use super::scoreboard::{Frame, Scoreboard, record};

// ── line-telemetry — `Changed<T>` keeps firing while something writes ────

/// Seeds three monitored transmission lines.
fn seed_transmission_lines(mut commands: Commands) {
    commands.spawn().insert(LineLoad(40));
    commands.spawn().insert(LineLoad(55));
    commands.spawn().insert(LineLoad(70));
}

/// The SCADA poll re-measures every line's load each tick.
fn poll_line_loads(mut q: Query<&mut LineLoad>) {
    for mut load in q.iter_mut() {
        load.0 = load.0.wrapping_add(1);
    }
}

/// `Query<&LineLoad, Changed<LineLoad>>` — the telemetry feed lists every
/// line whose load moved this tick. The poll rewrites all three every tick,
/// so the feed always reports all three — including frame 1, where the
/// reader is seeing the freshly seeded values for the first time. Expected:
/// 3, always.
fn telemetry_logs_moved_lines(
    mut board: ResMut<Scoreboard>,
    q: Query<&LineLoad, Changed<LineLoad>>,
) {
    record(
        &mut board,
        "line-telemetry",
        "Query<&LineLoad, Changed<LineLoad>>",
        q.iter().count(),
        3,
    );
}

// ── survey-cache — same filter, but nothing keeps writing, so it quiets ──

/// Seeds three completed surveys; nothing ever rewrites them.
fn seed_completed_surveys(mut commands: Commands) {
    commands.spawn().insert(Surveyed);
    commands.spawn().insert(Surveyed);
    commands.spawn().insert(Surveyed);
}

/// `Query<&Surveyed, Changed<Surveyed>>` — the site planner refreshes its
/// candidate-site cache for tiles whose survey status changed. Frame 1 sees
/// all three (the reader hadn't observed the freshly landed surveys yet);
/// from frame 2 nothing has moved, so the cache stays warm. The contrast
/// with `line-telemetry` is the lesson: identical shape, but a value nobody
/// rewrites stops counting as changed after the first look. Expected:
/// `{1 → 3, else → 0}`.
fn planner_refreshes_new_surveys(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&Surveyed, Changed<Surveyed>>,
) {
    let expected = if frame.0 == 1 { 3 } else { 0 };
    record(
        &mut board,
        "survey-cache",
        "Query<&Surveyed, Changed<Surveyed>>",
        q.iter().count(),
        expected,
    );
}

// ── plant-output / plant-commission — `Changed` (recurring) vs `Added` ───

/// Seeds two metered plants.
fn seed_metered_plants(mut commands: Commands) {
    commands.spawn().insert(Output(20));
    commands.spawn().insert(Output(35));
}

/// The meter sweep overwrites every plant's output each tick.
fn meter_plant_output(mut q: Query<&mut Output>) {
    for mut out in q.iter_mut() {
        out.0 = out.0.wrapping_add(1);
    }
}

/// `Query<&Output, Changed<Output>>` — the dispatcher re-balances any plant
/// whose output moved. The meter overwrites both every tick, so the
/// dispatcher reacts to both, every frame. Expected: 2, always.
fn dispatcher_rebalances_changed_output(
    mut board: ResMut<Scoreboard>,
    q: Query<&Output, Changed<Output>>,
) {
    record(
        &mut board,
        "plant-output",
        "Query<&Output, Changed<Output>>",
        q.iter().count(),
        2,
    );
}

/// `Query<&Output, Added<Output>>` — the commissioning inspector signs off
/// each plant exactly once, when its `Output` is first attached. "Added"
/// means *gained the component*, which never happens twice for these plants,
/// so this fires for both on frame 1 and then never — even as the meter
/// keeps changing their output. The same plants, watched two ways: one
/// reacts forever, one reacts once. Expected: `{1 → 2, else → 0}`.
fn commissioning_inspector_signs_off_new_plants(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&Output, Added<Output>>,
) {
    let expected = if frame.0 == 1 { 2 } else { 0 };
    record(
        &mut board,
        "plant-commission",
        "Query<&Output, Added<Output>>",
        q.iter().count(),
        expected,
    );
}

// ── battery-bank — change marking is precise, not "everything I touched" ─

/// Seeds a five-cell bank at 20 / 40 / 60 / 80 / 100 percent.
fn seed_battery_bank(mut commands: Commands) {
    for percent in [20u32, 40, 60, 80, 100] {
        commands.spawn().insert(BatteryCharge(percent));
    }
}

/// The charge controller tops up only the cells below full (+20, saturating).
/// Iterating `Query<&mut BatteryCharge>` hands out a change-marking handle
/// ([`Mut`](spark_ecs::Mut)) that stamps a cell "changed" **only** when the
/// body actually writes it — a full cell, looked at but skipped, is left
/// alone.
fn charge_low_cells(mut q: Query<&mut BatteryCharge>) {
    for mut cell in q.iter_mut() {
        if cell.0 < 100 {
            cell.0 = (cell.0 + 20).min(100);
        }
    }
}

/// `Query<&BatteryCharge, Changed<BatteryCharge>>` — the dashboard counts
/// cells still taking charge. Frame 1 shows all five (first look at the
/// seeded bank). Then precise marking shows: as each cell hits 100 the
/// controller stops touching it, so it drops out of the count, which decays
/// **5 → 3 → 2 → 1 → 0 across frames 1–5** and stays at 0. If marking were
/// sloppy (every visited cell counted), this would stay stuck at 5.
/// Expected: `{1 → 5, 2 → 3, 3 → 2, 4 → 1, else → 0}`.
fn dashboard_counts_charging_cells(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&BatteryCharge, Changed<BatteryCharge>>,
) {
    let expected = match frame.0 {
        1 => 5,
        2 => 3,
        3 => 2,
        4 => 1,
        _ => 0,
    };
    record(
        &mut board,
        "battery-bank",
        "Query<&BatteryCharge, Changed<BatteryCharge>>",
        q.iter().count(),
        expected,
    );
}

/// Registers Lesson 1. Within each example the writer is added before its
/// observer, so the observer sees the write the writer just made this frame.
/// (Examples use separate component types, so the order *between* examples
/// doesn't matter.)
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed_transmission_lines)
        .add_system(Stage::Update, poll_line_loads)
        .add_system(Stage::Update, telemetry_logs_moved_lines);

    app.add_system(Stage::Startup, seed_completed_surveys)
        .add_system(Stage::Update, planner_refreshes_new_surveys);

    app.add_system(Stage::Startup, seed_metered_plants)
        .add_system(Stage::Update, meter_plant_output)
        .add_system(Stage::Update, dispatcher_rebalances_changed_output)
        .add_system(Stage::Update, commissioning_inspector_signs_off_new_plants);

    app.add_system(Stage::Startup, seed_battery_bank)
        .add_system(Stage::Update, charge_low_cells)
        .add_system(Stage::Update, dashboard_counts_charging_cells);
}
