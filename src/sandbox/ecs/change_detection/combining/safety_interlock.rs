//! ## `safety-interlock` — `And` of two change sources
//!
//! The game situation: a reactor's safety interlock re-runs only when
//! **both** its fuel rods and its coolant moved this tick — a believable
//! "only act when everything is live" check.
//!
//! The change-detection idea — the mirror image of [`super::hybrid_output`]:
//! swap `Or` for `And` and "either moved" becomes "both moved". Of the three
//! reactors, one only cycles its rods, one does both, one only circulates
//! coolant. Frame 1 sees all three (both seeded readings are fresh); from
//! frame 2 only the reactor doing *both* still satisfies the `And`.
//!
//! Expected count: 3 on frame 1, then 1.

use spark_core::{Application, Stage};
use spark_ecs::{And, Changed, Commands, Query, Res, ResMut, With};

use super::super::components::{Coolant, CoolantCirculating, FuelRods, NuclearPlant, RodsCycling};
use super::super::scoreboard::{Frame, Scoreboard, record};

/// A plant whose fuel rods **and** coolant both moved since last tick.
type FuelAndCoolantMoved = And<(Changed<FuelRods>, Changed<Coolant>)>;

/// Seeds three reactors (all carry both readings): one only cycling rods,
/// one doing both, one only circulating coolant.
fn seed(mut commands: Commands) {
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

/// `Query<&NuclearPlant, And<(Changed<FuelRods>, Changed<Coolant>)>>` —
/// re-runs only when both readings moved. Settles to the one reactor doing
/// both.
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

/// Wires this example: seed in `Startup`; step both readings then check the
/// interlock in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, cycle_fuel_rods)
        .add_system(Stage::Update, circulate_coolant)
        .add_system(Stage::Update, run_safety_interlock);
}
