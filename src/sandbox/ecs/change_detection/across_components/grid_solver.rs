//! ## `grid-solver` — a change filter on a three-component read
//!
//! The game situation: a grid solver re-solves only the *complete*
//! substations — the nodes that have both a transformer and a feeder — whose
//! bus voltage drifted this tick.
//!
//! The change-detection idea: the `Changed<BusVoltage>` filter rides a query
//! that reads three components at once (`(&BusVoltage, &Transformer,
//! &Feeder)`). All three buses drift every tick, but a bare bus with no
//! transformer or feeder isn't even in the join — it's dropped before the
//! filter matters. So the filter narrows *within* the set the shape already
//! selected.
//!
//! Expected count: 2, every frame.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, ResMut};

use super::super::components::{BusVoltage, Feeder, Transformer};
use super::super::scoreboard::{Scoreboard, record};

/// Seeds two complete substations (bus + transformer + feeder) plus one bare
/// bus the solver must skip even though its voltage moves.
fn seed(mut commands: Commands) {
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

/// `Query<(&BusVoltage, &Transformer, &Feeder), Changed<BusVoltage>>` —
/// re-solves complete substations whose voltage moved. The bare bus fails
/// the three-way join, so only the two real substations count.
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

/// Wires this example: seed in `Startup`; drift then solve in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, fluctuate_bus_voltage)
        .add_system(Stage::Update, grid_solver_resolves_substations);
}
