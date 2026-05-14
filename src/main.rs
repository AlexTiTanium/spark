//! Spark binary entry point.
//!
//! # Summary
//!
//! Boot the engine and run the game. Today this is a stub that prints the
//! engine version and exits.
//!
//! # Logic
//!
//! As engine crates (`spark-core`, `spark-window`, `spark-input`, …) land,
//! this file grows into the canonical `App::new().add_plugin(...).run()`
//! shape from `docs/PLAN.md`. Right now it just dereferences
//! [`spark_core::VERSION`] to prove the workspace dependency graph is
//! wired up end-to-end.
//!
//! # Why it works
//!
//! Keeping `main.rs` thin — pure wiring, zero logic — means a reader can
//! see the full set of engine plugins at a glance. Behaviour lives in the
//! plugins, not here.
//!
//! # How to use
//!
//! ```bash
//! cargo run            # build and run the binary (debug)
//! cargo run --release  # release profile (optimised)
//! cargo run -p spark   # same, unambiguous from inside a sub-crate
//! ```
//!
//! # How NOT to use
//!
//! - Do not put game logic in this file. Anything stateful belongs in a
//!   plugin under `src/game/plugins/` once those land.
//! - Do not call into engine internals directly; everything goes through
//!   `App` and `Plugin::build` so the plugin list stays the only place
//!   that describes what the binary does.

fn main() {
    println!("Spark v{}", spark_core::VERSION);
}
