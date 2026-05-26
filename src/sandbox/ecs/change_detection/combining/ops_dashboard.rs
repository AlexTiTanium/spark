//! ## `ops-dashboard` — a nested combination
//!
//! The game situation: an operations dashboard lists the generators worth a
//! glance — the *online* ones that are "lively", meaning they either **just
//! commissioned** or are **streaming fresh telemetry**.
//!
//! The change-detection idea: combinators nest. This is `With<Online>` AND
//! (`Changed<Telemetry>` OR `Added<Commissioned>`) — a flag, an `Or`, a
//! `Changed`, and an `Added`, all in one filter. Of four generators, two are
//! online + reporting, one is online but silent, one is offline. Frame 1: all
//! three online ones light up (each just commissioned and/or has fresh
//! telemetry); the offline one is gated out by `With<Online>`. From frame 2
//! the one-shot `Added` half is spent, so the silent online generator drops
//! off, leaving the two still reporting. The whole lesson in one query.
//!
//! Expected count: 3 on frame 1, then 2.

use spark_core::{Application, Stage};
use spark_ecs::{Added, And, Changed, Commands, Or, Query, Res, ResMut, With};

use super::super::components::{Commissioned, Generator, Online, Reporting, Telemetry};
use super::super::scoreboard::{Frame, Scoreboard, record};

/// Online generators that either just commissioned **or** are streaming
/// fresh telemetry.
type OnlineAndLively = And<(With<Online>, Or<(Changed<Telemetry>, Added<Commissioned>)>)>;

/// Seeds four generators: two online + reporting, one online but silent, one
/// offline. All carry `Telemetry` and `Commissioned`.
fn seed(mut commands: Commands) {
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
/// Frame 1 includes the silent-but-just-commissioned generator (via `Added`);
/// from frame 2 only the two still streaming remain.
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

/// Wires this example: seed in `Startup`; stream then refresh the dashboard
/// in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, stream_telemetry)
        .add_system(Stage::Update, update_ops_dashboard);
}
