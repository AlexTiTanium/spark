//! Unit tests for the query layer, split by theme (issue #80 Phase 1).
//!
//! The shared component fixtures live here; each child module holds the
//! `#[test]` bodies for one theme, moved verbatim:
//! - [`data`] — single-component shapes, optional fetch, entity-as-data;
//! - [`joins`] — tuple joins, multi-mut, self-conflict, the `DenseMut` path;
//! - [`filters`] — `With` / `Without` / `And` / `Or`;
//! - [`change_detection`] — precise `Mut` change marking;
//! - [`driver_cost`] — kept whole; its exact driver-step counts are the
//!   codegen regression oracle, so they must not drift across the split.
#![allow(
    clippy::items_after_statements,
    clippy::needless_pass_by_value,
    reason = "test fns live next to their assertions; system fns take \
              `Query` by value to match how plugins write systems."
)]

use crate::{Component, Entity, World};

mod change_detection;
mod data;
mod driver_cost;
mod filters;
mod joins;

// Integer fields keep unit tests free of `clippy::float_cmp`
// assertions. Doc tests stay with the canonical `f32` flavour to
// read like real engine code.
#[derive(Debug, PartialEq, Component)]
struct Position(i32, i32);

#[derive(Debug, PartialEq, Component)]
struct Velocity(i32, i32);

#[derive(Debug, PartialEq, Component)]
struct Marker;

fn world_with_three_movers() -> (World, [Entity; 3]) {
    let mut world = World::new();
    let a = world
        .spawn()
        .insert(Position(0, 0))
        .insert(Velocity(1, 0))
        .id();
    let b = world
        .spawn()
        .insert(Position(10, 10))
        .insert(Velocity(0, 1))
        .id();
    let c = world
        .spawn()
        .insert(Position(20, 20))
        .insert(Velocity(1, 1))
        .id();
    (world, [a, b, c])
}

// Distinct unit-struct components for the higher-arity tuple tests.
// Plain `i32` newtypes so equality checks stay clippy-clean.
//
// These are independent test fixtures — *not* related to the
// `$first_flag $First, ...` macro variables in `impl_all_tuple!`.
#[derive(Debug, PartialEq, Component)]
struct A(i32);
#[derive(Debug, PartialEq, Component)]
struct B(i32);
#[derive(Debug, PartialEq, Component)]
struct C(i32);
#[derive(Debug, PartialEq, Component)]
struct D(i32);
#[derive(Debug, PartialEq, Component)]
struct E(i32);
