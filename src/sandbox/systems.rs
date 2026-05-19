//! Demo systems, one per `SystemParam` shape the ECS supports today.
//!
//! Each function exercises a distinct combination so the log output
//! paints a clear picture of what works:
//!
//! - [`report_initial`]: `Res<T>` + `Query<&T>`
//! - [`integrate_movement`]: `Query<(&mut A, &B)>` (canonical join)
//! - [`decay_health`]: `ResMut<T>` + `Query<&mut T>`
//! - [`report_player_position`]: `Query<(&A, &Marker)>` (marker-as-filter)
//! - [`report_tick_summary`]: `Res<T>` + two `Query`s in one signature
//!
//! `clippy::needless_pass_by_value` is allowed at the crate level (see
//! `src/Cargo.toml`) — `IntoSystem`'s calling convention hands every
//! `SystemParam` to user fns by value, so the lint can't apply here.

use spark_ecs::{Query, Res, ResMut};
use spark_log::{debug, info};

use super::components::{Health, Player, Position, Velocity};
use super::resources::TickCount;

/// **`Res<T>` + `Query<&T>`** — read a singleton resource and count
/// entities matching a single-component shape. Runs once during
/// STARTUP, before the runner takes over.
pub(super) fn report_initial(tick: Res<TickCount>, q_pos: Query<&Position>) {
    let count = q_pos.iter().count();
    info!(
        tick = tick.0,
        entities_with_position = count,
        "sandbox: initial state — Res<TickCount> + Query<&Position>"
    );
}

/// **`Query<(&mut Position, &Velocity)>`** — the canonical movement
/// system. Drives `Position`'s storage and sparse-looks-up `Velocity`
/// for each entity. The `statue` (no Velocity) is skipped by the
/// join, even though Position drives the walk.
pub(super) fn integrate_movement(mut q: Query<(&mut Position, &Velocity)>) {
    let mut moved = 0;
    for (pos, vel) in q.iter_mut() {
        pos.x += vel.x;
        pos.y += vel.y;
        moved += 1;
    }
    debug!(moved, "sandbox: integrate_movement — Query<(&mut P, &V)>");
}

/// **`ResMut<T>` + `Query<&mut T>`** — bump a singleton counter and
/// decay every entity's `Health`. Demonstrates that a system can
/// hold both a resource write and a component-storage write in the
/// same call.
pub(super) fn decay_health(mut tick: ResMut<TickCount>, mut q: Query<&mut Health>) {
    tick.0 += 1;
    let mut decayed = 0;
    for h in q.iter_mut() {
        h.0 = h.0.saturating_sub(5);
        decayed += 1;
    }
    info!(
        tick = tick.0,
        decayed, "sandbox: decay_health — ResMut<TickCount> + Query<&mut Health>"
    );
}

/// **`Query<(&Position, &Player)>`** — shared two-tuple join that uses
/// a zero-sized marker as a "filter via tuple element". Only the
/// player-tagged entity yields. (Real `With<T>` filters arrive in a
/// follow-up PR; today the marker-in-tuple pattern fills the gap.)
pub(super) fn report_player_position(q: Query<(&Position, &Player)>) {
    for (pos, _player) in q.iter() {
        info!(
            x = pos.x,
            y = pos.y,
            "sandbox: report_player_position — Query<(&Position, &Player)>"
        );
    }
}

/// **`Res<T>` + `Query<&T>` × 2** — two queries in one system, over
/// different storage types. Shows that arity-3 system fns work and
/// that mixing `Res` with multiple `Query`s in a single signature is
/// the intended ergonomics.
pub(super) fn report_tick_summary(
    tick: Res<TickCount>,
    q_pos: Query<&Position>,
    q_health: Query<&Health>,
) {
    let positions: Vec<(f32, f32)> = q_pos.iter().map(|p| (p.x, p.y)).collect();
    let healths: Vec<u32> = q_health.iter().map(|h| h.0).collect();
    info!(
        tick = tick.0,
        ?positions,
        ?healths,
        "sandbox: report_tick_summary — Res + 2× Query"
    );
}
