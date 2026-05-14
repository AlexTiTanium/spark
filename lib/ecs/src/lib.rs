//! Custom entity-component-system for the Spark engine.
//!
//! Built in 24 incremental stages (see `docs/ECS_DESIGN.md`). The design
//! combines Bevy-style function-parameter system extraction with
//! Shipyard-inspired named `Workload`s, sparse-set component storage, and
//! a single-threaded scheduler (parallel execution via Rayon is Phase 3).
//!
//! Current stage: **1 — Entity + `EntityAllocator`**. Only `entity` is
//! public; all other modules land in later stages.

pub mod entity;
