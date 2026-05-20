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
//! All four are plain `'static` types — no `#[derive(Component)]`
//! yet; `lib/ecs/src/storage.rs` carries the blanket
//! `impl<T: 'static> Component for T` until the derive PR lands.

/// 2D position. Named-field struct so log output reads naturally.
#[derive(Debug)]
pub(crate) struct Position {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// 2D velocity in units / tick.
#[derive(Debug)]
pub(crate) struct Velocity {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

/// Hit points. `saturating_sub` / `saturating_add` are used at decay
/// and regen sites so the value never wraps past zero or overflows.
#[derive(Debug)]
pub(crate) struct Health(pub(crate) u32);
