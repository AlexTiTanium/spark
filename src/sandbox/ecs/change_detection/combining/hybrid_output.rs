//! ## `hybrid-output` — `Or` of two change sources
//!
//! The game situation: a hybrid renewable site has both a solar array and a
//! wind turbine. The dispatch recomputes a site's net output if **either**
//! source moved this tick.
//!
//! The change-detection idea: wrap two `Changed<T>` filters in `Or`. Of the
//! three sites, one is sun-exposed (its solar moves), one is wind-exposed
//! (its wind moves), and one is exposed to neither. Frame 1 sees all three
//! (first look at the seeded yields); from frame 2 the becalmed, shaded site
//! drops out because *neither* of its sources is moving.
//!
//! Expected count: 3 on frame 1, then 2.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Or, Query, Res, ResMut, With};

use super::super::components::{HybridSite, SolarYield, SunExposed, WindExposed, WindYield};
use super::super::scoreboard::{Frame, Scoreboard, record};

/// A site whose solar **or** wind yield moved since last tick.
type SolarOrWindMoved = Or<(Changed<SolarYield>, Changed<WindYield>)>;

/// Seeds three hybrid sites (all carry both yields): one sun-exposed, one in
/// shade and still air, one wind-exposed.
fn seed(mut commands: Commands) {
    commands
        .spawn()
        .insert(HybridSite)
        .insert(SolarYield(10))
        .insert(WindYield(10))
        .insert(SunExposed);
    commands
        .spawn()
        .insert(HybridSite)
        .insert(SolarYield(20))
        .insert(WindYield(20)); // exposed to neither
    commands
        .spawn()
        .insert(HybridSite)
        .insert(SolarYield(30))
        .insert(WindYield(30))
        .insert(WindExposed);
}

/// Solar yield updates only on sun-exposed sites.
fn update_solar_yield(mut q: Query<&mut SolarYield, With<SunExposed>>) {
    for mut y in q.iter_mut() {
        y.0 = y.0.wrapping_add(1);
    }
}

/// Wind yield updates only on wind-exposed sites.
fn update_wind_yield(mut q: Query<&mut WindYield, With<WindExposed>>) {
    for mut y in q.iter_mut() {
        y.0 = y.0.wrapping_add(1);
    }
}

/// `Query<&HybridSite, Or<(Changed<SolarYield>, Changed<WindYield>)>>` —
/// recomputes a site if either source moved. Drops to 2 once the
/// neither-exposed site goes still.
fn recompute_hybrid_output(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&HybridSite, SolarOrWindMoved>,
) {
    let expected = if frame.0 == 1 { 3 } else { 2 };
    record(
        &mut board,
        "hybrid-output",
        "Query<&HybridSite, Or<(Changed<SolarYield>, Changed<WindYield>)>>",
        q.iter().count(),
        expected,
    );
}

/// Wires this example: seed in `Startup`; update both sources then recompute
/// in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, update_solar_yield)
        .add_system(Stage::Update, update_wind_yield)
        .add_system(Stage::Update, recompute_hybrid_output);
}
