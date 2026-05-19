//! Resource types for the sandbox demo.
//!
//! Just one singleton — [`TickCount`]. Bumped once per `UPDATE` pass
//! by [`super::systems::decay_health`].

/// Tick counter. Read via `Res<TickCount>` and written via
/// `ResMut<TickCount>`.
pub(super) struct TickCount(pub(super) u32);
