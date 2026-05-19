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
//!
//! Fields are `pub(super)` so the sibling [`super::systems`] module
//! can read / write them without going through accessors.

/// 2D acceleration in units / tick². Drives
/// [`super::systems::physics_step`], which integrates both velocity
/// (from acceleration) and position (from velocity) in a single
/// `Query<(&mut Position, &mut Velocity, &Acceleration)>` walk — the
/// arity-3 multi-mut shape that ships with the multi-mut PR.
#[derive(Debug)]
pub(super) struct Acceleration {
    pub(super) x: f32,
    pub(super) y: f32,
}

/// Marker for the player-controlled entity. Zero-sized — exists to
/// gate joins like `Query<(&Position, &Player)>` (log-the-player) and
/// `Query<(&Player, &mut Health)>` (player-only regen, demonstrating
/// the mut-not-first arity-2 shape that ships with the multi-mut PR).
pub(super) struct Player;
