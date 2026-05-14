//! Spark binary entry point.
//!
//! # Summary
//!
//! Boot the engine and run the game. Today this initialises `tracing`,
//! logs the engine version, and opens an OS window via `spark-window`.
//! Closing the window terminates the process cleanly.
//!
//! # Logic
//!
//! Three calls, in order:
//!
//! 1. [`spark_window::init_tracing`] — install the global `tracing`
//!    subscriber so every `tracing::info!` macro in the engine produces
//!    a formatted line on stdout. Level filter respects `RUST_LOG`.
//! 2. `tracing::info!("Spark v{}", ...)` — proof that logging is live
//!    and `spark-core` is wired through.
//! 3. [`spark_window::run`] — opens the window and pumps the OS event
//!    loop until the user closes the window. Returns when the loop
//!    exits.
//!
//! In milestone M4 — once `spark-ecs` provides the canonical
//! `App` / `Plugin` traits (see `docs/ECS_DESIGN.md` stage 14) — this
//! becomes the textbook `App::new().add_plugin(WindowPlugin).run()`
//! shape. The free-function form is a one-PR migration target, not the
//! long-term API.
//!
//! # Why it works
//!
//! Keeping `main.rs` thin — pure wiring, zero logic — means a reader can
//! see the full set of engine plugins at a glance. Behaviour lives in
//! the plugins (or, today, in `spark-window`), not here.
//!
//! # How to use
//!
//! ```bash
//! cargo run                                 # build and run (debug)
//! cargo run --release                       # release profile
//! cargo run -p spark                        # unambiguous from a sub-crate
//! RUST_LOG=spark_window=debug cargo run -p spark  # verbose window logs
//! ```
//!
//! Close the window to exit. Status is 0 on a normal close, non-zero if
//! `spark_window::run` returns a [`spark_window::WindowError`].
//!
//! # How NOT to use
//!
//! - Do not put game logic in this file. Anything stateful belongs in a
//!   plugin under `src/game/plugins/` once those land.
//! - Do not call into engine internals directly; everything goes through
//!   `spark_window::run` (today) or `App` + `Plugin::build` (from M4).
//! - Do not call `init_tracing()` twice; it warns and otherwise does
//!   nothing, but the call is wasted.

fn main() -> Result<(), spark_window::WindowError> {
    spark_window::init_tracing();
    tracing::info!("Spark v{}", spark_core::VERSION);
    spark_window::run(
        spark_window::WindowConfig::default()
            .with_title("Spark")
            .with_size(1280, 720),
    )
}
