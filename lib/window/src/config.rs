//! Configuration for the application window.
//!
//! [`WindowConfig`] is the plain-data input to [`crate::run`]: title,
//! initial size in logical pixels, and a `resizable` flag. Built with a
//! small chainable API. `#[non_exhaustive]` so future fields (icon,
//! decorations, fullscreen, …) can be added without a breaking change.

/// Configuration for the application window opened by [`crate::run`].
///
/// `size` is in **logical** (DPI-independent) pixels — on a 2x display,
/// `(1280, 720)` becomes `2560 × 1440` physical pixels. `winit` does the
/// conversion. `#[non_exhaustive]` forbids external struct-literal
/// construction; use [`WindowConfig::default`] and the `with_*` builders.
///
/// # Examples
///
/// ```
/// use spark_window::WindowConfig;
/// let cfg = WindowConfig::default()
///     .with_title("Hello")
///     .with_size(800, 600)
///     .with_resizable(false);
/// assert_eq!(cfg.title, "Hello");
/// assert_eq!(cfg.size, (800, 600));
/// assert!(!cfg.resizable);
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
    /// 720p, resizable, titled `"Spark"` — safe on essentially every
    /// modern display.
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default();
    /// assert_eq!(cfg.title, "Spark");
    /// assert_eq!(cfg.size, (1280, 720));
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
    /// Sets the window title. Accepts `&str` or `String` via `Into<String>`.
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_title("Hello");
    /// assert_eq!(cfg.title, "Hello");
    /// ```
    #[must_use]
    pub fn with_title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Sets the initial inner size in logical pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_size(800, 600);
    /// assert_eq!(cfg.size, (800, 600));
    /// ```
    #[must_use]
    pub fn with_size(mut self, width: u32, height: u32) -> Self {
        self.size = (width, height);
        self
    }

    /// Sets whether the user can resize the window.
    ///
    /// # Examples
    ///
    /// ```
    /// let cfg = spark_window::WindowConfig::default().with_resizable(false);
    /// assert!(!cfg.resizable);
    /// ```
    #[must_use]
    pub fn with_resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }
}
