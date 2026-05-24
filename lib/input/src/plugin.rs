//! [`InputPlugin`] — wires the input layer into an [`Application`].

use spark_core::{Application, Plugin, Stage};

use crate::collect::{collect_keyboard, collect_mouse_buttons, collect_mouse_motion};
use crate::event::{CursorMoved, FocusLost, KeyboardInput, MouseButtonInput, MouseWheel};
use crate::state::{KeyboardState, MouseState};

/// Registers the input events, the [`KeyboardState`] / [`MouseState`]
/// resources, and the three collection systems on [`Stage::Input`].
///
/// # Order matters, and it's self-contained
///
/// `build` calls every [`add_event`](Application::add_event) *before* its
/// [`add_system`](Application::add_system) calls. `add_event` registers each
/// event's buffer **and** its `swap_events` system on `Stage::Input`; doing
/// them first guarantees every swap runs before every `collect_*` in that
/// stage — so a collect reads buffers that were just rotated, giving input
/// state zero-frame latency. Because the ordering lives entirely inside this
/// one plugin, it holds no matter where `InputPlugin` sits in the `add_plugin`
/// chain.
///
/// `InputPlugin` owns the `Stage::Input` slot for input: it registers the
/// event swaps and the collectors there. Other plugins should treat that stage
/// as input's and not wedge first-slot systems into it.
///
/// # Independence
///
/// This plugin reads only the [`crate`] event types — it never references the
/// window. `spark-window` *emits* those events (guarded, so it's a no-op
/// without this plugin), but nothing here depends on it. That's also why the
/// systems are testable with synthetic events and no window at all.
///
/// # Examples
///
/// ```
/// use spark_core::Application;
/// use spark_input::{InputPlugin, KeyboardState};
///
/// let mut app = Application::new();
/// app.add_plugin(InputPlugin);
/// // The state resource is now present, ready for systems to read.
/// assert!(app.world().get_resource::<KeyboardState>().is_some());
/// ```
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut Application) {
        app.add_event::<KeyboardInput>()
            .add_event::<MouseButtonInput>()
            .add_event::<CursorMoved>()
            .add_event::<MouseWheel>()
            .add_event::<FocusLost>()
            .add_resource(KeyboardState::default())
            .add_resource(MouseState::default())
            .add_system(Stage::Input, collect_keyboard)
            .add_system(Stage::Input, collect_mouse_buttons)
            .add_system(Stage::Input, collect_mouse_motion);
    }
}
