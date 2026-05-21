//! The ECS sub-sandbox's plugin entry point.
//!
//! [`EcsSandboxPlugin`] registers the `Commands`-based seed system on
//! `Startup` and the per-frame systems on `PreUpdate` / `Update` /
//! `PostUpdate`. It installs *no* runner — that's `WindowPlugin`'s
//! job — and it does **not** add the shared `TickCount` resource;
//! the top-level [`crate::sandbox::SandboxPlugin`] inserts that
//! before this plugin's `build` runs.
//!
//! Adding the plugin in isolation (without `SandboxPlugin`) means
//! the consumer must add `TickCount` to the world themselves first.

use spark_core::{Application, Plugin, Stage};

use super::filters::{
    and_filter, bump_powered_capacity, filtered_join, nested_filter, or_filter, spawn_filter_demo,
    with_filter, without_filter,
};
use super::systems::{
    decay_health, physics_step, player_regen, report_initial, report_player_position,
    report_tick_summary, spawn_demo,
};

/// Plugin that wires the ECS sub-sandbox into an [`Application`].
///
/// Build order: every system goes through `add_system` (no
/// resources added here — see the module docs). Seeding happens
/// inside [`spawn_demo`] during `Startup` (via `Commands`) so the
/// entities are visible to every later stage's queries — the flush
/// at the `Startup` boundary makes them so before `PreUpdate`
/// fires.
pub struct EcsSandboxPlugin;

impl Plugin for EcsSandboxPlugin {
    fn build(&self, app: &mut Application) {
        // ----- Systems -----
        //
        // Startup:     queue the demo entities via `Commands`.
        // PreUpdate:   one-shot initial report (first tick only).
        // Update:      the per-tick demo loop —
        //                physics_step    : `(&mut P, &mut V, &Accel)`  (arity-3 multi-mut)
        //                decay_health    : `&mut Health`              (single mut)
        //                player_regen    : `(&Player, &mut Health)`   (arity-2 mut-not-first)
        //                report_tick_…   : `Res<T>` + 2× `Query`
        // PostUpdate:  player-position log, on settled state.
        //
        // `decay_health` runs *before* `player_regen` so the player's
        // net health change per tick is observable in the logs
        // (-5 from decay, +2 from regen → net -3).
        app.add_system(Stage::Startup, spawn_demo)
            .add_system(Stage::PreUpdate, report_initial)
            .add_system(Stage::Update, physics_step)
            .add_system(Stage::Update, decay_health)
            .add_system(Stage::Update, player_regen)
            .add_system(Stage::Update, report_tick_summary)
            .add_system(Stage::PostUpdate, report_player_position);

        // ----- Filter demo (`Query<D, F>`) -----
        //
        // Seed the power-grid roster in Startup (flushes before
        // PreUpdate), then run one report per filter combination in
        // PreUpdate. Each is first-tick-gated, so the whole set logs
        // its expected-vs-actual matches once. `filtered_join` (read)
        // is registered before `bump_powered_capacity` (mut) so the
        // join reports the original capacities before the bump applies.
        app.add_system(Stage::Startup, spawn_filter_demo)
            .add_system(Stage::PreUpdate, with_filter)
            .add_system(Stage::PreUpdate, without_filter)
            .add_system(Stage::PreUpdate, and_filter)
            .add_system(Stage::PreUpdate, or_filter)
            .add_system(Stage::PreUpdate, nested_filter)
            .add_system(Stage::PreUpdate, filtered_join)
            .add_system(Stage::PreUpdate, bump_powered_capacity);
    }
}
