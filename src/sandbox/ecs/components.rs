//! Component types **local to the ECS sub-sandbox**.
//!
//! Spatial / gameplay primitives (`Position`, `Velocity`, `Health`)
//! that other sub-sandboxes would also use live one level up at
//! [`crate::sandbox::components`]. This file holds only what is
//! specific to the ECS demo:
//!
//! - [`Acceleration`]: a physics input — if a future `physics`
//!   sub-sandbox lands, this likely moves up to the shared module.
//! - [`Player`]: a zero-sized marker tied to this demo's narrative.
//! - [`Building`] / [`Capacity`] + the [`Powered`] / [`Backup`] /
//!   [`Operational`] / [`UnderMaintenance`] markers: the "power-grid"
//!   roster [`super::filters`] uses to exercise `With` / `Without` /
//!   `And` / `Or`.
//!
//! Fields are `pub(super)` so the sibling [`super::systems`] and
//! [`super::filters`] modules can read / write them without going
//! through accessors.

use spark_ecs::Component;

/// 2D acceleration in units / tick². Drives
/// [`super::systems::physics_step`], which integrates both velocity
/// (from acceleration) and position (from velocity) in a single
/// `Query<(&mut Position, &mut Velocity, &Acceleration)>` walk — the
/// arity-3 multi-mut shape that ships with the multi-mut PR.
#[derive(Debug, Component)]
pub(super) struct Acceleration {
    pub(super) x: f32,
    pub(super) y: f32,
}

/// Marker for the player-controlled entity. Zero-sized — exists to
/// gate joins like `Query<(&Position, &Player)>` (log-the-player) and
/// `Query<(&Player, &mut Health)>` (player-only regen, demonstrating
/// the mut-not-first arity-2 shape that ships with the multi-mut PR).
#[derive(Component)]
pub(super) struct Player;

// ----- Filter-demo components -----
//
// A small "power-grid" roster, separate from the physics movers above,
// used by `super::filters` to exercise `With` / `Without` / `And` /
// `Or`. The four markers are zero-sized: a filter never *fetches* them,
// it only tests their presence (`With`) or absence (`Without`).

/// A power-grid building. `name` is purely for readable demo logs —
/// `super::filters` queries `&Building` and narrows the set with marker
/// filters, never touching the markers in the yielded item.
#[derive(Debug, Component)]
pub(super) struct Building {
    pub(super) name: &'static str,
}

/// Throughput capacity in MW. A *non-marker* data component, so the
/// filter demo can show a filter narrowing a multi-component shape
/// (`Query<(&Building, &Capacity), With<Powered>>`) and a filtered
/// mutation (`Query<(&Building, &mut Capacity), With<Powered>>`).
#[derive(Debug, Component)]
pub(super) struct Capacity(pub(super) u32);

/// Marker: the building draws grid power.
#[derive(Component)]
pub(super) struct Powered;

/// Marker: the building has an on-site backup generator.
#[derive(Component)]
pub(super) struct Backup;

/// Marker: the building is currently running.
#[derive(Component)]
pub(super) struct Operational;

/// Marker: the building is down for repairs.
#[derive(Component)]
pub(super) struct UnderMaintenance;
