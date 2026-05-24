//! Input sub-sandbox — proves the window → `spark-input` seam end-to-end.
//!
//! [`InputSandboxPlugin`] registers a single `Stage::Update` system that reads
//! [`KeyboardState`] / [`MouseState`] and logs what it sees, but only when
//! something is actually happening (so it isn't per-frame noise). Run with
//! `RUST_LOG=spark=debug cargo run -p spark`, then type, click, scroll, and
//! Alt-Tab away and back — the log lines should track it, and no key should
//! stay "held" across a focus change.

use spark_core::{Application, Plugin, Stage};
use spark_ecs::Res;
use spark_input::{KeyCode, KeyboardState, MouseButton, MouseState};
use spark_log::debug;

/// Logs live keyboard/mouse state — the human-visible proof that input is
/// flowing from the OS through `spark-window` into `spark-input`'s resources.
fn report_input(keys: Res<KeyboardState>, mouse: Res<MouseState>) {
    let held: Vec<KeyCode> = keys.pressed().collect();
    if !held.is_empty() {
        debug!(?held, cursor = ?mouse.position(), "sandbox/input: keys held");
    }
    // `just_pressed` is a one-frame edge — demonstrates one-shot detection.
    if keys.just_pressed(KeyCode::Space) {
        debug!("sandbox/input: space tapped (just_pressed)");
    }
    let scroll = mouse.scroll();
    if scroll != (0.0, 0.0) {
        debug!(?scroll, "sandbox/input: scrolled");
    }
    if mouse.is_pressed(MouseButton::Left) {
        debug!(cursor = ?mouse.position(), "sandbox/input: left button held");
    }
}

/// Wires the input demo into an [`Application`]. Assumes `InputPlugin` (engine)
/// is registered separately in `main.rs` — this sub-sandbox only *reads* the
/// state it produces.
pub struct InputSandboxPlugin;

impl Plugin for InputSandboxPlugin {
    fn build(&self, app: &mut Application) {
        app.add_system(Stage::Update, report_input);
    }
}
