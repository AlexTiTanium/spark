//! Translating `winit` OS input into [`spark_input`] events and forwarding
//! them into the world.
//!
//! This is the whole of `spark-window`'s input responsibility, kept out of
//! [`event_loop`](crate::event_loop) so that file stays about the loop. Each
//! `forward_*` function is called from one `WindowEvent` arm: it logs the raw
//! event and, when it maps to something `spark-input` models, sends the
//! corresponding event via [`try_send`].
//!
//! Forwarding is **guarded** — [`try_send`] is a no-op unless a consumer
//! registered the event buffer. So the window depends on `spark-input` only to
//! *name* the event types here; `spark-input` never depends back on the window.

use spark_core::Application;
use spark_ecs::{Event, Events};
use spark_input::{
    CursorMoved, FocusLost, KeyCode, KeyboardInput, MouseButton, MouseButtonInput, MouseWheel,
};
use tracing::{debug, trace};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton as WinitMouseButton, MouseScrollDelta};
use winit::keyboard::{KeyCode as WinitKeyCode, PhysicalKey};

/// Pixels of scroll per `winit` `LineDelta` notch.
///
/// winit reports wheel movement two ways: `LineDelta` (whole notches, most
/// desktop mice) and `PixelDelta` (already pixels, precision trackpads). We
/// scale the former so both report the same magnitude for one notch. ~50
/// px/line matches common GUI toolkits (GTK, Qt) — a chosen convention, not a
/// hardware fact.
const SCROLL_LINES_TO_PIXELS: f32 = 50.0;

/// Logs and forwards a keyboard press/release.
///
/// Drops OS auto-repeat (held-state tracks edges, not repeats) and keys outside
/// the curated [`KeyCode`] set.
pub(crate) fn forward_keyboard(app: &Application, event: &KeyEvent) {
    debug!(state = ?event.state, key = ?event.logical_key, "keyboard input");
    if !event.repeat
        && let PhysicalKey::Code(code) = event.physical_key
        && let Some(key) = map_key(code)
    {
        try_send(
            app,
            KeyboardInput {
                key,
                pressed: event.state.is_pressed(),
            },
        );
    }
}

/// Logs and forwards a mouse button press/release.
pub(crate) fn forward_mouse_button(
    app: &Application,
    state: ElementState,
    button: WinitMouseButton,
) {
    debug!(?state, ?button, "mouse input");
    try_send(
        app,
        MouseButtonInput {
            button: map_button(button),
            pressed: state.is_pressed(),
        },
    );
}

/// Logs and forwards an absolute cursor move (window pixels, top-left origin).
pub(crate) fn forward_cursor(app: &Application, position: PhysicalPosition<f64>) {
    trace!(x = position.x, y = position.y, "cursor moved");
    try_send(
        app,
        CursorMoved {
            x: px(position.x),
            y: px(position.y),
        },
    );
}

/// Logs and forwards a scroll-wheel delta, normalized to pixels so line-based
/// and pixel-based devices report comparable magnitudes.
pub(crate) fn forward_wheel(app: &Application, delta: MouseScrollDelta) {
    let (x, y) = match delta {
        MouseScrollDelta::LineDelta(x, y) => {
            (x * SCROLL_LINES_TO_PIXELS, y * SCROLL_LINES_TO_PIXELS)
        }
        MouseScrollDelta::PixelDelta(p) => (px(p.x), px(p.y)),
    };
    trace!(x, y, "mouse wheel");
    try_send(app, MouseWheel { x, y });
}

/// Forwards a focus-loss signal so consumers can clear held state (the OS
/// delivers the matching key-ups to whichever window took focus, not us).
pub(crate) fn forward_focus_lost(app: &Application) {
    try_send(app, FocusLost);
}

/// Sends `event` into its [`Events<E>`] buffer — but only if some plugin has
/// registered that buffer with `add_event::<E>()` (normally `InputPlugin`).
///
/// The guard lets the window forward input without assuming a consumer is
/// wired: with no registered buffer the send is a silent no-op.
fn try_send<E: Event>(app: &Application, event: E) {
    if let Some(mut events) = app.world().get_resource_mut::<Events<E>>() {
        events.send(event);
    }
}

/// Narrows a `winit` `f64` screen coordinate to the `f32` Spark stores.
///
/// Converting a pixel coordinate from `f64` to `f32` loses far less than one
/// pixel of precision, so the truncation is intended and harmless.
#[allow(
    clippy::cast_possible_truncation,
    reason = "f64 pixel coordinates fit f32 with sub-pixel error"
)]
fn px(v: f64) -> f32 {
    v as f32
}

/// Translates a `winit` physical [`KeyCode`](WinitKeyCode) into Spark's
/// [`KeyCode`], or `None` for keys outside Spark's curated set (dropped, no
/// event emitted).
///
/// Kept deliberately 1:1 with [`KeyCode`]: to support a new key, add the
/// variant there and a matching arm here.
fn map_key(code: WinitKeyCode) -> Option<KeyCode> {
    use WinitKeyCode as W;
    Some(match code {
        W::KeyA => KeyCode::KeyA,
        W::KeyB => KeyCode::KeyB,
        W::KeyC => KeyCode::KeyC,
        W::KeyD => KeyCode::KeyD,
        W::KeyE => KeyCode::KeyE,
        W::KeyF => KeyCode::KeyF,
        W::KeyG => KeyCode::KeyG,
        W::KeyH => KeyCode::KeyH,
        W::KeyI => KeyCode::KeyI,
        W::KeyJ => KeyCode::KeyJ,
        W::KeyK => KeyCode::KeyK,
        W::KeyL => KeyCode::KeyL,
        W::KeyM => KeyCode::KeyM,
        W::KeyN => KeyCode::KeyN,
        W::KeyO => KeyCode::KeyO,
        W::KeyP => KeyCode::KeyP,
        W::KeyQ => KeyCode::KeyQ,
        W::KeyR => KeyCode::KeyR,
        W::KeyS => KeyCode::KeyS,
        W::KeyT => KeyCode::KeyT,
        W::KeyU => KeyCode::KeyU,
        W::KeyV => KeyCode::KeyV,
        W::KeyW => KeyCode::KeyW,
        W::KeyX => KeyCode::KeyX,
        W::KeyY => KeyCode::KeyY,
        W::KeyZ => KeyCode::KeyZ,
        W::Digit0 => KeyCode::Digit0,
        W::Digit1 => KeyCode::Digit1,
        W::Digit2 => KeyCode::Digit2,
        W::Digit3 => KeyCode::Digit3,
        W::Digit4 => KeyCode::Digit4,
        W::Digit5 => KeyCode::Digit5,
        W::Digit6 => KeyCode::Digit6,
        W::Digit7 => KeyCode::Digit7,
        W::Digit8 => KeyCode::Digit8,
        W::Digit9 => KeyCode::Digit9,
        W::ArrowUp => KeyCode::ArrowUp,
        W::ArrowDown => KeyCode::ArrowDown,
        W::ArrowLeft => KeyCode::ArrowLeft,
        W::ArrowRight => KeyCode::ArrowRight,
        W::Space => KeyCode::Space,
        W::Enter => KeyCode::Enter,
        W::Escape => KeyCode::Escape,
        W::Tab => KeyCode::Tab,
        W::Backspace => KeyCode::Backspace,
        W::Delete => KeyCode::Delete,
        W::Home => KeyCode::Home,
        W::End => KeyCode::End,
        W::PageUp => KeyCode::PageUp,
        W::PageDown => KeyCode::PageDown,
        W::Insert => KeyCode::Insert,
        W::ShiftLeft => KeyCode::ShiftLeft,
        W::ShiftRight => KeyCode::ShiftRight,
        W::ControlLeft => KeyCode::ControlLeft,
        W::ControlRight => KeyCode::ControlRight,
        W::AltLeft => KeyCode::AltLeft,
        W::AltRight => KeyCode::AltRight,
        W::SuperLeft => KeyCode::SuperLeft,
        W::SuperRight => KeyCode::SuperRight,
        W::F1 => KeyCode::F1,
        W::F2 => KeyCode::F2,
        W::F3 => KeyCode::F3,
        W::F4 => KeyCode::F4,
        W::F5 => KeyCode::F5,
        W::F6 => KeyCode::F6,
        W::F7 => KeyCode::F7,
        W::F8 => KeyCode::F8,
        W::F9 => KeyCode::F9,
        W::F10 => KeyCode::F10,
        W::F11 => KeyCode::F11,
        W::F12 => KeyCode::F12,
        _ => return None,
    })
}

/// Translates a `winit` mouse button into Spark's [`MouseButton`]. Total —
/// every `winit` variant has a Spark counterpart.
fn map_button(button: WinitMouseButton) -> MouseButton {
    match button {
        WinitMouseButton::Left => MouseButton::Left,
        WinitMouseButton::Right => MouseButton::Right,
        WinitMouseButton::Middle => MouseButton::Middle,
        WinitMouseButton::Back => MouseButton::Back,
        WinitMouseButton::Forward => MouseButton::Forward,
        WinitMouseButton::Other(n) => MouseButton::Other(n),
    }
}

#[cfg(test)]
mod tests {
    use super::{map_button, map_key, px};
    use spark_input::{KeyCode, MouseButton};
    use winit::event::MouseButton as WinitMouseButton;
    use winit::keyboard::KeyCode as WinitKeyCode;

    #[test]
    fn map_key_translates_keys_in_the_subset() {
        assert_eq!(map_key(WinitKeyCode::KeyW), Some(KeyCode::KeyW));
        assert_eq!(map_key(WinitKeyCode::Space), Some(KeyCode::Space));
        assert_eq!(map_key(WinitKeyCode::ArrowUp), Some(KeyCode::ArrowUp));
        assert_eq!(map_key(WinitKeyCode::F12), Some(KeyCode::F12));
    }

    #[test]
    fn map_key_drops_keys_outside_the_subset() {
        // Real winit keys with no Spark counterpart — these are dropped, not
        // mapped to some catch-all.
        assert_eq!(map_key(WinitKeyCode::F13), None);
        assert_eq!(map_key(WinitKeyCode::NumLock), None);
    }

    #[test]
    fn map_button_covers_every_variant() {
        assert_eq!(map_button(WinitMouseButton::Left), MouseButton::Left);
        assert_eq!(map_button(WinitMouseButton::Right), MouseButton::Right);
        assert_eq!(map_button(WinitMouseButton::Middle), MouseButton::Middle);
        assert_eq!(map_button(WinitMouseButton::Back), MouseButton::Back);
        assert_eq!(map_button(WinitMouseButton::Forward), MouseButton::Forward);
        assert_eq!(
            map_button(WinitMouseButton::Other(7)),
            MouseButton::Other(7)
        );
    }

    #[test]
    fn px_narrows_f64_to_f32() {
        // Exactly-representable values; compare bit patterns to keep the check
        // exact (and clear of clippy's float-equality lint).
        assert_eq!(px(1280.0).to_bits(), 1280.0_f32.to_bits());
        assert_eq!(px(0.5).to_bits(), 0.5_f32.to_bits());
    }
}
