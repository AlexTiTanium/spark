//! ## `service-schedule` — write one component, read the next
//!
//! The game situation: running a plant wears it down each tick; a scheduler
//! counts down the service timer of every plant whose wear moved.
//!
//! The change-detection idea — the mirror of [`super::energy_toll`]: here
//! the **written** component (`ServiceCountdown`) comes *first* in the tuple
//! and the *watched* one (`WearLevel`) second. The filter doesn't care about
//! tuple order — only that it watches a component the body doesn't also
//! write. `WearLevel` is read twice (data + filter); that's fine.
//!
//! Expected count: 2, every frame.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, ResMut};

use super::super::components::{ServiceCountdown, WearLevel};
use super::super::scoreboard::{Scoreboard, record};

/// Seeds two operating plants.
fn seed(mut commands: Commands) {
    commands
        .spawn()
        .insert(ServiceCountdown(100))
        .insert(WearLevel(0));
    commands
        .spawn()
        .insert(ServiceCountdown(100))
        .insert(WearLevel(0));
}

/// Running a plant accumulates wear every tick.
fn accumulate_wear(mut q: Query<&mut WearLevel>) {
    for mut wear in q.iter_mut() {
        wear.0 = wear.0.wrapping_add(1);
    }
}

/// `Query<(&mut ServiceCountdown, &WearLevel), Changed<WearLevel>>` — counts
/// down the timer of each plant whose wear moved (written component first).
fn schedule_service(
    mut board: ResMut<Scoreboard>,
    mut q: Query<(&mut ServiceCountdown, &WearLevel), Changed<WearLevel>>,
) {
    let mut n = 0;
    for (mut countdown, _wear) in q.iter_mut() {
        countdown.0 = countdown.0.wrapping_sub(1);
        n += 1;
    }
    record(
        &mut board,
        "service-schedule",
        "Query<(&mut ServiceCountdown, &WearLevel), Changed<WearLevel>>",
        n,
        2,
    );
}

/// Wires this example: seed in `Startup`; wear then schedule in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, accumulate_wear)
        .add_system(Stage::Update, schedule_service);
}
