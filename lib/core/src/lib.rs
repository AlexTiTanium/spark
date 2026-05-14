//! Foundation crate for the Spark engine.
//!
//! Sits at the bottom of the engine dependency graph (see `docs/PLAN.md`).
//! Every other engine crate (`spark-ecs`, `spark-window`, `spark-render`,
//! …) depends on this one, so it stays small and dep-free. Submodules
//! (`math`, `time`, `error`, `log`, `ids`) land here over M1–M2 as the
//! features that need them arrive.

/// Semantic version of `spark-core`, taken from `Cargo.toml` at compile
/// time via [`env!`].
///
/// # Examples
///
/// ```
/// assert!(!spark_core::VERSION.is_empty());
/// assert!(spark_core::VERSION.starts_with("0."));
/// ```
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
