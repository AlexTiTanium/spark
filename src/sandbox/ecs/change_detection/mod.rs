//! # Change detection, by example — "what changed since I last looked?"
//!
//! Imagine you're running the power grid in this game. Every tick, hundreds
//! of little numbers move: line loads, fuel levels, city demand. A lazy
//! system re-checks *all* of them every tick — wasteful, and it can't even
//! tell "this **just** happened" from "this has been true for a while".
//!
//! **Change detection** is how a system asks a sharper question. Tag a query
//! with a filter and it hands back only the entities you care about:
//!
//! - `Changed<T>` → only the ones whose `T` was written **since this system
//!   last ran**.
//! - `Added<T>` → only the ones that **just gained** a `T`.
//!
//! That's it. The rest of this folder teaches what you can build with those
//! two filters, as three short lessons — each a believable slice of the
//! energy game, so the behaviour falls out of a situation the game would
//! really hit.
//!
//! ## The three lessons
//!
//! 1. [`reacting`] — **react only to what changed.** The fundamentals on the
//!    simplest query (`&T`): `Changed` vs `Added`, why "changed" is relative
//!    to *the reader*, and why marking is *precise* (a battery bank whose
//!    "still charging" count decays to zero).
//! 2. [`across_components`] — **when one thing changes, update another.** Use
//!    a change as a trigger — watch one component, write a different one —
//!    and see the filter clip onto every query shape (single write, 3-way
//!    read, two-write join).
//! 3. [`combining`] — **react only when the right *combination* changed.**
//!    A `Changed<T>` filter is an ordinary filter, so it nests inside
//!    `With` / `Without` / `And` / `Or` like any other.
//!
//! ## How the data and behaviour are split
//!
//! This is an ECS, so the two halves live apart on purpose:
//!
//! - [`components`] holds the **data** — every `LineLoad`, `FuelLevel`,
//!   `CityDemand`, … the lessons use. Components are just values (or empty
//!   "flags"); they contain no logic.
//! - the three lesson files hold the **behaviour** — the systems that read,
//!   write, and react.
//! - [`scoreboard`] is the **honesty check**: a demo that claims to work
//!   should prove it, so every example counts its matches each frame and
//!   compares them to the number its doc comment predicts.
//!
//! ## Watch it run
//!
//! ```bash
//! RUST_LOG=spark=info cargo run -p spark
//! ```
//!
//! Each frame the scoreboard prints one line per example —
//! `scenario`, the exact `Query<…>`, `actual` vs `expected`, and `PASS` /
//! `FAIL` — then an `N/M PASS` tally. After a few frames everything has
//! settled, so it goes quiet, and only speaks up again if a count ever
//! diverges. The whole demo passing, live, *is* the proof.
//!
//! ## One rule worth remembering
//!
//! `Changed<T>` / `Added<T>` quietly count as **reading** `T`, so you can't
//! write the very same `T` you filter on (`Query<&mut T, Changed<T>>` is
//! refused at startup). Always watch one component and write a *different*
//! one — which is exactly what Lesson 2 is built around.

use spark_core::{Application, Plugin};

mod across_components;
mod combining;
mod components;
mod reacting;
mod scoreboard;

/// Wires the change-detection sub-sandbox into an [`Application`].
///
/// [`scoreboard::register`] goes first — it inserts the shared frame counter
/// and scoreboard and the systems that open each frame (`PreUpdate`) and
/// report it (`PostUpdate`). The three lessons then register their own seeds
/// (`Startup`) and per-frame writer + observer systems (`Update`).
///
/// Ordering only matters *within* a single example: each lesson adds an
/// example's writer before its observer, so the observer sees the write its
/// writer just made. Examples use disjoint component types, so the order
/// *between* them is irrelevant — and the `PreUpdate` frame-open and
/// `PostUpdate` report bracket all the `Update` work from their own stages.
///
/// The demo owns its frame / scoreboard resources, so it composes cleanly
/// under [`super::EcsSandboxPlugin`] without depending on the shared
/// `TickCount`.
pub(super) struct ChangeDetectionPlugin;

impl Plugin for ChangeDetectionPlugin {
    fn build(&self, app: &mut Application) {
        scoreboard::register(app);
        reacting::register(app);
        across_components::register(app);
        combining::register(app);
    }
}
