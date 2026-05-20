//! Components shared by every sub-sandbox under `crate::sandbox`.
//!
//! These are generic spatial / gameplay primitives — `Position`,
//! `Velocity`, `Health`. Each sub-sandbox is free to import them
//! (`use crate::sandbox::components::Position;`) and pair them with
//! its own local components. Demo-specific markers (`Player`) and
//! physics-specific fields (`Acceleration`) live in the relevant
//! sub-sandbox instead — they don't generalise.
//!
//! `pub(crate)` scope: visible anywhere in the binary, which keeps
//! sub-sandbox imports short. The cost of broader visibility is
//! negligible — none of this leaves the binary.
//!
//! Each opts into the ECS with `#[derive(Component)]` — the explicit
//! marker that lets `World::insert` and `Query` accept it. The trait's
//! `Send + Sync + 'static` bound is satisfied trivially by these plain
//! data structs.

use spark_ecs::Component;

/// 2D position. Named-field struct so log output reads naturally.
#[derive(Debug, Component)]
pub(crate) struct Position {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// 2D velocity in units / tick.
#[derive(Debug, Component)]
pub(crate) struct Velocity {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// Hit points. `saturating_sub` / `saturating_add` are used at decay
/// and regen sites so the value never wraps past zero or overflows.
#[derive(Debug, Component)]
pub(crate) struct Health(pub(crate) u32);
