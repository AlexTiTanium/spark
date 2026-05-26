//! ## `line-telemetry` — `Changed<T>` keeps firing while something writes
//!
//! The game situation: the grid's monitoring system (SCADA) re-measures the
//! power on every transmission line each tick, and a telemetry feed logs
//! every line whose load moved.
//!
//! The change-detection idea: `Changed<T>` means "written since *this*
//! system last ran". Because the poll rewrites all three lines every tick,
//! the feed always sees all three — **including frame 1**, where it's
//! observing the freshly seeded values for the very first time.
//!
//! Expected count: 3, every frame.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, ResMut};

use super::super::components::LineLoad;
use super::super::scoreboard::{Scoreboard, record};

/// Seeds three monitored transmission lines.
fn seed(mut commands: Commands) {
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

/// `Query<&LineLoad, Changed<LineLoad>>` — lists every line whose load moved
/// this tick. All three move every tick, so the count is always 3.
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

/// Wires this example: seed in `Startup`; poll then observe in `Update`
/// (the poll runs first so the observer sees this tick's writes).
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, poll_line_loads)
        .add_system(Stage::Update, telemetry_logs_moved_lines);
}
