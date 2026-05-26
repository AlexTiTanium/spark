//! ## `substation-heat` — write *two* components, gated by a third's change
//!
//! The game situation: a thermal model recomputes both the cable and coil
//! temperatures of every substation whose load signal moved this tick.
//!
//! The change-detection idea: writing two components at once (`(&mut
//! CableTemp, &mut CoilTemp)`) is the "multi-mut" query shape, and the
//! `Changed<LoadSignal>` filter clips onto it unchanged. Frame 1 reacts to
//! all three substations (first look at the seeded signals); from frame 2
//! only the two *energised* ones keep changing, and the idle one is never
//! recomputed — the filter trims work even on a two-write join.
//!
//! Expected count: 3 on frame 1, then 2.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, Res, ResMut, With};

use super::super::components::{CableTemp, CoilTemp, Energised, LoadSignal};
use super::super::scoreboard::{Frame, Scoreboard, record};

/// Seeds three substations (cable + coil + load signal); two are energised,
/// one is idle.
fn seed(mut commands: Commands) {
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

/// `Query<(&mut CableTemp, &mut CoilTemp), Changed<LoadSignal>>` — recomputes
/// both temperatures for substations whose load signal moved. Drops to 2
/// once only the energised pair keeps changing.
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

/// Wires this example: seed in `Startup`; pulse then recompute in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, pulse_load_signals)
        .add_system(Stage::Update, recompute_substation_heat);
}
