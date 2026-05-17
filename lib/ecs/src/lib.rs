#![doc = include_str!("../README.md")]

mod system;
mod world;

pub use system::{IntoSystem, Res, ResMut, SystemParam};
pub use world::World;
