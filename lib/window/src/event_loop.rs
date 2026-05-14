//! OS event-loop driver.
//!
//! # Summary
//!
//! Contains the load-bearing entry point of this crate: [`run`], which
//! constructs a [`winit::event_loop::EventLoop`], builds an internal
//! `EventLoopRunner` value that implements
//! [`winit::application::ApplicationHandler`], and hands the loop off to
//! winit. Every OS event the engine cares about is observed here and
//! emitted as a `tracing` event.
//!
//! # Logic
//!
//! winit 0.30 adopted a callback-object model: instead of a `match` arm
//! per event inside a closure, you implement
//! [`ApplicationHandler`](winit::application::ApplicationHandler) on a
//! state struct and pass that struct to
//! [`EventLoop::run_app`](winit::event_loop::EventLoop::run_app). Two
//! callbacks matter today:
//!
//! 1. `resumed` — fired once on desktop platforms (and every time the OS
//!    re-foregrounds the app on mobile). We create the actual
//!    [`winit::window::Window`] here, because some platforms only allow
//!    window creation after the event loop is "active".
//! 2. `window_event` — fired for every input or lifecycle event tied to
//!    the window. We translate the variants we care about into
//!    `tracing::info!` / `debug!` / `trace!` calls and exit the loop on
//!    `CloseRequested`.
//!
//! # Memory layout
//!
//! ```text
//! EventLoopRunner {
//!     config: WindowConfig,     // copied in from the caller
//!     window: Option<Window>,   // None until `resumed`; Some afterwards
//! }
//! ```
//!
//! Stored on the stack of [`run`]; winit holds a `&mut` to it for the
//! lifetime of the event loop. No globals.
//!
//! # Why it works
//!
//! `Option<Window>` matches winit 0.30's contract: the window does not
//! exist before `resumed` and must not be created on the main thread
//! before the event loop is pumping. Storing it as `Option<Window>` lets
//! the value type be constructed up front (before `run_app` takes the
//! borrow) and populated lazily inside the callback. Every read of
//! `self.window` happens inside `window_event`, where the OS has already
//! delivered events for *some* window, so `Some(_)` is guaranteed.
//!
//! # How to use
//!
//! ```no_run
//! spark_window::run(spark_window::WindowConfig::default())?;
//! # Ok::<(), spark_window::WindowError>(())
//! ```
//!
//! # How NOT to use
//!
//! - Do not call [`run`] off the main thread; winit requires the OS
//!   event loop to live on the process's main thread on every supported
//!   platform.
//! - Do not call [`run`] twice in the same process; the second call will
//!   panic inside winit.
//! - Do not store a reference to the window across loop ticks. Use the
//!   `&mut self` borrow that [`ApplicationHandler`](winit::application::ApplicationHandler)
//!   gives you.

use std::num::NonZeroU32;

use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::config::WindowConfig;
use crate::error::WindowError;

/// Opens a window described by `config` and drives the OS event loop
/// until the user closes the window.
///
/// # Logic
///
/// 1. Build a [`winit::event_loop::EventLoop`].
/// 2. Set its control flow to [`ControlFlow::Wait`] so the thread blocks
///    between events instead of busy-looping.
/// 3. Construct an `EventLoopRunner` value that owns the `config` and a
///    `None` window slot.
/// 4. Call
///    [`EventLoop::run_app`](winit::event_loop::EventLoop::run_app),
///    handing the runner across to winit. winit calls back into the
///    runner's methods until `el.exit()` is invoked (currently triggered
///    by `WindowEvent::CloseRequested`).
///
/// # Errors
///
/// Returns [`WindowError::EventLoop`] if winit cannot create or drive
/// the OS event loop (rare; usually another library has claimed the
/// main thread). Returns [`WindowError::Os`] if the OS refuses to
/// create the window (invalid size, missing permissions, …) — surfaced
/// via the `?` operator from inside [`ApplicationHandler::resumed`].
///
/// # Why it works
///
/// `EventLoop::new()` is the documented entry point for desktop platforms
/// and produces an error type already wrapped by [`WindowError::EventLoop`].
/// The control-flow choice keeps the CPU idle when nothing is happening —
/// appropriate for a desk-bound editor but will be replaced by
/// [`ControlFlow::Poll`] once we have a fixed-timestep simulation that
/// needs to tick every frame (M3+).
///
/// # How to use
///
/// ```no_run
/// fn main() -> Result<(), spark_window::WindowError> {
///     spark_window::init_tracing();
///     spark_window::run(
///         spark_window::WindowConfig::default()
///             .with_title("Spark")
///             .with_size(1280, 720),
///     )
/// }
/// ```
///
/// # How NOT to use
///
/// - Do not call this from a unit or doc test; it blocks on the OS
///   event loop and the test will hang.
/// - Do not call this from a non-main thread; winit panics.
///
/// # Examples
///
/// ```no_run
/// // Open a small fixed-size window:
/// spark_window::run(
///     spark_window::WindowConfig::default()
///         .with_title("Tiny")
///         .with_size(320, 240)
///         .with_resizable(false),
/// )?;
/// # Ok::<(), spark_window::WindowError>(())
/// ```
pub fn run(config: WindowConfig) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut runner = EventLoopRunner {
        config,
        window: None,
    };

    info!(
        version = spark_core::VERSION,
        "spark-window starting event loop"
    );
    event_loop.run_app(&mut runner)?;
    info!("spark-window event loop exited");

    Ok(())
}

/// Internal state held across calls into the OS event loop.
///
/// # Logic
///
/// Holds the [`WindowConfig`] passed by the caller and the
/// [`winit::window::Window`] handle (which only exists after
/// [`ApplicationHandler::resumed`] has fired). All trait-method bodies
/// read or mutate `self`; nothing escapes the value.
///
/// # Memory layout
///
/// ```text
/// EventLoopRunner {
///     config: WindowConfig,
///     window: Option<Window>,
/// }
/// ```
///
/// # Why it works
///
/// winit hands the trait methods `&mut self`, so any state we need
/// across events lives here. Using `Option` for the window matches
/// winit's "no window before `resumed`" contract without requiring
/// uninitialised memory.
///
/// # How to use
///
/// Not part of the public API; construct it via [`run`] and let winit
/// drive it.
///
/// # How NOT to use
///
/// - Do not expose this type publicly; it is an implementation detail
///   of [`run`] and may change shape when M4 introduces the
///   `WindowPlugin` / ECS-resource form.
struct EventLoopRunner {
    config: WindowConfig,
    window: Option<Window>,
}

impl ApplicationHandler for EventLoopRunner {
    /// Called by winit once the OS event loop becomes active.
    ///
    /// # Logic
    ///
    /// Builds [`WindowAttributes`] from `self.config` and asks the
    /// [`ActiveEventLoop`] to create the actual OS window. The handle is
    /// stored in `self.window` so `window_event` can inspect it later.
    /// On failure we log the error and exit the loop — there is no
    /// useful recovery path before the user has even seen a window.
    ///
    /// # Why it works
    ///
    /// `ActiveEventLoop::create_window` is the only winit 0.30 API that
    /// can produce a [`Window`]; calling it from `resumed` matches the
    /// platform contract on Android, iOS, macOS, Wayland, and X11 (where
    /// a window cannot exist before the event loop is pumping).
    ///
    /// # How NOT to use
    ///
    /// - Do not call this directly; winit invokes it.
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }

        let (width, height) = self.config.size;
        let attrs = WindowAttributes::default()
            .with_title(self.config.title.clone())
            .with_inner_size(LogicalSize::new(width, height))
            .with_resizable(self.config.resizable);

        match event_loop.create_window(attrs) {
            Ok(window) => {
                let inner = window.inner_size();
                info!(
                    title = %self.config.title,
                    requested_size = ?self.config.size,
                    actual_size = ?(inner.width, inner.height),
                    scale_factor = window.scale_factor(),
                    "window created"
                );
                self.window = Some(window);
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to create window; exiting");
                event_loop.exit();
            }
        }
    }

    /// Called by winit for every window-scoped OS event.
    ///
    /// # Logic
    ///
    /// Routes the event variant to the right `tracing` macro: lifecycle
    /// events (`CloseRequested`, `Resized`, `Focused`,
    /// `ScaleFactorChanged`) go to `info!`; per-input events
    /// (`KeyboardInput`, `MouseInput`) go to `debug!`; high-rate events
    /// (`CursorMoved`) go to `trace!`. `CloseRequested` additionally
    /// calls `event_loop.exit()` so [`run`] returns.
    ///
    /// # Why it works
    ///
    /// The level split keeps a default `spark_window=info` filter
    /// readable (one line per lifecycle change) while leaving
    /// `RUST_LOG=spark_window=trace` available for cursor-level detail.
    /// The `_` arm explicitly ignores unhandled variants so adding new
    /// event types in future winit versions is a compile-time noise
    /// rather than a behaviour change.
    ///
    /// # How NOT to use
    ///
    /// - Do not call this directly; winit invokes it.
    /// - Do not add expensive work here. The OS event loop is the
    ///   main thread; long work belongs in a system that runs in
    ///   `Update` / `Render` (M3+).
    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                info!("close requested; exiting event loop");
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                let nonzero = (NonZeroU32::new(size.width), NonZeroU32::new(size.height));
                info!(
                    width = size.width,
                    height = size.height,
                    minimised = matches!(nonzero, (None, _) | (_, None)),
                    "window resized"
                );
            }
            WindowEvent::Focused(focused) => {
                info!(focused, "window focus changed");
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                info!(scale_factor, "window scale factor changed");
            }
            WindowEvent::KeyboardInput { event, .. } => {
                debug!(state = ?event.state, key = ?event.logical_key, "keyboard input");
            }
            WindowEvent::MouseInput { state, button, .. } => {
                debug!(?state, ?button, "mouse input");
            }
            WindowEvent::CursorMoved { position, .. } => {
                trace!(x = position.x, y = position.y, "cursor moved");
            }
            _ => {}
        }
    }
}
