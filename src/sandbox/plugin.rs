//! The plugin entry point.
//!
//! [`SandboxPlugin`] registers a `Commands`-based seed system on
//! `STARTUP` and the per-frame systems on `PRE_UPDATE` / `UPDATE` /
//! `POST_UPDATE`. It installs *no* runner — that's `WindowPlugin`'s
//! job. The whole point of M3 Issue C is that any plugin can register
//! systems and the `WindowPlugin`'s per-frame loop ticks them; the
//! sandbox is the canonical demo of that.

use spark_core::{Application, Plugin, stages};

use super::resources::TickCount;
use super::systems::{
    decay_health, integrate_movement, report_initial, report_player_position, report_tick_summary,
    spawn_demo,
};

/// Plugin that wires the sandbox demo into an [`Application`].
///
/// Build order: the resource lands first, then every system. Seeding
/// happens inside [`spawn_demo`] during `STARTUP` (via `Commands`) so
/// the entities are visible to every later stage's queries — the
/// flush at the `STARTUP` boundary makes them so before `PRE_UPDATE`
/// fires.
pub struct SandboxPlugin;

impl Plugin for SandboxPlugin {
    fn build(&self, app: &mut Application) {
        // ----- Resources -----
        app.add_resource(TickCount(0));

        // ----- Systems -----
        //
        // STARTUP:     queue the four demo entities via `Commands`.
        // PRE_UPDATE:  one-shot initial report (first tick only).
        // UPDATE:      the per-tick demo loop — movement, decay, snapshot.
        // POST_UPDATE: player-position log, on settled state.
        app.add_system(stages::STARTUP, spawn_demo)
            .add_system(stages::PRE_UPDATE, report_initial)
            .add_system(stages::UPDATE, integrate_movement)
            .add_system(stages::UPDATE, decay_health)
            .add_system(stages::UPDATE, report_tick_summary)
            .add_system(stages::POST_UPDATE, report_player_position);
    }
}
