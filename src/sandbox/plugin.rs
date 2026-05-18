//! The plugin entry point.
//!
//! [`SandboxPlugin`] seeds the world with four entities, registers
//! the five demo systems across `STARTUP` / `UPDATE`, and installs a
//! runner that ticks `UPDATE` five times before returning. The world-
//! seeding happens via `app.world_mut()` — the registration-time
//! escape hatch from M3 Issue A.

use spark_core::{Application, Plugin, stages};
use spark_log::info;

use super::components::{Health, Player, Position, Velocity};
use super::resources::TickCount;
use super::systems::{
    decay_health, integrate_movement, report_initial, report_player_position, report_tick_summary,
};

/// Plugin that wires the sandbox demo into an [`Application`].
///
/// Build order matters: the resource lands first, then the entities
/// (so the systems registered next have something to walk), then the
/// runner (last-write-wins on the `Application`).
pub struct SandboxPlugin;

impl Plugin for SandboxPlugin {
    fn build(&self, app: &mut Application) {
        // ----- Resources -----
        app.add_resource(TickCount(0));

        // ----- Components on four entities -----
        //
        // `mover_a`, `mover_b`: Position + Velocity + Health
        // `player`:             Position + Velocity + Health + Player
        // `statue`:             Position + Health (no Velocity)
        //
        // The `statue` is what makes the movement join interesting —
        // it's present in the Position storage but the join must skip
        // it because it has no Velocity.
        let world = app.world_mut();
        world
            .spawn()
            .insert(Position { x: 0.0, y: 0.0 })
            .insert(Velocity { x: 1.0, y: 0.5 })
            .insert(Health(100));
        world
            .spawn()
            .insert(Position { x: 10.0, y: -5.0 })
            .insert(Velocity { x: -0.5, y: 1.0 })
            .insert(Health(75));
        world
            .spawn()
            .insert(Position { x: 0.0, y: 0.0 })
            .insert(Velocity { x: 2.0, y: 0.0 })
            .insert(Health(200))
            .insert(Player);
        world
            .spawn()
            .insert(Position { x: 50.0, y: 50.0 })
            .insert(Health(50));

        // ----- Systems -----
        //
        // STARTUP: one-shot report of the seeded state.
        // UPDATE:  the per-tick demo loop; our custom runner drives it.
        app.add_system(stages::STARTUP, report_initial)
            .add_system(stages::UPDATE, integrate_movement)
            .add_system(stages::UPDATE, decay_health)
            .add_system(stages::UPDATE, report_player_position)
            .add_system(stages::UPDATE, report_tick_summary);

        // ----- Custom runner -----
        //
        // Drives `UPDATE` exactly five times, then returns. No window,
        // no event loop — this is what "simple runner" looks like.
        app.set_runner(|mut app| {
            for _ in 0..5 {
                app.run_stage(stages::UPDATE);
            }
            info!("sandbox: runner finished after 5 ticks");
            Ok(())
        });
    }
}
