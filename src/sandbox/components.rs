//! Component types for the sandbox demo.
//!
//! All four are plain `'static` types — no `#[derive(Component)]` yet;
//! `lib/ecs/src/storage.rs` carries the blanket `impl<T: 'static>
//! Component for T` until the derive PR lands. Fields are
//! `pub(super)` so the sibling [`super::systems`] module can read /
//! write them without going through accessors.

/// 2D position. Named-field struct so log output reads naturally.
#[derive(Debug)]
pub(super) struct Position {
    pub(super) x: f32,
    pub(super) y: f32,
}

/// 2D velocity in units / tick.
#[derive(Debug)]
pub(super) struct Velocity {
    pub(super) x: f32,
    pub(super) y: f32,
}

/// Hit points. `saturating_sub` is used at the decay site so the
/// value never wraps past zero.
#[derive(Debug)]
pub(super) struct Health(pub(super) u32);

/// Marker for the player-controlled entity. Zero-sized — exists only
/// to gate `Query<(&Position, &Player)>` to the player entity.
pub(super) struct Player;
