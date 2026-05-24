//! OS event-loop driver — also the per-frame scheduler.
//!
//! [`run`] takes an owned [`Application`] plus a [`WindowConfig`],
//! builds a [`winit::event_loop::EventLoop`], pairs it with an
//! internal `EventLoopRunner` that implements
//! [`ApplicationHandler`](winit::application::ApplicationHandler), and
//! hands the loop to winit. On every `RedrawRequested`, the runner
//! ticks the per-frame stages — `Input → PreUpdate → (FixedUpdate × N) →
//! Update → PostUpdate` — then asks winit for the next redraw. `Input`
//! runs first and swaps the double-buffered event queues; `FixedUpdate`
//! runs off a real-time accumulator so the simulation advances in fixed
//! 60 Hz steps regardless of display frame rate.

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

/// Banks one frame's elapsed time into `accumulator` and returns how many
/// whole [`FIXED_TIMESTEP`] steps to run this frame, carrying the remainder.
///
/// This is the pure heart of the fixed-timestep loop, factored out of the
/// `RedrawRequested` handler so it can be tested without a window or an OS
/// clock. The caller samples the wall clock; this function owns the policy:
///
/// 1. Clamp `frame_dt` to [`MAX_FRAME_TIME`] *before* banking, so one long
///    stall can't bank seconds and trigger a catch-up burst (the spiral of
///    death — see [`MAX_FRAME_TIME`]).
/// 2. Add the clamped delta to `accumulator`.
/// 3. Spend the accumulator in whole [`FIXED_TIMESTEP`] chunks, returning the
///    count and leaving the sub-step remainder banked for the next call.
///
/// Because the step is fixed, the simulation advances at a steady 60 Hz no
/// matter the display rate — the property that keeps it deterministic across
/// hardware. The `>=` test is inclusive: an accumulator resting exactly on a
/// step boundary spends it now, not next frame. Behaviour is pinned by the
/// table-driven tests in this module (a doctest can't reach a private fn).
fn drain_fixed_steps(accumulator: &mut Duration, frame_dt: Duration) -> u32 {
    *accumulator += frame_dt.min(MAX_FRAME_TIME);
    let mut steps: u32 = 0;
    while *accumulator >= FIXED_TIMESTEP {
        *accumulator -= FIXED_TIMESTEP;
        steps += 1;
    }
    steps
}

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
    /// - `RedrawRequested`: ticks `Input → PreUpdate → (FixedUpdate × N) →
    ///   Update → PostUpdate`, swapping event buffers in `Input`, draining
    ///   the fixed-timestep accumulator between `PreUpdate` and `Update`,
    ///   then requests the next redraw to keep the loop alive.
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
                // Sample this frame's real elapsed wall-clock time. The clamp
                // and accumulator math live in `drain_fixed_steps`; here we
                // only read the clock and hand it the delta.
                let now = Instant::now();
                let frame_time = self
                    .last_frame
                    .map_or(Duration::ZERO, |prev| now.saturating_duration_since(prev));
                self.last_frame = Some(now);
                let fixed_steps = drain_fixed_steps(&mut self.fixed_accumulator, frame_time);

                // Per-frame tick. Each stage flushes its pending commands at
                // the end of its run via `Application::run_stage`.
                //
                // `Input` runs first: it pumps the per-event swap systems
                // registered by `Application::add_event`, rotating each
                // `Events<T>` buffer so this frame's readers observe last
                // frame's sends before any other stage touches them.
                self.app.run_stage(Stage::Input);
                self.app.run_stage(Stage::PreUpdate);
                // Fixed-timestep simulation: `drain_fixed_steps` already told us
                // how many whole 1/60 s steps this frame's banked time covers —
                // run `FixedUpdate` exactly that many times. Zero on a fast
                // frame, several after a slow one, but never an unbounded burst
                // (the 250 ms clamp inside the helper caps it at ~15).
                for _ in 0..fixed_steps {
                    self.app.run_stage(Stage::FixedUpdate);
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{FIXED_TIMESTEP, MAX_FRAME_TIME, drain_fixed_steps};

    /// A frame worth exactly one fixed step runs one `FixedUpdate` and leaves
    /// nothing banked.
    #[test]
    fn single_step_at_60hz() {
        let mut acc = Duration::ZERO;
        assert_eq!(drain_fixed_steps(&mut acc, FIXED_TIMESTEP), 1);
        assert_eq!(acc, Duration::ZERO);
    }

    /// A frame shorter than one step runs zero `FixedUpdate`s and banks the
    /// whole delta for next time.
    #[test]
    fn fast_frame_runs_no_steps_and_banks_time() {
        let mut acc = Duration::ZERO;
        let dt = Duration::from_millis(8); // ~half a step
        assert_eq!(drain_fixed_steps(&mut acc, dt), 0);
        assert_eq!(acc, dt);
    }

    /// Sub-step deltas accumulate across frames: four 5 ms frames bank 20 ms,
    /// and the single whole step inside that fires on the fourth frame, with
    /// the ~3.33 ms remainder carried forward.
    #[test]
    fn carry_across_four_fast_frames() {
        let mut acc = Duration::ZERO;
        let dt = Duration::from_millis(5);
        let mut total: u32 = 0;
        for _ in 0..4 {
            total += drain_fixed_steps(&mut acc, dt);
        }
        assert_eq!(total, 1);
        // Remainder + the one step we spent == the 20 ms banked (stated as an
        // addition to dodge `Duration`'s unchecked-subtraction lint).
        assert_eq!(acc + FIXED_TIMESTEP, Duration::from_millis(20));
    }

    /// A 50 ms frame covers three whole steps, so it runs three `FixedUpdate`s
    /// and banks a sub-step remainder.
    #[test]
    fn slow_frame_runs_multiple_steps() {
        let mut acc = Duration::ZERO;
        assert_eq!(drain_fixed_steps(&mut acc, Duration::from_millis(50)), 3);
        assert!(acc < FIXED_TIMESTEP);
    }

    /// Two slow frames in a row keep the accumulator's carry: each 50 ms frame
    /// runs three steps (six total) and the tiny remainder persists between
    /// them rather than resetting.
    #[test]
    fn two_slow_frames_preserve_carry() {
        let mut acc = Duration::ZERO;
        let first = drain_fixed_steps(&mut acc, Duration::from_millis(50));
        let second = drain_fixed_steps(&mut acc, Duration::from_millis(50));
        assert_eq!(first, 3);
        assert_eq!(second, 3);
        assert!(acc < FIXED_TIMESTEP);
    }

    /// The spiral-of-death guard: a pathological 1 s frame is clamped to
    /// `MAX_FRAME_TIME` (250 ms) before banking, capping catch-up at the ~15
    /// steps that fit in 250 ms instead of the 60 a full second would demand.
    #[test]
    fn clamp_caps_pathological_frame() {
        let mut acc = Duration::ZERO;
        let steps = drain_fixed_steps(&mut acc, Duration::from_secs(1));
        let max_steps = MAX_FRAME_TIME.as_nanos() / FIXED_TIMESTEP.as_nanos();
        assert_eq!(u128::from(steps), max_steps);
        assert_eq!(steps, 15);
    }

    /// A zero-length delta (the first frame, before any time has passed) runs
    /// nothing and banks nothing.
    #[test]
    fn zero_elapsed_runs_no_steps() {
        let mut acc = Duration::ZERO;
        assert_eq!(drain_fixed_steps(&mut acc, Duration::ZERO), 0);
        assert_eq!(acc, Duration::ZERO);
    }

    /// The `>=` boundary is inclusive: an accumulator resting exactly on a step
    /// boundary spends it now (one step, zero remainder), even with no new time
    /// this frame.
    #[test]
    fn exact_boundary_is_inclusive() {
        let mut acc = FIXED_TIMESTEP;
        assert_eq!(drain_fixed_steps(&mut acc, Duration::ZERO), 1);
        assert_eq!(acc, Duration::ZERO);
    }
}
