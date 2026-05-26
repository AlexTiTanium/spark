//! ## `survey-cache` — the same filter goes *quiet* when nothing rewrites
//!
//! The game situation: when a map tile's terrain survey finishes, the tile
//! gets a `Surveyed` flag. A site planner refreshes its candidate-site cache
//! for tiles whose survey status changed.
//!
//! The change-detection idea — and the contrast with
//! [`super::line_telemetry`]: the query shape and filter are identical
//! (`&T` + `Changed<T>`), but here **nobody keeps writing**. A survey lands
//! once and never moves. So the planner sees all three the first time it
//! looks (it hadn't observed them yet — "changed" is relative to the
//! reader), and from then on the cache stays warm. Same tool, opposite
//! outcome; the difference is who keeps writing.
//!
//! Expected count: 3 on frame 1, then 0 forever.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, Res, ResMut};

use super::super::components::Surveyed;
use super::super::scoreboard::{Frame, Scoreboard, record};

/// Seeds three completed surveys; nothing ever rewrites them.
fn seed(mut commands: Commands) {
    commands.spawn().insert(Surveyed);
    commands.spawn().insert(Surveyed);
    commands.spawn().insert(Surveyed);
}

/// `Query<&Surveyed, Changed<Surveyed>>` — refreshes the cache for tiles
/// whose survey status changed. Frame 1 sees all three (first look); from
/// frame 2 nothing has moved.
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

/// Wires this example: seed in `Startup`; observe in `Update`. (No writer —
/// that's the whole point.)
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, planner_refreshes_new_surveys);
}
