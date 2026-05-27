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
//! - [`driver_selection`]: demo systems showing that each query drives
//!   off the **smallest candidate set** (a tuple element, a filter, an
//!   `And` arm, or an `Or` union), over a deliberately skewed roster —
//!   cost ∝ result, not query shape (#65). Self-checks each count.
//! - [`change_detection`]: a self-contained sub-plugin
//!   (`ChangeDetectionPlugin`) that sweeps `Changed<T>` / `Added<T>`
//!   across every supported query-data shape and filter combination,
//!   self-checking each against its expected per-frame count.
//! - [`plugin`]: [`EcsSandboxPlugin`] — seeds the demo entities,
//!   registers the systems, and nests `ChangeDetectionPlugin`. Assumes
//!   [`crate::sandbox::SandboxPlugin`] has already inserted the shared
//!   resources.
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
//! - **Entity-as-data** — `Query<(Entity, &Position)>` (id surfaced
//!   alongside the component shape).
//! - **`Res<T>` + multiple `Query`s in one signature** — the
//!   per-tick summary system.
//!
//! Filters (in [`filters`]) — `With` / `Without` / `And<(…)>` /
//! `Or<(…)>`, nested combinators, a filter over a multi-component
//! shape, a filtered mutation, and **entity-as-data under a filter**
//! (`Query<Entity, With<Building>>`, `Query<(Entity, &Building),
//! With<Powered>>`). Each logs its expected vs actual matches; see the
//! module docs for the roster and outcome tables.
//!
//! Driver selection (in [`driver_selection`]) — the smallest candidate
//! drives every shape: a non-first tuple element
//! (`Query<(&Telemetry, &Critical)>`), a filter
//! (`Query<Entity, With<Critical>>`), an `And` arm, and a deduplicated
//! `Or` union. Each logs a `verdict` (PASS/FAIL) against the count the
//! skewed roster implies.

mod change_detection;
mod components;
mod driver_selection;
mod filters;
mod plugin;
mod systems;

pub use plugin::EcsSandboxPlugin;
