//! OS event-loop driver — also the per-frame scheduler.
//!
//! [`run`] takes an owned [`Application`] plus a [`WindowConfig`],
//! builds a [`winit::event_loop::EventLoop`], pairs it with an
//! internal `EventLoopRunner` that implements
//! [`ApplicationHandler`](winit::application::ApplicationHandler), and
//! hands the loop to winit under [`ControlFlow::Poll`]. The frame splits
//! across two winit hooks:
//!
//! - [`about_to_wait`](ApplicationHandler::about_to_wait) runs once per loop
//!   iteration and drives the *simulation* block — `Input → PreUpdate →
//!   (FixedUpdate × N) → Update → PostUpdate`. `Input` runs first and swaps the
//!   double-buffered event queues; `PreUpdate` advances the
//!   [`Time`](spark_common::Time) clock, which owns the 60 Hz fixed-timestep
//!   accumulator. The runner then reads
//!   [`Time::fixed_steps_this_frame`](spark_common::Time::fixed_steps_this_frame)
//!   and dispatches `FixedUpdate` that many times, so the simulation advances
//!   in fixed 60 Hz steps regardless of display frame rate.
//! - `RedrawRequested` drives only `Stage::Render`. Once a `wgpu` swapchain
//!   lands (M5) its present-rate paces this hook at vsync; until then no render
//!   systems are registered, so it is a no-op.
//!
//! The runner holds no clock state of its own.
//!
//! Under `Poll`, `about_to_wait` fires as fast as the CPU allows. With no
//! swapchain yet to gate `RedrawRequested`, nothing throttles the loop and it
//! busy-spins a core — a known, deferred cost (see [`run`]). Sim correctness is
//! unaffected: `advance_time` banks *wall-clock* time, so `FixedUpdate` still
//! steps at a steady 60 Hz no matter how often the loop iterates.

use std::num::NonZeroU32;

use spark_common::Time;
use spark_core::{Application, Stage};
use tracing::info;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::config::WindowConfig;
use crate::error::WindowError;

/// Opens the window from `config` and drives the OS event loop until
/// the user closes it. Blocks the calling thread; must run on the main
/// thread. The simulation block ticks in
/// [`about_to_wait`](ApplicationHandler::about_to_wait); `Stage::Render`
/// ticks in `RedrawRequested`.
///
/// Uses [`ControlFlow::Poll`]: winit begins a new loop iteration
/// immediately rather than sleeping, so `about_to_wait` fires every
/// iteration. **Without a swapchain to gate `RedrawRequested` (M5,
/// `spark-render`), nothing paces the loop and it busy-spins a CPU
/// core.** That regression is accepted deliberately — this flip ships
/// ahead of its [trigger conditions](https://github.com/AlexTiTanium/spark/issues/50)
/// so the sim/render split is in place; the swapchain's present-rate
/// will gate `RedrawRequested` and the spin disappears once it lands.
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
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut runner = EventLoopRunner {
        app,
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
/// `app` is the owned [`Application`] — the runner ticks its simulation
/// stages in `about_to_wait` and `Stage::Render` in `RedrawRequested`.
/// `window` is `Option` because winit only
/// creates it after the loop is active
/// ([`ApplicationHandler::resumed`]). M4 will move the window onto the
/// ECS `World` as a resource. The runner holds no clock state of its own:
/// the fixed-timestep accumulator lives in the [`Time`] resource, which
/// `advance_time` updates in `PreUpdate`.
struct EventLoopRunner {
    app: Application,
    config: WindowConfig,
    window: Option<Window>,
}

impl ApplicationHandler for EventLoopRunner {
    /// Builds the OS window from `self.config`. Under
    /// [`ControlFlow::Poll`] the loop runs `about_to_wait` every
    /// iteration regardless, so no redraw kick is needed to start the
    /// simulation — `about_to_wait` requests each frame's redraw itself.
    /// winit calls this once on desktop platforms and again on every
    /// foreground on mobile.
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

    /// Routes each OS event to the right handler.
    ///
    /// - `CloseRequested`: exits the loop so [`run`] returns.
    /// - `RedrawRequested`: ticks `Stage::Render` only. The simulation
    ///   block lives in [`about_to_wait`](Self::about_to_wait); this hook
    ///   is reserved for rendering, which a `wgpu` swapchain will pace at
    ///   vsync (M5). No render systems are registered yet, so today it is a
    ///   no-op.
    /// - Lifecycle / input events: logged at appropriate `tracing`
    ///   levels (cursor at `trace`, input at `debug`, focus/resize at
    ///   `info`). Device input (keyboard, mouse-button, cursor, wheel)
    ///   and focus-loss are handed to [`crate::input`], which logs them
    ///   and forwards the matching [`spark_input`] events into the world
    ///   (guarded, so a no-op unless a consumer registered the buffers).
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
                // Render only. The simulation block runs in `about_to_wait`;
                // this hook is reserved for `Stage::Render`, which a `wgpu`
                // swapchain will pace at vsync (M5). No render systems are
                // registered yet, so this is a no-op today.
                self.app.run_stage(Stage::Render);
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
                // Losing focus: the OS delivers the matching key-up / button-up
                // events to whatever window took focus, not to us. Signal
                // consumers to clear held state so nothing appears stuck.
                if !focused {
                    crate::input::forward_focus_lost(&self.app);
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                info!(scale_factor, "window scale factor changed");
            }
            // Device input: logged and forwarded by `crate::input`, keeping the
            // winit→`spark-input` translation out of this loop.
            WindowEvent::KeyboardInput { event, .. } => {
                crate::input::forward_keyboard(&self.app, &event);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                crate::input::forward_mouse_button(&self.app, state, button);
            }
            WindowEvent::CursorMoved { position, .. } => {
                crate::input::forward_cursor(&self.app, position);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                crate::input::forward_wheel(&self.app, delta);
            }
            _ => {}
        }
    }

    /// Drives the per-frame **simulation** block, once per loop iteration.
    ///
    /// Under [`ControlFlow::Poll`] winit calls this every iteration after
    /// pending OS events drain — the natural home for input + simulation,
    /// decoupled from rendering. It ticks the stages in `Application`'s
    /// canonical order, minus the reserved render slot:
    ///
    /// `Input → PreUpdate → (FixedUpdate × N) → Update → PostUpdate`
    ///
    /// `Input` runs **first** (non-negotiable): it pumps the per-event swap
    /// systems registered by `Application::add_event`, rotating each
    /// `Events<T>` buffer so this frame's readers observe last frame's sends
    /// before any other stage touches them. `PreUpdate` runs `advance_time`
    /// (registered first by `TimePlugin`): it samples the wall clock, advances
    /// [`Time`], and banks the fixed-timestep accumulator for this frame. Each
    /// stage flushes its pending commands at the end of its run via
    /// `Application::run_stage`.
    ///
    /// After the sim block it asks the platform for a frame; the render lands
    /// in `RedrawRequested`. Redraw requests coalesce, so requesting one every
    /// iteration is harmless — once a swapchain gates `RedrawRequested` at
    /// vsync (M5) this hook keeps stepping the sim while rendering paces
    /// independently.
    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        self.app.run_stage(Stage::Input);
        self.app.run_stage(Stage::PreUpdate);

        // Fixed-timestep simulation: `Time` already computed how many whole
        // 1/60 s steps this frame's banked time covers — run `FixedUpdate`
        // exactly that many times. Zero on a fast frame, several after a slow
        // one, but never an unbounded burst (the 250 ms clamp inside
        // `Time::tick` caps it at ~15).
        let fixed_steps = self.app.world().resource::<Time>().fixed_steps_this_frame();
        for _ in 0..fixed_steps {
            self.app.run_stage(Stage::FixedUpdate);
        }

        self.app.run_stage(Stage::Update);
        self.app.run_stage(Stage::PostUpdate);

        // Ask the platform for a frame; `RedrawRequested` runs `Stage::Render`.
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }
}
