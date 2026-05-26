//! ## `hydro-dam` — three-part `And` and `Or`
//!
//! The game situation: a hydro dam has three flows — the penstock (intake
//! pressure), the turbine (RPM), and the tailrace (outflow). A "full
//! generation" light needs all three flowing; an "activity" indicator needs
//! any one.
//!
//! The change-detection idea: the combinators aren't limited to two parts —
//! `And` and `Or` take 2-, 3-, and 4-part tuples. Of four dams, only the
//! second has all three flows active; the fourth is fully idle. The same
//! roster, read two ways: a three-part `And` (all moved) and a three-part
//! `Or` (any moved). Frame 1 both see all four (every flow is fresh); from
//! frame 2 the `And` keeps only the all-active dam, while the `Or` keeps the
//! three with any activity.
//!
//! Expected: full-flow 4 then 1; any-activity 4 then 3.

use spark_core::{Application, Stage};
use spark_ecs::{And, Changed, Commands, Or, Query, Res, ResMut, With};

use super::super::components::{
    Penstock, PenstockOpen, Tailrace, TailraceDraining, Turbine, TurbineSpinning,
};
use super::super::scoreboard::{Frame, Scoreboard, record};

/// All three of a dam's flows moved this tick (full generation).
type AllFlowsMoved = And<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>;
/// Any of a dam's flows moved this tick (some activity).
type AnyFlowMoved = Or<(Changed<Penstock>, Changed<Turbine>, Changed<Tailrace>)>;

/// Seeds four hydro dams (all carry all three flows): only the second has
/// every flow active; the fourth is fully idle.
fn seed(mut commands: Commands) {
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
/// full-generation light: all three flows moving. Settles to the one fully
/// active dam.
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
/// activity indicator: any one flow moving. Settles to the three non-idle
/// dams.
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

/// Wires this example: seed in `Startup`; step the three flows then run both
/// checks in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, flow_penstock)
        .add_system(Stage::Update, spin_turbine)
        .add_system(Stage::Update, drain_tailrace)
        .add_system(Stage::Update, check_full_flow)
        .add_system(Stage::Update, check_any_activity);
}
