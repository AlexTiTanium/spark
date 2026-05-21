//! Demo systems, each exercising a distinct `SystemParam` and/or
//! `Query` shape the ECS supports today.
//!
//! What each system shows:
//!
//! - [`spawn_demo`]: `Commands` — seeds the four demo entities from a
//!   Startup system (deferred spawn pattern that lands with Issue C).
//! - [`report_initial`]: `Res<T>` + `Query<&T>` — runs in `PreUpdate`
//!   so it sees the entities `spawn_demo` queued in Startup (those
//!   flush at the Startup boundary before `PreUpdate` fires).
//! - [`physics_step`]: `Query<(&mut P, &mut V, &A)>` — **arity-3
//!   multi-mut**, new with the multi-mut PR. Symplectic Euler step.
//! - [`decay_health`]: `ResMut<T>` + `Query<&mut T>` — bumps the tick
//!   counter and decays every entity's health.
//! - [`player_regen`]: `Query<(&Player, &mut Health)>` — **arity-2
//!   mut-not-first**, new with the multi-mut PR. Read-driver (Player
//!   marker), mut non-driver (Health). Player-only regen.
//! - [`report_player_position`]: `Query<(&P, &Player)>` — shared
//!   two-tuple join with a zero-sized marker filtering down to the
//!   player.
//! - [`report_tick_summary`]: `Res<T>` + two `Query`s in one
//!   signature — three system params side by side.
//!
//! `clippy::needless_pass_by_value` is allowed at the crate level
//! (see `src/Cargo.toml`) — `IntoSystem`'s calling convention hands
//! every `SystemParam` to user fns by value, so the lint can't apply
//! here.

use spark_ecs::{Commands, Query, Res, ResMut};
use spark_log::{debug, info};

// Shared spatial / gameplay primitives live at the sandbox crate
// level so any future sub-sandbox can reuse them. Local
// (ECS-demo-only) types — `Acceleration`, `Player` — stay here.
use crate::sandbox::components::{Health, Position, Velocity};
use crate::sandbox::resources::TickCount;

use super::components::{Acceleration, Player};

/// **`Commands`** — seeds the demo entities deferred-style. Runs in
/// `Startup`; the entities are flushed in by the time `PreUpdate`
/// fires for the first time.
///
/// Entity roster (all movers carry `Acceleration` so `physics_step`
/// applies to them):
///
/// - `mover_a`, `mover_b`: `Position + Velocity + Acceleration + Health`
/// - `player`: `Position + Velocity + Acceleration + Health + Player`
/// - `statue`: `Position + Health` (no `Velocity`, no `Acceleration`)
///
/// The `statue` is what makes the physics join interesting — present
/// in the `Position` storage but skipped by `physics_step` because it
/// lacks `Velocity` / `Acceleration`.
pub(super) fn spawn_demo(mut commands: Commands) {
    // mover_a — gentle right-up drift, slight downward gravity-ish accel.
    commands
        .spawn()
        .insert(Position { x: 0.0, y: 0.0 })
        .insert(Velocity { x: 1.0, y: 0.5 })
        .insert(Acceleration { x: 0.0, y: -0.1 })
        .insert(Health(100));
    // mover_b — left-up drift, no acceleration (zero accel still
    // exercises the arity-3 path; physics_step reads the zeros).
    commands
        .spawn()
        .insert(Position { x: 10.0, y: -5.0 })
        .insert(Velocity { x: -0.5, y: 1.0 })
        .insert(Acceleration { x: 0.0, y: 0.0 })
        .insert(Health(75));
    // player — fast-rightward with mild thrust; regenerates HP.
    commands
        .spawn()
        .insert(Position { x: 0.0, y: 0.0 })
        .insert(Velocity { x: 2.0, y: 0.0 })
        .insert(Acceleration { x: 0.1, y: 0.0 })
        .insert(Health(200))
        .insert(Player);
    // statue — Position only (plus Health). Stays put by construction.
    commands
        .spawn()
        .insert(Position { x: 50.0, y: 50.0 })
        .insert(Health(50));
    info!("sandbox/ecs: spawn_demo queued 4 entities (Commands)");
}

/// **`Res<T>` + `Query<&T>`** — read a singleton resource and count
/// entities matching a single-component shape. Runs in `PreUpdate`
/// so it sees the entities [`spawn_demo`] queued during `Startup`
/// (they flush at the stage boundary before `PreUpdate` fires).
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
        "sandbox/ecs: initial state — Res<TickCount> + Query<&Position>"
    );
}

/// **`Query<(&mut Position, &mut Velocity, &Acceleration)>`** —
/// arity-3 multi-mut, the headline new shape from the multi-mut PR.
/// Symplectic Euler: update velocity from acceleration, then position
/// from the *updated* velocity, in a single pass.
///
/// Joins on entities that have all three storages. The `statue`
/// (Position + Health only) is skipped. Demonstrates that the engine
/// can hand out `&mut Position` and `&mut Velocity` to the same
/// closure body without aliasing, thanks to the `DenseMut` view +
/// query-construction self-conflict check.
pub(super) fn physics_step(mut q: Query<(&mut Position, &mut Velocity, &Acceleration)>) {
    let mut stepped = 0;
    for (pos, vel, acc) in q.iter_mut() {
        // velocity += acceleration  (Euler symplectic — order matters)
        vel.x += acc.x;
        vel.y += acc.y;
        // position += updated velocity
        pos.x += vel.x;
        pos.y += vel.y;
        stepped += 1;
    }
    debug!(
        stepped,
        "sandbox/ecs: physics_step — Query<(&mut P, &mut V, &A)>"
    );
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
        decayed, "sandbox/ecs: decay_health — ResMut<TickCount> + Query<&mut Health>"
    );
}

/// **`Query<(&Player, &mut Health)>`** — arity-2 **mut-not-first**,
/// new with the multi-mut PR. `Player` drives iteration (read),
/// `Health` is fetched per entity via the `DenseMut` random-access
/// view (write). Before this PR the workaround was to swap the
/// elements (`Query<(&mut Health, &Player)>`) so the mut came first;
/// now both orderings work and the natural one — "for each player,
/// mutate its health" — reads as it sounds.
///
/// Runs after [`decay_health`] so the player's per-tick net change
/// (-5 from decay, +2 from regen, net -3) shows up clearly in the
/// debug logs.
pub(super) fn player_regen(mut q: Query<(&Player, &mut Health)>) {
    for (_player, hp) in q.iter_mut() {
        hp.0 = hp.0.saturating_add(2);
        debug!(
            hp = hp.0,
            "sandbox/ecs: player_regen — Query<(&Player, &mut Health)>"
        );
    }
}

/// **`Query<(&Position, &Player)>`** — shared two-tuple join that
/// uses a zero-sized marker as a tuple element. Only the player-tagged
/// entity yields. This pattern *fetches* `Player` (you get `&Player` in
/// the item and ignore it); when you only need to *narrow* the set
/// without reading the marker, prefer the `With<Player>` filter — see
/// the [`super::filters`] demo for `With` / `Without` / `And` / `Or`.
pub(super) fn report_player_position(q: Query<(&Position, &Player)>) {
    for (pos, _player) in q.iter() {
        debug!(
            x = pos.x,
            y = pos.y,
            "sandbox/ecs: report_player_position — Query<(&Position, &Player)>"
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
        "sandbox/ecs: report_tick_summary — Res + 2× Query"
    );
}
