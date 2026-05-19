//! Sandbox plugins — playgrounds that demonstrate engine subsystems
//! end-to-end. Composed under a top-level [`SandboxPlugin`] that
//! adds the shared resources used by every sub-sandbox, then
//! registers each sub-sandbox plugin in turn.
//!
//! # Module layout
//!
//! - [`components`]: shared component types (`Position`, `Velocity`,
//!   `Health`). Reused by every sub-sandbox so spatial/gameplay
//!   primitives aren't redefined in three places.
//! - [`resources`]: shared resources (`TickCount`). Added once by
//!   [`SandboxPlugin`]; sub-sandbox systems consume them.
//! - [`plugin`]: [`SandboxPlugin`] — the composer. Adds shared
//!   resources, then registers sub-sandbox plugins.
//! - [`ecs`]: ECS-surface sub-sandbox ([`ecs::EcsSandboxPlugin`]) —
//!   demonstrates every supported `Query` and `SystemParam` shape.
//!
//! # Adding a new sub-sandbox (future)
//!
//! Mirror the `ecs/` folder layout, expose `<Name>SandboxPlugin`,
//! add one `.add_plugin(<Name>SandboxPlugin)` line inside
//! [`SandboxPlugin::build`]. The shared `components` and `resources`
//! are already in scope under `crate::sandbox::*`, so the new
//! sub-sandbox can reuse `Position` / `Velocity` / `Health` /
//! `TickCount` without redefining them.

pub mod ecs;

mod components;
mod plugin;
mod resources;

pub use plugin::SandboxPlugin;
