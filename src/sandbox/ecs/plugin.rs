//! The ECS sub-sandbox's plugin entry point.
//!
//! [`EcsSandboxPlugin`] registers the `Commands`-based seed system on
//! `STARTUP` and the per-frame systems on `PRE_UPDATE` / `UPDATE` /
//! `POST_UPDATE`. It installs *no* runner — that's `WindowPlugin`'s
//! job — and it does **not** add the shared `TickCount` resource;
//! the top-level [`crate::sandbox::SandboxPlugin`] inserts that
//! before this plugin's `build` runs.
//!
//! Adding the plugin in isolation (without `SandboxPlugin`) means
//! the consumer must add `TickCount` to the world themselves first.

use spark_core::{Application, Plugin, stages};

use super::systems::{
    decay_health, physics_step, player_regen, report_initial, report_player_position,
    report_tick_summary, spawn_demo,
};

/// Plugin that wires the ECS sub-sandbox into an [`Application`].
///
/// Build order: every system goes through `add_system` (no
/// resources added here — see the module docs). Seeding happens
/// inside [`spawn_demo`] during `STARTUP` (via `Commands`) so the
/// entities are visible to every later stage's queries — the flush
/// at the `STARTUP` boundary makes them so before `PRE_UPDATE`
/// fires.
pub struct EcsSandboxPlugin;

impl Plugin for EcsSandboxPlugin {
    fn build(&self, app: &mut Application) {
        // ----- Systems -----
        //
        // STARTUP:     queue the demo entities via `Commands`.
        // PRE_UPDATE:  one-shot initial report (first tick only).
        // UPDATE:      the per-tick demo loop —
        //                physics_step    : `(&mut P, &mut V, &Accel)`  (arity-3 multi-mut)
        //                decay_health    : `&mut Health`              (single mut)
        //                player_regen    : `(&Player, &mut Health)`   (arity-2 mut-not-first)
        //                report_tick_…   : `Res<T>` + 2× `Query`
        // POST_UPDATE: player-position log, on settled state.
        //
        // `decay_health` runs *before* `player_regen` so the player's
        // net health change per tick is observable in the logs
        // (-5 from decay, +2 from regen → net -3).
        app.add_system(stages::STARTUP, spawn_demo)
            .add_system(stages::PRE_UPDATE, report_initial)
            .add_system(stages::UPDATE, physics_step)
            .add_system(stages::UPDATE, decay_health)
            .add_system(stages::UPDATE, player_regen)
            .add_system(stages::UPDATE, report_tick_summary)
            .add_system(stages::POST_UPDATE, report_player_position);
    }
}
