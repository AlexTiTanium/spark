//! # Lesson 1 — react only to what changed
//!
//! The big idea: instead of re-checking *every* entity every tick, a system
//! asks the query for only the ones whose `T` was written **since this
//! system last ran** (`Changed<T>`) — or only the ones that **just gained**
//! a `T` (`Added<T>`).
//!
//! Four bite-size examples, one per file, best read in order:
//!
//! 1. [`line_telemetry`] — `Changed` keeps firing while something keeps
//!    writing.
//! 2. [`survey_cache`] — the *same* filter, but a value nobody rewrites goes
//!    quiet after the first look ("changed" is relative to the reader).
//! 3. [`plant_meter`] — `Changed` (reacts every tick) vs `Added` (reacts
//!    once), on the same plants.
//! 4. [`battery_bank`] — change marking is *precise*: only the cells you
//!    actually write count, so the "still charging" tally decays to zero.

use spark_core::Application;

mod battery_bank;
mod line_telemetry;
mod plant_meter;
mod survey_cache;

/// Registers Lesson 1's examples, in reading order.
pub(super) fn register(app: &mut Application) {
    line_telemetry::register(app);
    survey_cache::register(app);
    plant_meter::register(app);
    battery_bank::register(app);
}
