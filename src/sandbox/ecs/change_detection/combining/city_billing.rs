//! ## `city-billing` — `Changed` AND a `With` / `Without` flag
//!
//! The game situation: every city's power demand shifts each tick. A billing
//! run charges the grid-*connected* cities whose demand moved; a brownout
//! monitor flags the *off-grid* ones whose demand moved.
//!
//! The change-detection idea: a `Changed<T>` filter combines with a presence
//! filter via `And`. Demand moves on all three cities, so the `With<Connected>`
//! / `Without<Connected>` half does the splitting. Between them the two checks
//! partition every changed city (2 + 1 = 3) — proof that `With` and `Without`
//! are exact complements.
//!
//! Expected: billing 2, off-grid alert 1 — every frame.

use spark_core::{Application, Stage};
use spark_ecs::{And, Changed, Commands, Query, ResMut, With, Without};

use super::super::components::{CityDemand, Connected};
use super::super::scoreboard::{Scoreboard, record};

/// Grid-connected cities whose demand moved this tick.
type ConnectedDemandChanged = And<(With<Connected>, Changed<CityDemand>)>;
/// Off-grid cities whose demand moved — the exact complement.
type OffgridDemandChanged = And<(Without<Connected>, Changed<CityDemand>)>;

/// Seeds three cities; two are grid-connected, one is off-grid.
fn seed(mut commands: Commands) {
    commands.spawn().insert(CityDemand(50)).insert(Connected);
    commands.spawn().insert(CityDemand(80)).insert(Connected);
    commands.spawn().insert(CityDemand(30)); // off-grid hamlet
}

/// Every city's demand shifts each tick (a day/night load curve).
fn update_city_demand(mut q: Query<&mut CityDemand>) {
    for mut d in q.iter_mut() {
        d.0 = d.0.wrapping_add(1);
    }
}

/// `And<(With<Connected>, Changed<CityDemand>)>` — bills connected cities
/// whose demand moved. The two connected ones.
fn bill_connected_cities(
    mut board: ResMut<Scoreboard>,
    q: Query<&CityDemand, ConnectedDemandChanged>,
) {
    record(
        &mut board,
        "billing",
        "Query<&CityDemand, And<(With<Connected>, Changed<CityDemand>)>>",
        q.iter().count(),
        2,
    );
}

/// `And<(Without<Connected>, Changed<CityDemand>)>` — flags off-grid cities
/// whose demand moved. The lone off-grid one.
fn alert_offgrid_cities(
    mut board: ResMut<Scoreboard>,
    q: Query<&CityDemand, OffgridDemandChanged>,
) {
    record(
        &mut board,
        "offgrid-alert",
        "Query<&CityDemand, And<(Without<Connected>, Changed<CityDemand>)>>",
        q.iter().count(),
        1,
    );
}

/// Wires this example: seed in `Startup`; update then both observers in
/// `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, update_city_demand)
        .add_system(Stage::Update, bill_connected_cities)
        .add_system(Stage::Update, alert_offgrid_cities);
}
