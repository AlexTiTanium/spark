//! Custom entity-component-system for the Spark engine.
//!
//! The current public surface is a single primitive: [`World`], a
//! type-erased container with exactly one verb — [`World::add_resource`].
//! Resources are singletons keyed by [`std::any::TypeId`]; one value per
//! type, last write wins. Read accessors (`get_resource`, `Res<T>`,
//! `ResMut<T>`) and the entity/component/query machinery land in later
//! milestones; the storage shape here is chosen so those can grow on top
//! additively (see `docs/ECS_DESIGN.md`).
//!
//! `spark-ecs` is the deepest crate in the engine dependency graph: pure
//! stdlib, no engine deps. `spark-core` depends on it because
//! [`spark_core::Application`] embeds a `World`.

mod world;

pub use world::World;
