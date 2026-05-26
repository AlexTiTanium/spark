//! ## `plant-meter` — `Changed` (every tick) vs `Added` (just once)
//!
//! The game situation: a meter overwrites every power plant's output each
//! tick. Two systems watch the *same* plants two different ways — a
//! dispatcher that re-balances whenever output moves, and a commissioning
//! inspector that signs off each plant when it first appears.
//!
//! The change-detection idea: `Changed<T>` fires whenever `T` is *written*,
//! so the dispatcher reacts every tick. `Added<T>` fires only when an entity
//! *gains* a `T` — which happens once and never again — so the inspector
//! fires for each plant on frame 1 and then never, no matter how much the
//! meter keeps moving the value. Same data, two questions: "did it change?"
//! vs "is it new?".
//!
//! Expected: dispatcher 2 every frame; inspector 2 on frame 1, then 0.

use spark_core::{Application, Stage};
use spark_ecs::{Added, Changed, Commands, Query, Res, ResMut};

use super::super::components::Output;
use super::super::scoreboard::{Frame, Scoreboard, record};

/// Seeds two metered plants.
fn seed(mut commands: Commands) {
    commands.spawn().insert(Output(20));
    commands.spawn().insert(Output(35));
}

/// The meter sweep overwrites every plant's output each tick.
fn meter_plant_output(mut q: Query<&mut Output>) {
    for mut out in q.iter_mut() {
        out.0 = out.0.wrapping_add(1);
    }
}

/// `Query<&Output, Changed<Output>>` — the dispatcher re-balances any plant
/// whose output moved. Both move every tick → 2, always.
fn dispatcher_rebalances_changed_output(
    mut board: ResMut<Scoreboard>,
    q: Query<&Output, Changed<Output>>,
) {
    record(
        &mut board,
        "plant-output",
        "Query<&Output, Changed<Output>>",
        q.iter().count(),
        2,
    );
}

/// `Query<&Output, Added<Output>>` — the commissioning inspector signs off
/// each plant exactly once, when its `Output` is first attached. Fires for
/// both on frame 1, then never (the meter's overwrites don't re-add).
fn commissioning_inspector_signs_off_new_plants(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&Output, Added<Output>>,
) {
    let expected = if frame.0 == 1 { 2 } else { 0 };
    record(
        &mut board,
        "plant-commission",
        "Query<&Output, Added<Output>>",
        q.iter().count(),
        expected,
    );
}

/// Wires this example: seed in `Startup`; meter then both observers in
/// `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, meter_plant_output)
        .add_system(Stage::Update, dispatcher_rebalances_changed_output)
        .add_system(Stage::Update, commissioning_inspector_signs_off_new_plants);
}
