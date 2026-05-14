//! OS event-loop driver.
//!
//! Contains [`run`]: builds a [`winit::event_loop::EventLoop`], pairs it
//! with an internal `EventLoopRunner` that implements
//! [`ApplicationHandler`](winit::application::ApplicationHandler), and
//! hands the loop off to winit. Every OS event the engine cares about is
//! observed here and emitted as a `tracing` event.
//!
//! winit 0.30 adopted a callback-object model: state lives in a struct
//! that implements `ApplicationHandler`, and winit calls the trait
//! methods until `event_loop.exit()` is invoked. We use that struct to
//! hold the [`WindowConfig`] and the lazily-created
//! [`Window`](winit::window::Window) — the window cannot exist before
//! `resumed` fires.

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
/// Sets [`ControlFlow::Wait`] so the thread blocks between events (no
/// busy-loop). This will switch to [`ControlFlow::Poll`] once we have a
/// fixed-timestep simulation that needs to tick every frame (M3+).
///
/// Blocks the calling thread. Must run on the main thread; winit panics
/// otherwise.
///
/// # Errors
///
/// Returns [`WindowError::EventLoop`] if winit cannot create or drive
/// the OS event loop, or [`WindowError::Os`] if the OS refuses to create
/// the window.
///
/// # Examples
///
/// ```no_run
/// spark_window::run(
///     spark_window::WindowConfig::default()
///         .with_title("Tiny")
///         .with_size(320, 240),
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

/// State held across calls into the OS event loop.
///
/// `window` is `Option` because winit only allows window creation after
/// the event loop is active — it stays `None` until
/// [`ApplicationHandler::resumed`] populates it. Once `spark-ecs` lands
/// in M4, this field set becomes a `Window` resource on the `World`.
struct EventLoopRunner {
    config: WindowConfig,
    window: Option<Window>,
}

impl ApplicationHandler for EventLoopRunner {
    /// Builds the OS window from `self.config`. winit calls this once on
    /// desktop platforms, and again on every foreground on mobile.
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

    /// Routes each OS event to the right `tracing` level: lifecycle at
    /// `info`, per-input at `debug`, high-rate cursor moves at `trace`.
    /// `CloseRequested` calls `event_loop.exit()` so [`run`] returns.
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
