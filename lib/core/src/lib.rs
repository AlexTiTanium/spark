//! Foundation crate for the Spark engine.
//!
//! # Summary
//!
//! `spark-core` sits at the bottom of the engine dependency graph
//! (see `docs/PLAN.md`). Every other engine crate — `spark-ecs`,
//! `spark-window`, `spark-render`, …  — depends on this one, so it
//! deliberately stays small and dependency-free.
//!
//! # Logic
//!
//! Today the crate is an empty shell whose only job is to establish the
//! workspace layout and the `spark-<module>` naming convention. Modules
//! land here over the next M1–M2 PRs in roughly this order:
//!
//! - `math`  — re-exports of `glam` types (`Vec2`, `Mat4`, …) tuned for 2D
//! - `time`  — fixed-timestep accumulator, `Time` resource, frame clock
//! - `error` — `thiserror`-based root error type for engine layers
//! - `log`   — `tracing` + `tracing-subscriber` init helper for binaries
//! - `ids`   — typed newtype handles (e.g. `EntityId`, `AssetId`) on
//!   top of generational indices
//!
//! Submodules will be added one at a time in their own PRs so each can be
//! reviewed (and documented) in isolation.
//!
//! # Why it works
//!
//! Anything that is "the same everywhere" — vector math, time, IDs, error
//! plumbing, logging setup — naturally belongs in a shared bottom crate.
//! Keeping the surface minimal here means upstream changes don't trigger
//! recompilation of the entire workspace.
//!
//! # How to use
//!
//! In another workspace crate's `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! spark-core = { path = "../core" }
//! ```
//!
//! Then in code:
//!
//! ```ignore
//! // Once submodules land, e.g.:
//! // use spark_core::time::Time;
//! // use spark_core::math::Vec2;
//! ```
//!
//! # How NOT to use
//!
//! - Do not import engine-layer types (window handles, render contexts,
//!   ECS internals) here — those belong in their own crates and would
//!   create a dependency cycle.
//! - Do not put game-specific types (cities, plants, workers) here; the
//!   game lives in `src/` and depends on the engine, not the other way
//!   around.
