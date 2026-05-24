//! The raw input events and the key/button types they carry.
//!
//! These are the low-level events `spark-input` exposes. The window layer
//! (`spark-window`, which sits *above* this crate) translates `winit`'s OS
//! events into them and forwards them into the world; this crate's `collect_*`
//! systems then turn them into [`KeyboardState`](crate::KeyboardState) /
//! [`MouseState`](crate::MouseState).
//!
//! The key and button enums are hand-rolled rather than taken from `winit`, so
//! no windowing type appears in a public signature — `spark-input` depends on
//! neither `winit` nor `spark-window`.

use spark_ecs::Event;

/// A physical keyboard key, identified by position rather than the character
/// it produces — so `KeyCode::KeyW` is the same physical key on QWERTY and
/// AZERTY, which is what game movement bindings want.
///
/// This is a **curated subset** sized for indie-game scope (letters, digits,
/// arrows, common editing/navigation keys, modifiers, and the function row),
/// not the full set `winit` exposes.
///
/// # Extending this enum
///
/// To add a key:
/// 1. Add a variant here, keeping the name identical to `winit`'s
///    `KeyCode` variant (so the mapping stays an obvious 1:1).
/// 2. Add the matching arm in `spark_window`'s private `map_key` translation.
///
/// `winit` keys with no variant here are silently skipped at the window
/// boundary (no event is emitted). The enum deliberately does **not** use
/// `#[non_exhaustive]`, so `match` over a `KeyCode` in gameplay code stays
/// exhaustive and the compiler flags a missing arm when you add a variant.
///
/// # Examples
///
/// ```
/// use spark_input::KeyCode;
///
/// let movement = [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD];
/// assert!(movement.contains(&KeyCode::KeyW));
/// assert_ne!(KeyCode::Space, KeyCode::Enter);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyCode {
    // Letters.
    KeyA,
    KeyB,
    KeyC,
    KeyD,
    KeyE,
    KeyF,
    KeyG,
    KeyH,
    KeyI,
    KeyJ,
    KeyK,
    KeyL,
    KeyM,
    KeyN,
    KeyO,
    KeyP,
    KeyQ,
    KeyR,
    KeyS,
    KeyT,
    KeyU,
    KeyV,
    KeyW,
    KeyX,
    KeyY,
    KeyZ,
    // Digit row.
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    // Arrows.
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    // Whitespace / editing.
    Space,
    Enter,
    Escape,
    Tab,
    Backspace,
    Delete,
    // Navigation cluster.
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    // Modifiers (left/right distinguished, matching `winit`).
    ShiftLeft,
    ShiftRight,
    ControlLeft,
    ControlRight,
    AltLeft,
    AltRight,
    SuperLeft,
    SuperRight,
    // Function row.
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

/// A mouse button. Mirrors `winit`'s set; `Other(u16)` carries extra buttons
/// (thumb buttons beyond back/forward, etc.) by raw id.
///
/// # Examples
///
/// ```
/// use spark_input::MouseButton;
///
/// assert_eq!(MouseButton::Left, MouseButton::Left);
/// assert_ne!(MouseButton::Other(9), MouseButton::Other(10));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
    Other(u16),
}

/// A key going down or up. `pressed` is `true` for a press, `false` for a
/// release. Auto-repeat is filtered out at the window boundary, so a held key
/// produces exactly one `pressed: true`.
///
/// # Examples
///
/// ```
/// use spark_input::{KeyboardInput, KeyCode};
///
/// let down = KeyboardInput { key: KeyCode::Space, pressed: true };
/// assert!(down.pressed);
/// ```
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardInput {
    /// The physical key that changed.
    pub key: KeyCode,
    /// `true` if pressed this event, `false` if released.
    pub pressed: bool,
}

/// A mouse button going down or up. `pressed` follows the same convention as
/// [`KeyboardInput`].
///
/// # Examples
///
/// ```
/// use spark_input::{MouseButtonInput, MouseButton};
///
/// let click = MouseButtonInput { button: MouseButton::Left, pressed: true };
/// assert!(click.pressed);
/// ```
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MouseButtonInput {
    /// The button that changed.
    pub button: MouseButton,
    /// `true` if pressed this event, `false` if released.
    pub pressed: bool,
}

/// The cursor moved to an absolute position, in window pixels with the origin
/// at the top-left corner.
///
/// # Examples
///
/// ```
/// use spark_input::CursorMoved;
///
/// let m = CursorMoved { x: 12.0, y: 8.0 };
/// assert_eq!((m.x, m.y), (12.0, 8.0));
/// ```
#[derive(Event, Debug, Clone, Copy, PartialEq)]
pub struct CursorMoved {
    /// Horizontal position in window pixels from the left edge.
    pub x: f32,
    /// Vertical position in window pixels from the top edge.
    pub y: f32,
}

/// A scroll-wheel movement, as a delta already normalized to pixel units by
/// the window layer (so line-based and pixel-based devices agree).
///
/// # Examples
///
/// ```
/// use spark_input::MouseWheel;
///
/// let w = MouseWheel { x: 0.0, y: 50.0 };
/// assert_eq!(w.y, 50.0);
/// ```
#[derive(Event, Debug, Clone, Copy, PartialEq)]
pub struct MouseWheel {
    /// Horizontal scroll delta in pixels.
    pub x: f32,
    /// Vertical scroll delta in pixels.
    pub y: f32,
}

/// The window lost OS focus. The `collect_*` systems treat every held
/// key/button as released on this signal: the OS delivers the matching key-up
/// events to whichever window took focus, not to us, so our "held" state would
/// otherwise go stale and a key could appear stuck after Alt-Tab.
///
/// There is no `FocusGained` counterpart — regaining focus can't restore
/// physical key positions, so it isn't actionable.
///
/// # Examples
///
/// ```
/// use spark_input::FocusLost;
///
/// let _ = FocusLost; // zero-sized "it happened" signal
/// ```
#[derive(Event, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusLost;
