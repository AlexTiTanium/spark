//! Sandbox plugin — a small playground that demonstrates the ECS
//! surface end-to-end without opening a window.
//!
//! Module layout:
//!
//! - [`components`]: the four component types (`Position`, `Velocity`,
//!   `Health`, `Player`).
//! - [`resources`]: the single resource (`TickCount`).
//! - [`systems`]: the five demo systems, each exercising a distinct
//!   `SystemParam` shape.
//! - [`plugin`]: [`SandboxPlugin`] — seeds the world, registers the
//!   systems, installs the five-tick runner.
//!
//! What it shows end-to-end:
//!
//! - **Resources**: `TickCount` read via `Res<T>` and written via
//!   `ResMut<T>`.
//! - **Components**: four entities spawned with different combinations
//!   so joins have something to skip.
//! - **Query shapes**: single-component read (`Query<&T>`),
//!   single-component mutate (`Query<&mut T>`), two-tuple mut/read
//!   join (`Query<(&mut Position, &Velocity)>` — the canonical
//!   movement system), two-tuple shared join with a marker
//!   (`Query<(&Position, &Player)>`), and multiple queries inside one
//!   system.
//! - **Runner**: a tiny custom runner that drives the `UPDATE` stage
//!   five times and exits — proves `Application::set_runner` works.
//!
//! Run it: `cargo run -p spark`. Filter logs with
//! `RUST_LOG=spark=info` (or `=debug` for per-entity detail).

mod components;
mod plugin;
mod resources;
mod systems;

pub use plugin::SandboxPlugin;
