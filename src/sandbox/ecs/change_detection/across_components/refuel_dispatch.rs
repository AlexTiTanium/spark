//! ## `refuel-dispatch` — watch one component, write another
//!
//! The game situation: each tick combustion burns a plant's fuel down. A
//! dispatcher reacts by bumping a refuel order for every plant whose fuel
//! moved.
//!
//! The change-detection idea: the query **writes** `RefuelOrder` and
//! **watches** `Changed<FuelLevel>` — two *different* components. That's the
//! cornerstone of the whole lesson: you can't write the same component you
//! filter on (the engine refuses `Query<&mut T, Changed<T>>` at startup),
//! so a change always drives a write to a *sibling*. Both plants burn fuel
//! every tick, so both get an order.
//!
//! Expected count: 2, every frame.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, ResMut};

use super::super::components::{FuelLevel, RefuelOrder};
use super::super::scoreboard::{Scoreboard, record};

/// Seeds two fuelled plants.
fn seed(mut commands: Commands) {
    commands
        .spawn()
        .insert(FuelLevel(80))
        .insert(RefuelOrder(0));
    commands
        .spawn()
        .insert(FuelLevel(60))
        .insert(RefuelOrder(0));
}

/// Combustion burns fuel down every tick.
fn burn_fuel(mut q: Query<&mut FuelLevel>) {
    for mut fuel in q.iter_mut() {
        fuel.0 = fuel.0.wrapping_sub(1);
    }
}

/// `Query<&mut RefuelOrder, Changed<FuelLevel>>` — bumps the refuel order of
/// every plant whose fuel moved (writes `RefuelOrder`, watches `FuelLevel`).
fn dispatch_refuel_orders(
    mut board: ResMut<Scoreboard>,
    mut q: Query<&mut RefuelOrder, Changed<FuelLevel>>,
) {
    let mut n = 0;
    for mut order in q.iter_mut() {
        order.0 = order.0.wrapping_add(1);
        n += 1;
    }
    record(
        &mut board,
        "refuel-dispatch",
        "Query<&mut RefuelOrder, Changed<FuelLevel>>",
        n,
        2,
    );
}

/// Wires this example: seed in `Startup`; burn then dispatch in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, burn_fuel)
        .add_system(Stage::Update, dispatch_refuel_orders);
}
