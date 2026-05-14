//! Configuration for the application window.
//!
//! # Summary
//!
//! [`WindowConfig`] is the plain-data input to [`crate::run`]: a title, an
//! initial size in logical pixels, and a `resizable` flag. It exposes a
//! small builder API so call sites read as a sentence
//! (`WindowConfig::default().with_title("Spark").with_size(1280, 720)`).
//!
//! # Logic
//!
//! The struct is `#[non_exhaustive]` so future fields (icon, decorations,
//! transparency, …) can be added without a breaking change. `Default`
//! returns sensible values for a 720p desktop window. Each builder method
//! takes `self` by value and returns `Self`, which lets calls chain.
//!
//! # Memory layout
//!
//! ```text
//! WindowConfig {
//!     title:     String,         // heap-allocated, owned
//!     size:      (u32, u32),     // (width, height) in logical pixels
//!     resizable: bool,           // user can drag corners?
//! }
//! ```
//!
//! # Why it works
//!
//! The config is a value type with no invariants beyond the field types
//! themselves: any non-empty `String`, any `(u32, u32)`, any `bool` is
//! valid. Validation (e.g. minimum size) happens inside
//! [`crate::run`] when the values reach `winit`.
//!
//! # How to use
//!
//! ```
//! let cfg = spark_window::WindowConfig::default()
//!     .with_title("Spark")
//!     .with_size(1280, 720)
//!     .with_resizable(true);
//! assert_eq!(cfg.title, "Spark");
//! assert_eq!(cfg.size, (1280, 720));
//! assert!(cfg.resizable);
//! ```
//!
//! # How NOT to use
//!
//! - Do not construct `WindowConfig` with a struct literal — it is
//!   `#[non_exhaustive]` from outside the crate. Use [`WindowConfig::default`]
//!   and the `with_*` builders.
//! - Do not pass `(0, 0)` for `size`; `winit` will reject it. The crate
//!   does not currently clamp the value.

/// Configuration for the application window opened by [`crate::run`].
///
/// # Logic
///
/// Holds the title shown in the OS title bar, the initial inner size in
/// logical pixels, and whether the user can resize the window by dragging
/// its borders. Constructed via [`WindowConfig::default`] and refined with
/// the chainable `with_*` builders.
///
/// # Memory layout
///
/// ```text
/// title:     String      // owned, heap-allocated
/// size:      (u32, u32)  // (width, height), logical pixels
/// resizable: bool        // OS allows resize gestures
/// ```
///
/// # Why it works
///
/// `#[non_exhaustive]` forbids external struct-literal construction, so
/// adding fields later (icon, decorations, fullscreen, …) is non-breaking.
/// All current fields are independent — there is no invariant that links
/// them — so the builder methods can be applied in any order.
///
/// # How to use
///
/// ```
/// let cfg = spark_window::WindowConfig::default()
///     .with_title("Hello")
///     .with_size(800, 600);
/// assert_eq!(cfg.title, "Hello");
/// ```
///
/// # How NOT to use
///
/// - Do not mutate the fields in place via `&mut` from another crate — the
///   fields are `pub` for read access (and `Debug` ergonomics) but the
///   intended write path is the builder.
/// - Do not pass an empty string as the title on every platform; some
///   compositors (notably Wayland) treat it specially. Use a meaningful
///   value.
///
/// # Examples
///
/// ```
/// use spark_window::WindowConfig;
/// let cfg = WindowConfig::default();
/// assert_eq!(cfg.title, "Spark");
/// assert_eq!(cfg.size, (1280, 720));
/// assert!(cfg.resizable);
/// ```
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct WindowConfig {
    /// Text shown in the OS title bar.
    pub title: String,
    /// Initial inner size in logical pixels: `(width, height)`.
    pub size: (u32, u32),
    /// Whether the user can resize the window by dragging its borders.
    pub resizable: bool,
}

impl Default for WindowConfig {
    /// Returns a 720p, resizable window titled `"Spark"`.
    ///
    /// # Logic
    ///
    /// Picks `1280 × 720` — the smallest common 16:9 resolution — so the
    /// window opens on essentially every modern display without scrolling
    /// off-screen.
    ///
    /// # Why it works
    ///
    /// These values are safe defaults for a desktop game window; they are
    /// not "right" for every host but every host accepts them. Override
    /// with the `with_*` builders when something else is wanted.
    ///
    /// # How to use
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default();
    /// assert_eq!(cfg.size, (1280, 720));
    /// ```
    ///
    /// # How NOT to use
    ///
    /// - Do not assume the defaults will never change. Treat them as
    ///   "sensible starting point", not "load-bearing values".
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default();
    /// assert!(cfg.resizable);
    /// ```
    fn default() -> Self {
        Self {
            title: "Spark".to_owned(),
            size: (1280, 720),
            resizable: true,
        }
    }
}

impl WindowConfig {
    /// Sets the window title.
    ///
    /// # Logic
    ///
    /// Takes anything that can be turned into a [`String`] (so `&str`,
    /// `String`, `Cow<str>`, etc. all work), stores it, and returns `self`
    /// for chaining.
    ///
    /// # Why it works
    ///
    /// `Into<String>` is the standard Rust way to accept "anything
    /// stringy" in a builder without forcing the caller to choose
    /// between `&str` and `String`.
    ///
    /// # How to use
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_title("Hello");
    /// assert_eq!(cfg.title, "Hello");
    /// ```
    ///
    /// # How NOT to use
    ///
    /// - Do not call this in a hot loop; each call allocates a new
    ///   `String`.
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default()
    ///     .with_title(String::from("Owned"));
    /// assert_eq!(cfg.title, "Owned");
    /// ```
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the initial inner size in logical pixels.
    ///
    /// # Logic
    ///
    /// Stores `(width, height)` and returns `self`. No clamping or
    /// validation is performed — `winit` rejects invalid sizes at
    /// window-creation time inside [`crate::run`].
    ///
    /// # Why it works
    ///
    /// Logical (DPI-independent) pixels are the right unit for "what the
    /// human sees": on a 2x retina display, `1280 × 720` logical pixels
    /// becomes `2560 × 1440` physical pixels. `winit` does the conversion.
    ///
    /// # How to use
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_size(800, 600);
    /// assert_eq!(cfg.size, (800, 600));
    /// ```
    ///
    /// # How NOT to use
    ///
    /// - Do not pass `(0, 0)`; the OS will reject it and [`crate::run`]
    ///   will return an error.
    /// - Do not pass *physical* pixels here. Use the host's logical scale.
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_size(1920, 1080);
    /// assert_eq!(cfg.size.0, 1920);
    /// ```
    #[must_use]
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.size = (width, height);
        self
    }

    /// Sets whether the user can resize the window.
    ///
    /// # Logic
    ///
    /// Stores the flag and returns `self`. Passed straight through to
    /// [`winit::window::WindowAttributes::with_resizable`] inside
    /// [`crate::run`].
    ///
    /// # Why it works
    ///
    /// Games sometimes want a fixed window (e.g. pixel-art titles that
    /// snap to integer scale); most desktop apps want resize. A single
    /// boolean covers both.
    ///
    /// # How to use
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_resizable(false);
    /// assert!(!cfg.resizable);
    /// ```
    ///
    /// # How NOT to use
    ///
    /// - On some platforms (notably Wayland) a non-resizable window may
    ///   still be "resized" by the compositor (tiling). Do not assume
    ///   `false` means "the size will never change at runtime".
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_resizable(true);
    /// assert!(cfg.resizable);
    /// ```
    #[must_use]
    pub fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }
}
