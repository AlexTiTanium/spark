#![doc = include_str!("../README.md")]
// `plugin::InputPlugin`, `state::KeyboardState`, … — the public names echo
// their modules, mirroring spark-core's convention.
#![allow(clippy::module_name_repetitions)]

mod collect;
mod event;
mod plugin;
mod press_set;
mod state;

pub use event::{
    CursorMoved, FocusLost, KeyCode, KeyboardInput, MouseButton, MouseButtonInput, MouseWheel,
};
pub use plugin::InputPlugin;
pub use state::{KeyboardState, MouseState};
