//! # Lesson 3 — react only when the *right combination* changed
//!
//! Real control systems rarely react to a single bare change. They want
//! "cities that changed **and** are on the grid", or "sites where solar
//! **or** wind moved". A `Changed<T>` filter is an ordinary filter, so it
//! drops straight into the same combinators you'd use with `With` /
//! `Without`:
//!
//! - `And<(A, B)>` — every part must match.
//! - `Or<(A, B)>` — any part matches.
//! - `With<M>` / `Without<M>` — entity has / lacks a flag component.
//!
//! …and they nest and stretch (2-, 3-, 4-part tuples). The examples, in
//! order of increasing structure:
//!
//! 1. [`city_billing`] — `Changed` AND a `With` / `Without` flag (and its
//!    complement).
//! 2. [`hybrid_output`] — `Or` of two change sources.
//! 3. [`safety_interlock`] — `And` of two change sources.
//! 4. [`ops_dashboard`] — a nested `And<(With, Or<(Changed, Added)>)>`.
//! 5. [`hydro_dam`] — three-part `And` and `Or`.

use spark_core::Application;

mod city_billing;
mod hybrid_output;
mod hydro_dam;
mod ops_dashboard;
mod safety_interlock;

/// Registers Lesson 3's examples, in reading order.
pub(super) fn register(app: &mut Application) {
    city_billing::register(app);
    hybrid_output::register(app);
    safety_interlock::register(app);
    ops_dashboard::register(app);
    hydro_dam::register(app);
}
