//! OS event-loop driver — also the per-frame scheduler.
//!
//! [`run`] takes an owned [`Application`] plus a [`WindowConfig`],
//! builds a [`winit::event_loop::EventLoop`], pairs it with an
//! internal `EventLoopRunner` that implements
//! [`ApplicationHandler`](winit::application::ApplicationHandler), and
//! hands the loop to winit. On every `RedrawRequested`, the runner
//! ticks the per-frame stages — `PreUpdate → (FixedUpdate × N) →
//! Update → PostUpdate` — then asks winit for the next redraw.
//! `FixedUpdate` runs off a real-time accumulator so the simulation
//! advances in fixed 60 Hz steps regardless of display frame rate.

use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use spark_core::{Application, Stage};
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::config::WindowConfig;
use crate::error::WindowError;

/// Length of one fixed-timestep simulation step: 1/60 s ≈ 16.667 ms.
const FIXED_TIMESTEP: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Upper bound on the real time a single frame may contribute to the
/// accumulator.
///
/// Without it, a long stall (a breakpoint, a dragged title bar) would
/// bank seconds of elapsed time and then fire hundreds of catch-up
/// `FixedUpdate` steps in one frame — the "spiral of death", where each
/// oversized frame begets a larger one. Clamping a frame to 250 ms caps
/// catch-up at ~15 steps and lets the sim fall behind gracefully instead.
const MAX_FRAME_TIME: Duration = Duration::from_millis(250);

/// Opens the window from `config` and drives the OS event loop until
/// the user closes it, ticking [`Application`]'s per-frame stages on
/// every `RedrawRequested`. Blocks the calling thread; must run on the
/// main thread. Uses [`ControlFlow::Wait`] — `Window::request_redraw`
/// wakes the loop for the next frame, naturally throttled by the OS.
///
/// Typically reached via
/// [`WindowPlugin`](crate::WindowPlugin)'s runner closure rather than
/// called directly.
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
/// use spark_core::Application;
/// use spark_window::{run, WindowConfig};
///
/// let app = Application::new();
/// run(app, WindowConfig::default().with_title("Tiny").with_size(320, 240))?;
/// # Ok::<(), spark_window::WindowError>(())
/// ```
pub fn run(app: Application, config: WindowConfig) -> Result<(), WindowError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut runner = EventLoopRunner {
        app,
        config,
        window: None,
        last_frame: None,
        fixed_accumulator: Duration::ZERO,
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
/// `app` is the owned [`Application`] — the runner ticks its stages on
/// every `RedrawRequested`. `window` is `Option` because winit only
/// creates it after the loop is active
/// ([`ApplicationHandler::resumed`]). M4 will move the window onto the
/// ECS `World` as a resource. `last_frame` and `fixed_accumulator` are
/// the fixed-timestep clock: the instant of the previous frame and the
/// real time banked but not yet consumed by whole `FixedUpdate` steps.
struct EventLoopRunner {
    app: Application,
    config: WindowConfig,
    window: Option<Window>,
    last_frame: Option<Instant>,
    fixed_accumulator: Duration,
}

impl ApplicationHandler for EventLoopRunner {
    /// Builds the OS window from `self.config`, then asks winit to
    /// deliver the first `RedrawRequested` — that's what kicks the
    /// per-frame loop into motion. winit calls this once on desktop
    /// platforms and again on every foreground on mobile.
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
                // Kick the redraw chain — without this, the loop sits
                // idle in `Wait` mode until an external event fires.
                window.request_redraw();
                self.window = Some(window);
            }
            Err(err) => {
                tracing::error!(error = %err, "failed to create window; exiting");
                event_loop.exit();
            }
        }
    }

    /// Routes each OS event to the right handler.
    ///
    /// - `CloseRequested`: exits the loop so [`run`] returns.
    /// - `RedrawRequested`: ticks `PreUpdate → (FixedUpdate × N) →
    ///   Update → PostUpdate`, draining the fixed-timestep accumulator
    ///   between `PreUpdate` and `Update`, then requests the next redraw
    ///   to keep the loop alive.
    /// - Lifecycle / input events: logged at appropriate `tracing`
    ///   levels (cursor at `trace`, input at `debug`, focus/resize at
    ///   `info`).
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
            WindowEvent::RedrawRequested => {
                // Advance the fixed-timestep clock: bank this frame's real
                // elapsed time (clamped against the spiral of death), so the
                // accumulator below can spend it in whole 1/60 s steps.
                let now = Instant::now();
                let frame_time = self.last_frame.map_or(Duration::ZERO, |prev| {
                    now.saturating_duration_since(prev).min(MAX_FRAME_TIME)
                });
                self.last_frame = Some(now);
                self.fixed_accumulator += frame_time;

                // Per-frame tick. Each stage flushes its pending commands at
                // the end of its run via `Application::run_stage`.
                self.app.run_stage(Stage::PreUpdate);
                // Fixed-timestep simulation: run one `FixedUpdate` per whole
                // step the accumulator covers, carrying the remainder into the
                // next frame. Zero steps on a fast frame, several after a slow
                // one — but never an unbounded burst, thanks to the clamp.
                while self.fixed_accumulator >= FIXED_TIMESTEP {
                    self.app.run_stage(Stage::FixedUpdate);
                    self.fixed_accumulator -= FIXED_TIMESTEP;
                }
                self.app.run_stage(Stage::Update);
                self.app.run_stage(Stage::PostUpdate);
                // Request the next frame. Under `ControlFlow::Wait`
                // this is what wakes the loop for the next tick.
                if let Some(window) = self.window.as_ref() {
                    window.request_redraw();
                }
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
