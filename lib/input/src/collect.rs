//! The `Stage::Input` systems that fold raw input events into state.
//!
//! Each runs after the event-buffer swap (see [`InputPlugin`](crate::InputPlugin)),
//! so it reads the events the window forwarded since last frame and applies
//! them to [`KeyboardState`] / [`MouseState`]. Keyboard is one system; the
//! mouse is split into two — folding all mouse input into one would need five
//! [`SystemParam`](spark_ecs::SystemParam)s (buttons, cursor, wheel, focus,
//! state), and `IntoSystem` tops out at four.
//!
//! All three follow the same shape: clear this frame's edge sets, react to
//! [`FocusLost`] by releasing everything held, then apply the per-event deltas.
//! The held sets persist across frames; the edge sets and the scroll delta do
//! not — they describe only the frame just collected.

use spark_ecs::{EventReader, ResMut};

use crate::event::{CursorMoved, FocusLost, KeyboardInput, MouseButtonInput, MouseWheel};
use crate::state::{KeyboardState, MouseState};

/// Folds [`KeyboardInput`] (and [`FocusLost`]) into [`KeyboardState`].
///
/// Clears the press/release edges, releases all held keys on focus loss, then
/// applies each key event. A press already in the held set is ignored (so OS
/// auto-repeat that slipped past the window filter can't double-register);
/// every genuine press/release records the matching edge.
#[allow(
    clippy::needless_pass_by_value,
    reason = "EventReader<T> / ResMut<T> are SystemParams — IntoSystem hands them in by \
              value; their by-reference forms are not SystemParams."
)]
pub(crate) fn collect_keyboard(
    keys: EventReader<KeyboardInput>,
    focus: EventReader<FocusLost>,
    mut kb: ResMut<KeyboardState>,
) {
    kb.just_pressed.clear();
    kb.just_released.clear();

    // Focus loss: the OS routed the key-ups elsewhere, so treat every held key
    // as released. `mem::take` empties `pressed` in one move (a `drain` loop
    // that pushed into another field wouldn't borrow-check through `ResMut`).
    for _ in focus.read() {
        let released = std::mem::take(&mut kb.pressed);
        kb.just_released.extend(released);
    }

    for event in keys.read() {
        if event.pressed {
            if !kb.pressed.contains(&event.key) {
                kb.pressed.push(event.key);
                kb.just_pressed.push(event.key);
            }
        } else if let Some(i) = kb.pressed.iter().position(|k| *k == event.key) {
            kb.pressed.swap_remove(i);
            kb.just_released.push(event.key);
        }
    }
}

/// Folds [`MouseButtonInput`] (and [`FocusLost`]) into [`MouseState`]'s button
/// fields. Mirrors [`collect_keyboard`] for buttons; leaves cursor and scroll
/// to [`collect_mouse_motion`].
#[allow(
    clippy::needless_pass_by_value,
    reason = "EventReader<T> / ResMut<T> are SystemParams — IntoSystem hands them in by \
              value; their by-reference forms are not SystemParams."
)]
pub(crate) fn collect_mouse_buttons(
    buttons: EventReader<MouseButtonInput>,
    focus: EventReader<FocusLost>,
    mut mouse: ResMut<MouseState>,
) {
    mouse.buttons_just_pressed.clear();
    mouse.buttons_just_released.clear();

    for _ in focus.read() {
        let released = std::mem::take(&mut mouse.buttons);
        mouse.buttons_just_released.extend(released);
    }

    for event in buttons.read() {
        if event.pressed {
            if !mouse.buttons.contains(&event.button) {
                mouse.buttons.push(event.button);
                mouse.buttons_just_pressed.push(event.button);
            }
        } else if let Some(i) = mouse.buttons.iter().position(|b| *b == event.button) {
            mouse.buttons.swap_remove(i);
            mouse.buttons_just_released.push(event.button);
        }
    }
}

/// Folds [`CursorMoved`] and [`MouseWheel`] into [`MouseState`]'s position and
/// scroll.
///
/// Cursor position is absolute and *persists* — the last move this frame wins,
/// and it is deliberately **not** cleared on focus loss (the cursor still has a
/// sensible last-known location). Scroll is a per-frame delta, so it resets to
/// `(0, 0)` every frame and accumulates this frame's wheel events.
#[allow(
    clippy::needless_pass_by_value,
    reason = "EventReader<T> / ResMut<T> are SystemParams — IntoSystem hands them in by \
              value; their by-reference forms are not SystemParams."
)]
pub(crate) fn collect_mouse_motion(
    cursor: EventReader<CursorMoved>,
    wheel: EventReader<MouseWheel>,
    mut mouse: ResMut<MouseState>,
) {
    mouse.scroll = (0.0, 0.0);
    for event in cursor.read() {
        mouse.position = (event.x, event.y);
    }
    for event in wheel.read() {
        mouse.scroll.0 += event.x;
        mouse.scroll.1 += event.y;
    }
}

#[cfg(test)]
mod tests {
    use spark_core::{Application, Stage};
    use spark_ecs::Events;

    use crate::event::{
        CursorMoved, FocusLost, KeyCode, KeyboardInput, MouseButton, MouseButtonInput, MouseWheel,
    };
    use crate::plugin::InputPlugin;
    use crate::state::{KeyboardState, MouseState};

    /// Fresh app with `InputPlugin` registered — the event buffers, state
    /// resources, and `Stage::Input` systems are all wired.
    fn app() -> Application {
        let mut app = Application::new();
        app.add_plugin(InputPlugin);
        app
    }

    /// Queues a synthetic event into its buffer, exactly as the window runner
    /// would before a frame.
    fn send<E: spark_ecs::Event>(app: &mut Application, event: E) {
        app.world_mut().resource_mut::<Events<E>>().send(event);
    }

    /// One `Stage::Input` pump: swaps the event buffers, then runs the
    /// `collect_*` systems — the same order the window runner uses per frame.
    fn tick(app: &mut Application) {
        app.run_stage(Stage::Input);
    }

    fn keyboard(app: &Application) -> std::cell::Ref<'_, KeyboardState> {
        app.world().resource::<KeyboardState>()
    }
    fn mouse(app: &Application) -> std::cell::Ref<'_, MouseState> {
        app.world().resource::<MouseState>()
    }

    #[test]
    fn key_press_sets_held_and_just_pressed() {
        let mut app = app();
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyW,
                pressed: true,
            },
        );
        tick(&mut app);
        let kb = keyboard(&app);
        assert!(kb.is_pressed(KeyCode::KeyW));
        assert!(kb.just_pressed(KeyCode::KeyW));
        assert!(!kb.just_released(KeyCode::KeyW));
    }

    #[test]
    fn key_release_clears_held_and_sets_just_released() {
        let mut app = app();
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyW,
                pressed: true,
            },
        );
        tick(&mut app);
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyW,
                pressed: false,
            },
        );
        tick(&mut app);
        let kb = keyboard(&app);
        assert!(!kb.is_pressed(KeyCode::KeyW));
        assert!(kb.just_released(KeyCode::KeyW));
        assert!(!kb.just_pressed(KeyCode::KeyW));
    }

    #[test]
    fn held_key_persists_and_edges_clear_next_frame() {
        let mut app = app();
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::Space,
                pressed: true,
            },
        );
        tick(&mut app);
        tick(&mut app); // a frame with no new events
        let kb = keyboard(&app);
        assert!(kb.is_pressed(KeyCode::Space), "still held");
        assert!(!kb.just_pressed(KeyCode::Space), "edge is one frame only");
    }

    #[test]
    fn duplicate_press_does_not_double_register() {
        let mut app = app();
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyA,
                pressed: true,
            },
        );
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyA,
                pressed: true,
            },
        );
        tick(&mut app);
        assert_eq!(
            keyboard(&app)
                .pressed()
                .filter(|k| *k == KeyCode::KeyA)
                .count(),
            1
        );
    }

    #[test]
    fn multiple_keys_tracked_independently() {
        let mut app = app();
        for key in [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyD] {
            send(&mut app, KeyboardInput { key, pressed: true });
        }
        tick(&mut app);
        // Next frame: only KeyW newly pressed; the others stay held but not "just".
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyW,
                pressed: false,
            },
        );
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyW,
                pressed: true,
            },
        );
        tick(&mut app);
        let kb = keyboard(&app);
        assert!(
            kb.is_pressed(KeyCode::KeyW)
                && kb.is_pressed(KeyCode::KeyA)
                && kb.is_pressed(KeyCode::KeyD)
        );
        assert!(kb.just_pressed(KeyCode::KeyW));
        assert!(!kb.just_pressed(KeyCode::KeyA));
    }

    #[test]
    fn focus_loss_releases_held_keys() {
        let mut app = app();
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyW,
                pressed: true,
            },
        );
        send(
            &mut app,
            KeyboardInput {
                key: KeyCode::KeyA,
                pressed: true,
            },
        );
        tick(&mut app);
        send(&mut app, FocusLost);
        tick(&mut app);
        let kb = keyboard(&app);
        assert!(!kb.is_pressed(KeyCode::KeyW) && !kb.is_pressed(KeyCode::KeyA));
        assert!(kb.just_released(KeyCode::KeyW) && kb.just_released(KeyCode::KeyA));
        assert_eq!(kb.pressed().count(), 0);
    }

    #[test]
    fn focus_loss_with_nothing_held_is_noop() {
        let mut app = app();
        send(&mut app, FocusLost);
        tick(&mut app);
        assert_eq!(keyboard(&app).pressed().count(), 0);
    }

    #[test]
    fn mouse_button_press_release_and_focus() {
        let mut app = app();
        send(
            &mut app,
            MouseButtonInput {
                button: MouseButton::Left,
                pressed: true,
            },
        );
        tick(&mut app);
        assert!(mouse(&app).is_pressed(MouseButton::Left));
        assert!(mouse(&app).just_pressed(MouseButton::Left));

        send(&mut app, FocusLost);
        tick(&mut app);
        let m = mouse(&app);
        assert!(!m.is_pressed(MouseButton::Left));
        assert!(m.just_released(MouseButton::Left));
    }

    #[test]
    fn cursor_position_updates_last_wins() {
        let mut app = app();
        send(&mut app, CursorMoved { x: 10.0, y: 20.0 });
        send(&mut app, CursorMoved { x: 30.0, y: 40.0 });
        tick(&mut app);
        assert_eq!(mouse(&app).position(), (30.0, 40.0));
    }

    #[test]
    fn cursor_position_persists_through_focus_loss() {
        let mut app = app();
        send(&mut app, CursorMoved { x: 50.0, y: 50.0 });
        tick(&mut app);
        send(&mut app, FocusLost);
        tick(&mut app);
        assert_eq!(
            mouse(&app).position(),
            (50.0, 50.0),
            "position must survive focus loss"
        );
    }

    #[test]
    fn scroll_accumulates_then_resets() {
        let mut app = app();
        send(&mut app, MouseWheel { x: 0.0, y: 1.0 });
        send(&mut app, MouseWheel { x: 0.0, y: 2.0 });
        tick(&mut app);
        assert_eq!(
            mouse(&app).scroll(),
            (0.0, 3.0),
            "deltas accumulate within a frame"
        );
        tick(&mut app); // no scroll this frame
        assert_eq!(mouse(&app).scroll(), (0.0, 0.0), "scroll resets each frame");
    }
}
