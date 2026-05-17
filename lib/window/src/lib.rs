#![doc = include_str!("../README.md")]

mod config;
mod error;
mod event_loop;
mod plugin;

pub use config::WindowConfig;
pub use error::WindowError;
pub use event_loop::run;
pub use plugin::WindowPlugin;
