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
//! - [`components`]: ECS-local components — `Acceleration`, the
//!   `Player` marker, and the `Building` / `Capacity` + marker roster
//!   the filter demo uses. Shared spatial / gameplay components are at
//!   [`crate::sandbox::components`].
//! - [`systems`]: the data-shape demo systems, each exercising a
//!   distinct `Query<D>` shape.
//! - [`filters`]: the filter demo systems, each exercising a distinct
//!   `Query<D, F>` filter combination over a power-grid roster.
//! - [`plugin`]: [`EcsSandboxPlugin`] — seeds the demo entities,
//!   registers the systems. Assumes [`crate::sandbox::SandboxPlugin`]
//!   has already inserted the shared resources.
//!
//! # Query shapes exercised
//!
//! Data shapes (in [`systems`]):
//!
//! - **Single-component read** — `Query<&Position>` (initial-state
//!   report).
//! - **Single-component mutate** — `Query<&mut Health>` (decay).
//! - **Arity-2 shared join with a marker** — `Query<(&Position,
//!   &Player)>` (player position log).
//! - **Arity-3 multi-mut** — `Query<(&mut Position, &mut Velocity,
//!   &Acceleration)>` (symplectic Euler physics step).
//! - **Arity-2 mut-not-first** — `Query<(&Player, &mut Health)>`
//!   (player-only regen).
//! - **`Res<T>` + multiple `Query`s in one signature** — the
//!   per-tick summary system.
//!
//! Filters (in [`filters`]) — `With` / `Without` / `And<(…)>` /
//! `Or<(…)>`, nested combinators, a filter over a multi-component
//! shape, and a filtered mutation. Each logs its expected vs actual
//! matches; see the module docs for the roster and outcome tables.

mod change_detection;
mod components;
mod filters;
mod plugin;
mod systems;

pub use plugin::EcsSandboxPlugin;
