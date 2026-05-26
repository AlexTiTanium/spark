//! ## `battery-bank` — change marking is *precise*, not "everything I saw"
//!
//! The game situation: a storage bank of five cells. A charge controller
//! tops up only the cells below full each tick; a dashboard counts how many
//! are still taking charge.
//!
//! The change-detection idea — the payoff of the whole feature: iterating
//! `Query<&mut BatteryCharge>` hands out a change-marking handle that stamps
//! a cell "changed" **only when the body actually writes it**. A full cell
//! the controller skips is left untouched, so it drops out of the count.
//! That's why the tally decays **5 → 3 → 2 → 1 → 0** as the bank fills. If
//! marking were sloppy (every cell the loop *looked at* counted), it would
//! stay stuck at 5.
//!
//! Expected count: `{frame 1 → 5, 2 → 3, 3 → 2, 4 → 1, then → 0}`.

use spark_core::{Application, Stage};
use spark_ecs::{Changed, Commands, Query, Res, ResMut};

use super::super::components::BatteryCharge;
use super::super::scoreboard::{Frame, Scoreboard, record};

/// Seeds a five-cell bank at 20 / 40 / 60 / 80 / 100 percent.
fn seed(mut commands: Commands) {
    for percent in [20u32, 40, 60, 80, 100] {
        commands.spawn().insert(BatteryCharge(percent));
    }
}

/// The charge controller tops up only the cells below full (+20, saturating).
/// Skipping a full cell leaves it unmarked — that's the precision.
fn charge_low_cells(mut q: Query<&mut BatteryCharge>) {
    for mut cell in q.iter_mut() {
        if cell.0 < 100 {
            cell.0 = (cell.0 + 20).min(100);
        }
    }
}

/// `Query<&BatteryCharge, Changed<BatteryCharge>>` — counts cells still
/// taking charge. Decays to zero as cells reach 100 and stop being written.
fn dashboard_counts_charging_cells(
    frame: Res<Frame>,
    mut board: ResMut<Scoreboard>,
    q: Query<&BatteryCharge, Changed<BatteryCharge>>,
) {
    let expected = match frame.0 {
        1 => 5,
        2 => 3,
        3 => 2,
        4 => 1,
        _ => 0,
    };
    record(
        &mut board,
        "battery-bank",
        "Query<&BatteryCharge, Changed<BatteryCharge>>",
        q.iter().count(),
        expected,
    );
}

/// Wires this example: seed in `Startup`; charge then observe in `Update`.
pub(super) fn register(app: &mut Application) {
    app.add_system(Stage::Startup, seed)
        .add_system(Stage::Update, charge_low_cells)
        .add_system(Stage::Update, dashboard_counts_charging_cells);
}
