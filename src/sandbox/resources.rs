//! Resources shared by every sub-sandbox under `crate::sandbox`.
//!
//! Added once by [`super::SandboxPlugin`]; sub-sandboxes consume
//! them via `Res<T>` / `ResMut<T>`. Sub-sandbox plugins must **not**
//! re-add these — a second `add_resource` overwrites, which would
//! reset the counter mid-run if the order ever flipped.

use spark_ecs::Resource;

/// Per-tick counter. Bumped once per `UPDATE` pass by the ECS
/// sub-sandbox's `decay_health` system; read by every "report"
/// system that wants to print a tick number.
#[derive(Resource)]
pub(crate) struct TickCount(pub(crate) u32);
