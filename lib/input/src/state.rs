//! The queryable input state — [`KeyboardState`] and [`MouseState`].
//!
//! These are the resources gameplay reads. They're *built* from the raw events
//! by the `collect_*` systems (see [`crate::collect`]); the fields are
//! `pub(crate)` so those systems can mutate them, while the public surface is
//! read-only accessors. Held sets and per-frame edge sets are `Vec`s, not
//! `HashSet`s — lookups go through `is_pressed` / `just_*`, and nothing iterates
//! a hashed collection, so iteration order can never make a simulation
//! non-deterministic.

use spark_ecs::Resource;

use crate::event::{KeyCode, MouseButton};

/// Which keys are held, plus this frame's press/release *edges*.
///
/// "Held" persists across frames until a release arrives; the edge sets
/// (`just_pressed` / `just_released`) describe **only the current frame** and
/// are cleared at the top of every `collect_keyboard` run. That split is the
/// whole point: `is_pressed` answers "is W down right now?" (movement), while
/// `just_pressed` answers "did W go down *this* frame?" (a one-shot action like
/// jumping), without each caller tracking last-frame state itself.
///
/// # Examples
///
/// ```
/// use spark_input::{KeyboardState, KeyCode};
///
/// // A freshly built state holds nothing. The engine populates it each frame
/// // from forwarded events; gameplay reads it through `Res<KeyboardState>`.
/// let keys = KeyboardState::default();
/// assert!(!keys.is_pressed(KeyCode::KeyW));
/// assert!(!keys.just_pressed(KeyCode::Space));
/// ```
#[derive(Resource, Default, Debug)]
pub struct KeyboardState {
    /// Keys currently held. Persists across frames; insertion order, no dupes.
    pub(crate) pressed: Vec<KeyCode>,
    /// Keys that went down this frame. Cleared at the top of `collect_keyboard`.
    pub(crate) just_pressed: Vec<KeyCode>,
    /// Keys that went up this frame. Cleared at the top of `collect_keyboard`.
    pub(crate) just_released: Vec<KeyCode>,
}

impl KeyboardState {
    /// Whether `key` is currently held.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::{KeyboardState, KeyCode};
    /// assert!(!KeyboardState::default().is_pressed(KeyCode::Space));
    /// ```
    #[must_use]
    pub fn is_pressed(&self, key: KeyCode) -> bool {
        self.pressed.contains(&key)
    }

    /// Whether `key` went down *this frame* (true for exactly the frame the
    /// press arrived).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::{KeyboardState, KeyCode};
    /// assert!(!KeyboardState::default().just_pressed(KeyCode::Space));
    /// ```
    #[must_use]
    pub fn just_pressed(&self, key: KeyCode) -> bool {
        self.just_pressed.contains(&key)
    }

    /// Whether `key` went up *this frame*.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::{KeyboardState, KeyCode};
    /// assert!(!KeyboardState::default().just_released(KeyCode::Space));
    /// ```
    #[must_use]
    pub fn just_released(&self, key: KeyCode) -> bool {
        self.just_released.contains(&key)
    }

    /// Iterates the currently-held keys.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::KeyboardState;
    /// assert_eq!(KeyboardState::default().pressed().count(), 0);
    /// ```
    pub fn pressed(&self) -> impl Iterator<Item = KeyCode> + '_ {
        self.pressed.iter().copied()
    }
}

/// Mouse button state (held + per-frame edges), cursor position, and this
/// frame's scroll delta.
///
/// Buttons follow the same held/edge model as [`KeyboardState`]. `position` is
/// absolute (window pixels, top-left origin) and persists across frames;
/// `scroll` is a *per-frame delta* that resets to `(0, 0)` every frame, so it's
/// non-zero only on frames the wheel actually moved.
///
/// # Examples
///
/// ```
/// use spark_input::{MouseState, MouseButton};
///
/// let mouse = MouseState::default();
/// assert!(!mouse.is_pressed(MouseButton::Left));
/// assert_eq!(mouse.position(), (0.0, 0.0));
/// assert_eq!(mouse.scroll(), (0.0, 0.0));
/// ```
#[derive(Resource, Default, Debug)]
pub struct MouseState {
    /// Buttons currently held. Persists across frames; insertion order, no dupes.
    pub(crate) buttons: Vec<MouseButton>,
    /// Buttons that went down this frame. Cleared at the top of `collect_mouse_buttons`.
    pub(crate) buttons_just_pressed: Vec<MouseButton>,
    /// Buttons that went up this frame. Cleared at the top of `collect_mouse_buttons`.
    pub(crate) buttons_just_released: Vec<MouseButton>,
    /// Last known cursor position, window pixels, top-left origin.
    pub(crate) position: (f32, f32),
    /// This frame's accumulated scroll delta (pixels). Reset each frame.
    pub(crate) scroll: (f32, f32),
}

impl MouseState {
    /// Whether `button` is currently held.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::{MouseState, MouseButton};
    /// assert!(!MouseState::default().is_pressed(MouseButton::Left));
    /// ```
    #[must_use]
    pub fn is_pressed(&self, button: MouseButton) -> bool {
        self.buttons.contains(&button)
    }

    /// Whether `button` went down *this frame*.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::{MouseState, MouseButton};
    /// assert!(!MouseState::default().just_pressed(MouseButton::Right));
    /// ```
    #[must_use]
    pub fn just_pressed(&self, button: MouseButton) -> bool {
        self.buttons_just_pressed.contains(&button)
    }

    /// Whether `button` went up *this frame*.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::{MouseState, MouseButton};
    /// assert!(!MouseState::default().just_released(MouseButton::Right));
    /// ```
    #[must_use]
    pub fn just_released(&self, button: MouseButton) -> bool {
        self.buttons_just_released.contains(&button)
    }

    /// Iterates the currently-held buttons. Mirrors
    /// [`KeyboardState::pressed`](crate::KeyboardState::pressed).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::MouseState;
    /// assert_eq!(MouseState::default().buttons().count(), 0);
    /// ```
    pub fn buttons(&self) -> impl Iterator<Item = MouseButton> + '_ {
        self.buttons.iter().copied()
    }

    /// Cursor position in window pixels, top-left origin.
    ///
    /// # Future change
    ///
    /// Returns `(f32, f32)` today; this becomes `Vec2` when the math module
    /// lands. Call sites should expect a breaking signature change then.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::MouseState;
    /// assert_eq!(MouseState::default().position(), (0.0, 0.0));
    /// ```
    #[must_use]
    pub fn position(&self) -> (f32, f32) {
        self.position
    }

    /// This frame's scroll delta, in pixels, resetting to `(0, 0)` each frame.
    ///
    /// Line-based wheels are normalized to pixels by the window layer (see
    /// `spark_window`'s `SCROLL_LINES_TO_PIXELS`), so both delta sources report
    /// a comparable magnitude.
    ///
    /// # Future change
    ///
    /// Same `Vec2` migration as [`position`](Self::position).
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_input::MouseState;
    /// assert_eq!(MouseState::default().scroll(), (0.0, 0.0));
    /// ```
    #[must_use]
    pub fn scroll(&self) -> (f32, f32) {
        self.scroll
    }
}
