//! ECS sub-sandbox — demonstrates the `spark-ecs` surface
//! end-to-end without owning a window.
//!
//! Composed under [`crate::sandbox::SandboxPlugin`] alongside any
//! future sub-sandboxes; shared resources (`TickCount`) and
//! components (`Position`, `Velocity`, `Health`) come from
//! [`crate::sandbox`], local ones live in [`components`].
//!
//! # Module layout
//!
//! - [`components`]: ECS-local components — `Acceleration` and the
//!   `Player` marker. Shared spatial / gameplay components are at
//!   [`crate::sandbox::components`].
//! - [`systems`]: the demo systems, each exercising a distinct
//!   `Query` shape.
//! - [`plugin`]: [`EcsSandboxPlugin`] — seeds the demo entities,
//!   registers the systems. Assumes [`crate::sandbox::SandboxPlugin`]
//!   has already inserted the shared resources.
//!
//! # Query shapes exercised
//!
//! - **Single-component read** — `Query<&Position>` (initial-state
//!   report).
//! - **Single-component mutate** — `Query<&mut Health>` (decay).
//! - **Arity-2 shared join with a marker** — `Query<(&Position,
//!   &Player)>` (player position log).
//! - **Arity-3 multi-mut** — `Query<(&mut Position, &mut Velocity,
//!   &Acceleration)>` (symplectic Euler physics step — *new shape*
//!   from the multi-mut PR).
//! - **Arity-2 mut-not-first** — `Query<(&Player, &mut Health)>`
//!   (player-only regen — *new shape* from the multi-mut PR).
//! - **`Res<T>` + multiple `Query`s in one signature** — the
//!   per-tick summary system.

mod components;
mod plugin;
mod systems;

pub use plugin::EcsSandboxPlugin;
