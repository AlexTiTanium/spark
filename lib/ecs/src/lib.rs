#![doc = include_str!("../README.md")]

mod entity;
mod query;
mod storage;
mod system;
mod world;

pub use entity::{Entity, EntityAllocator};
pub use query::{Query, QueryData, ReadOnlyQueryData};
pub use storage::{AnyStorage, Component, ComponentStorage};
pub use system::{IntoSystem, Res, ResMut, SystemParam};
pub use world::{EntityMut, World};
