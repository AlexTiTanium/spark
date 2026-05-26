//! ## `transmission-segment` — one read join, watched with `Changed` *and* `Added`
//!
//! The game situation: a loss calculator recomputes endpoint segments whose
//! load moved; separately, a registrar logs newly-laid endpoint segments.
//!
//! The change-detection idea: both systems read the same two-component join
//! `(&SegmentLoad, &Endpoint)`, but ask different questions —
//! `Changed<SegmentLoad>` ("did the load move?") fires every tick, while
//! `Added<Endpoint>` ("is this segment new?") fires once. Note `Added` here
//! watches `Endpoint`, the *same* flag the join already reads; sharing a
//! read is allowed.
//!
//! Expected: loss calculator 2 every frame; registrar 2 on frame 1, then 0.

use spark_core::{Application, Stage};
use spark_ecs::{Added, Changed, Commands, Query, Res, ResMut};

use super::super::components::{Endpoint, SegmentLoad};
use super::super::scoreboard::{Frame, Scoreboard, record};

/// Seeds two endpoint segments plus one mid-span segment (no endpoint) the
/// join must skip.
fn seed(mut commands: Commands) {
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

/// `Query<(&SegmentLoad, &Endpoint), Changed<SegmentLoad>>` — recomputes
/// endpoint segments whose load moved. Two endpoints, both moving → 2.
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

/// `Query<(&SegmentLoad, &Endpoint), Added<Endpoint>>` — logs newly-laid
/// endpoint segments. One-shot: both on frame 1, then none.
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

/// Wires this example: seed in `Startup`; update then both observers in
/// `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, update_segment_loads)
        .add_system(Stage::Update, recompute_segment_loss)
        .add_system(Stage::Update, register_new_segments);
}
