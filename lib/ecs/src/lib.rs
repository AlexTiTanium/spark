//! Custom entity-component-system for the Spark engine.
//!
//! The current public surface is two layers:
//!
//! - [`World`] — type-erased container that owns one value per
//!   [`TypeId`](std::any::TypeId), inserted with
//!   [`World::add_resource`] and read back through
//!   [`World::get_resource`] / [`World::resource`] (and `_mut`
//!   variants). Backed by [`RefCell`](std::cell::RefCell), so accessors
//!   take `&self`.
//!
//! - [`SystemParam`] / [`Res`] / [`ResMut`] / [`IntoSystem`] — the
//!   Bevy-style system-parameter machinery that lets a plain Rust fn
//!   describe what it reads and writes through its arguments. The
//!   engine wraps any such fn into a uniform `Box<dyn FnMut(&World)>`
//!   for storage on a stage.
//!
//! Entities, components, queries, and the multi-threaded scheduler
//! land in M3/M4 (see `docs/ECS_DESIGN.md`). The storage shape here is
//! chosen so those can grow on top additively.
//!
//! `spark-ecs` is the deepest crate in the engine dependency graph:
//! pure stdlib, no engine deps. `spark-core` depends on it because
//! [`spark_core::Application`] embeds a [`World`].

mod system;
mod world;

pub use system::{IntoSystem, Res, ResMut, SystemParam};
pub use world::World;
