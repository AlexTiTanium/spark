//! ## `energy-toll` — read one component, write the next in the tuple
//!
//! The game situation: each tick a metering station re-reads its
//! throughput; a toll run bills every station whose throughput moved by
//! adding the flow onto its running total.
//!
//! The change-detection idea: the query reads `Throughput` (the filter reads
//! it too — two reads of the same component are fine) and **writes** the
//! second element, `EnergySold`. The "read drives, write follows" shape — the
//! written component is *second* in the tuple.
//!
//! Expected count: 2, every frame.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, ResMut};

use super::super::components::{EnergySold, Throughput};
use super::super::scoreboard::{Scoreboard, record};

/// Seeds two metering stations.
fn seed(mut commands: Commands) {
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

/// `Query<(&Throughput, &mut EnergySold), Changed<Throughput>>` — bills each
/// station whose throughput moved by adding the flow onto its total.
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

/// Wires this example: seed in `Startup`; update then bill in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, update_throughput)
        .add_system(Stage::Update, accrue_energy_sold);
}
