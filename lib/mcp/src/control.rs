//! `McpControl` resource + `apply_control` system.
//!
//! P1 is a no-op stub: the resource exists so future phases can drop in
//! state without touching plugin wiring, but nothing reads or writes
//! [`spark_common::Time`] yet. P3 fills in the real mapping
//! (`want_paused`/`pending_steps` → `Time::set_paused` /
//! `Time::set_scale`) once the `spark.control` tool ships.

use spark_ecs::{ResMut, Resource};

/// Agent-driven control state mirrored onto [`spark_common::Time`] each
/// frame.
///
/// In P3 [`apply_control`] is the single writer; the dispatcher mutates
/// this resource (via `tools/call spark.control`) and the system below
/// reflects it onto `Time` at the start of the next frame. Keeping the
/// staging here means the HTTP thread never touches `Time` directly,
/// and the per-frame transition is observable in one place.
#[derive(Resource, Debug, Default)]
pub struct McpControl {
    /// What the agent last asked for. Mirrored onto `Time::set_paused`
    /// each frame once P3 lands.
    pub want_paused: bool,
    /// Single-frame ticks the agent has scheduled while paused.
    /// Decremented each frame until zero, then `want_paused` re-applies.
    pub pending_steps: u32,
    /// Virtual-clock multiplier the agent last asked for. `1.0` means
    /// "leave `Time::set_scale` alone."
    pub scale: f32,
}

/// `Stage::PreUpdate` no-op stub.
///
/// P1 only proves the wiring (resource registration + workload slot);
/// P3 replaces this body with the real `Time` mapping. The signature
/// already takes `ResMut<McpControl>` so a P3 PR only edits the body.
#[allow(
    clippy::needless_pass_by_value,
    reason = "ResMut<T> is a SystemParam — IntoSystem hands it in by value, same shape as every other system in the engine."
)]
pub fn apply_control(_ctl: ResMut<McpControl>) {
    // P3: when `pending_steps > 0`, unpause Time for one tick and
    // decrement. Otherwise propagate `want_paused` and `scale` onto
    // `Time::set_paused` / `Time::set_scale`.
}
