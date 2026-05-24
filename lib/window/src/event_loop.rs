//! OS event-loop driver — also the per-frame scheduler.
//!
//! [`run`] takes an owned [`Application`] plus a [`WindowConfig`],
//! builds a [`winit::event_loop::EventLoop`], pairs it with an
//! internal `EventLoopRunner` that implements
//! [`ApplicationHandler`](winit::application::ApplicationHandler), and
//! hands the loop to winit. On every `RedrawRequested`, the runner
//! ticks the per-frame stages — `Input → PreUpdate → (FixedUpdate × N) →
//! Update → PostUpdate` — then asks winit for the next redraw. `Input`
//! runs first and swaps the double-buffered event queues; `PreUpdate`
//! advances the [`Time`](spark_common::Time) clock, which owns the 60 Hz
//! fixed-timestep accumulator. The runner then reads
//! [`Time::fixed_steps_this_frame`](spark_common::Time::fixed_steps_this_frame)
//! and dispatches `FixedUpdate` that many times, so the simulation advances
//! in fixed 60 Hz steps regardless of display frame rate. The runner holds no
//! clock state of its own.

use std::num::NonZeroU32;

use spark_common::Time;
use spark_core::{Application, Stage};
use spark_ecs::{Event, Events};
use spark_input::{
    CursorMoved, FocusLost, KeyCode, KeyboardInput, MouseButton, MouseButtonInput, MouseWheel,
};
use tracing::{debug, info, trace};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{MouseButton as WinitMouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode as WinitKeyCode, PhysicalKey};
use winit::window::{Window, WindowAttributes, WindowId};

use crate::config::WindowConfig;
use crate::error::WindowError;

/// Pixels of scroll per `winit` `LineDelta` notch.
///
/// winit reports wheel movement two ways: `LineDelta` (whole notches, most
/// desktop mice) and `PixelDelta` (already pixels, precision trackpads). We
/// scale the former so both report the same magnitude for one notch. ~50
/// px/line matches common GUI toolkits (GTK, Qt) — a chosen convention, not a
/// hardware fact.
const SCROLL_LINES_TO_PIXELS: f32 = 50.0;

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
    };

    info!(
        version = spark_core::VERSION,
        "spark-window starting event loop"
    );
    event_loop.run_app(&mut runner)?;
    info!("spark-window event loop exited");

    Ok(())
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

/// Sends `event` into its [`Events<E>`] buffer — but only if some plugin has
/// registered that buffer with `add_event::<E>()` (normally `InputPlugin`).
///
/// The guard lets the window forward input without assuming a consumer is
/// wired: with no registered buffer the send is a silent no-op. The window
/// depends on `spark-input` only to *name* the event types it emits here; it
/// never reads the `KeyboardState` / `MouseState` they feed, so the dependency
/// points one way.
fn try_send<E: Event>(app: &Application, event: E) {
    if let Some(mut events) = app.world().get_resource_mut::<Events<E>>() {
        events.send(event);
    }
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

/// State held across calls into the OS event loop.
///
/// `app` is the owned [`Application`] — the runner ticks its stages on
/// every `RedrawRequested`. `window` is `Option` because winit only
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
    ///   Update → PostUpdate`, swapping event buffers in `Input`. `PreUpdate`
    ///   advances [`Time`] (which banks the fixed-timestep accumulator); the
    ///   runner then reads `Time::fixed_steps_this_frame()` to dispatch
    ///   `FixedUpdate` that many times, and requests the next redraw to keep
    ///   the loop alive.
    /// - Lifecycle / input events: logged at appropriate `tracing`
    ///   levels (cursor at `trace`, input at `debug`, focus/resize at
    ///   `info`). Keyboard, mouse-button, cursor, wheel, and focus-loss
    ///   events are *also* translated into [`spark_input`]'s event types
    ///   and forwarded into the world via [`try_send`] — guarded, so
    ///   they're a no-op unless something (normally `InputPlugin`)
    ///   registered the matching buffers.
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
                // Per-frame tick. Each stage flushes its pending commands at
                // the end of its run via `Application::run_stage`.
                //
                // `Input` runs first: it pumps the per-event swap systems
                // registered by `Application::add_event`, rotating each
                // `Events<T>` buffer so this frame's readers observe last
                // frame's sends before any other stage touches them.
                self.app.run_stage(Stage::Input);
                // `PreUpdate` runs `advance_time` (registered first by
                // `TimePlugin`): it samples the wall clock, advances `Time`, and
                // banks the fixed-timestep accumulator for this frame.
                self.app.run_stage(Stage::PreUpdate);

                // Fixed-timestep simulation: `Time` already computed how many
                // whole 1/60 s steps this frame's banked time covers — run
                // `FixedUpdate` exactly that many times. Zero on a fast frame,
                // several after a slow one, but never an unbounded burst (the
                // 250 ms clamp inside `Time::tick` caps it at ~15).
                let fixed_steps = self.app.world().resource::<Time>().fixed_steps_this_frame();
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
                // Losing focus: the OS delivers the matching key-up / button-up
                // events to whatever window took focus, not to us. Signal
                // consumers to clear held state so nothing appears stuck.
                if !focused {
                    try_send(&self.app, FocusLost);
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                info!(scale_factor, "window scale factor changed");
            }
            WindowEvent::KeyboardInput { event, .. } => {
                debug!(state = ?event.state, key = ?event.logical_key, "keyboard input");
                // Skip OS auto-repeat — held-state tracks edges, not repeats.
                if !event.repeat
                    && let PhysicalKey::Code(code) = event.physical_key
                    && let Some(key) = map_key(code)
                {
                    let pressed = event.state.is_pressed();
                    try_send(&self.app, KeyboardInput { key, pressed });
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                debug!(?state, ?button, "mouse input");
                let input = MouseButtonInput {
                    button: map_button(button),
                    pressed: state.is_pressed(),
                };
                try_send(&self.app, input);
            }
            WindowEvent::CursorMoved { position, .. } => {
                trace!(x = position.x, y = position.y, "cursor moved");
                try_send(
                    &self.app,
                    CursorMoved {
                        x: px(position.x),
                        y: px(position.y),
                    },
                );
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Normalize both winit delta shapes to pixels (see
                // `SCROLL_LINES_TO_PIXELS`) so devices report comparable magnitudes.
                let (x, y) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => {
                        (x * SCROLL_LINES_TO_PIXELS, y * SCROLL_LINES_TO_PIXELS)
                    }
                    MouseScrollDelta::PixelDelta(p) => (px(p.x), px(p.y)),
                };
                trace!(x, y, "mouse wheel");
                try_send(&self.app, MouseWheel { x, y });
            }
            _ => {}
        }
    }
}
