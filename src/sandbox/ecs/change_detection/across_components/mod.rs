//! # Lesson 2 — when one thing changes, update another
//!
//! Lesson 1 *watched* a component. The real power is using a change as a
//! **trigger**: "for every plant whose fuel dropped, write a refuel order."
//! The query watches one component and writes a *different* one.
//!
//! That "different" matters: a `Changed<T>` filter quietly counts as
//! *reading* `T`, so writing the very same `T` you filter on is refused at
//! startup. Watch one component, write a sibling — the natural design, and
//! what every example here does.
//!
//! The examples also show the change filter clipping onto **any** query
//! shape, in order:
//!
//! 1. [`refuel_dispatch`] — write a single `&mut` component.
//! 2. [`grid_solver`] — read three components at once (an arity-3 join).
//! 3. [`energy_toll`] — read one, write the next in the tuple.
//! 4. [`service_schedule`] — write one, read the next (order doesn't matter).
//! 5. [`substation_heat`] — write *two* components at once (a multi-mut join).
//! 6. [`transmission_segment`] — one read join watched with `Changed` *and*
//!    `Added`.

use spark_core::Application;

mod energy_toll;
mod grid_solver;
mod refuel_dispatch;
mod service_schedule;
mod substation_heat;
mod transmission_segment;

/// Registers Lesson 2's examples, in reading order.
pub(super) fn register(app: &mut Application) {
    refuel_dispatch::register(app);
    grid_solver::register(app);
    energy_toll::register(app);
    service_schedule::register(app);
    substation_heat::register(app);
    transmission_segment::register(app);
}
