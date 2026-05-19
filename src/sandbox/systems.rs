//! Demo systems, one per `SystemParam` shape the ECS supports today.
//!
//! Each function exercises a distinct combination so the log output
//! paints a clear picture of what works:
//!
//! - [`spawn_demo`]: `Commands` — seeds the four demo entities from a
//!   STARTUP system (deferred spawn pattern that lands with Issue C).
//! - [`report_initial`]: `Res<T>` + `Query<&T>` — runs in `PRE_UPDATE`
//!   so it sees the entities `spawn_demo` queued in STARTUP (those
//!   flush at the STARTUP boundary, before `PRE_UPDATE` fires).
//! - [`integrate_movement`]: `Query<(&mut A, &B)>` (canonical join)
//! - [`decay_health`]: `ResMut<T>` + `Query<&mut T>`
//! - [`report_player_position`]: `Query<(&A, &Marker)>` (marker-as-filter)
//! - [`report_tick_summary`]: `Res<T>` + two `Query`s in one signature
//!
//! `clippy::needless_pass_by_value` is allowed at the crate level (see
//! `src/Cargo.toml`) — `IntoSystem`'s calling convention hands every
//! `SystemParam` to user fns by value, so the lint can't apply here.

use spark_ecs::{Commands, Query, Res, ResMut};
use spark_log::{debug, info};

use super::components::{Health, Player, Position, Velocity};
use super::resources::TickCount;

/// **`Commands`** — seeds the four demo entities deferred-style. Runs
/// in `STARTUP`; the entities are flushed in by the time `PRE_UPDATE`
/// fires for the first time.
///
/// `mover_a`, `mover_b`: `Position + Velocity + Health`
/// `player`: `Position + Velocity + Health + Player`
/// `statue`: `Position + Health` (no `Velocity`)
///
/// The `statue` is what makes the movement join interesting — it's
/// present in the `Position` storage but the join must skip it because
/// it has no `Velocity`.
pub(super) fn spawn_demo(mut commands: Commands) {
    commands
        .spawn()
        .insert(Position { x: 0.0, y: 0.0 })
        .insert(Velocity { x: 1.0, y: 0.5 })
        .insert(Health(100));
    commands
        .spawn()
        .insert(Position { x: 10.0, y: -5.0 })
        .insert(Velocity { x: -0.5, y: 1.0 })
        .insert(Health(75));
    commands
        .spawn()
        .insert(Position { x: 0.0, y: 0.0 })
        .insert(Velocity { x: 2.0, y: 0.0 })
        .insert(Health(200))
        .insert(Player);
    commands
        .spawn()
        .insert(Position { x: 50.0, y: 50.0 })
        .insert(Health(50));
    info!("sandbox: spawn_demo queued 4 entities (Commands)");
}

/// **`Res<T>` + `Query<&T>`** — read a singleton resource and count
/// entities matching a single-component shape. Runs in `PRE_UPDATE`
/// so it sees the entities [`spawn_demo`] queued during `STARTUP`
/// (they flush at the stage boundary before `PRE_UPDATE` fires).
pub(super) fn report_initial(tick: Res<TickCount>, q_pos: Query<&Position>) {
    // First-tick guard so this only logs once. (A `Local<T>` system
    // param would be tidier; lands in a follow-up.)
    if tick.0 != 0 {
        return;
    }
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
    debug!(
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
        debug!(
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
    debug!(
        tick = tick.0,
        ?positions,
        ?healths,
        "sandbox: report_tick_summary — Res + 2× Query"
    );
}
