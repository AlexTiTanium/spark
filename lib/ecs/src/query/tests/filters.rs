use super::*;

use crate::filter::{And, Or, With, Without};
use crate::system::IntoSystem;
use crate::{Query, World};
// -------- Query filters: Query<D, F> --------

#[test]
fn query_with_filter_yields_only_entities_having_the_marker() {
    let mut world = World::new();
    world.spawn().insert(Position(1, 1)).insert(Marker);
    world.spawn().insert(Position(2, 2)); // no Marker
    world.spawn().insert(Position(3, 3)).insert(Marker);

    let q = Query::<&Position, With<Marker>>::from_world(&world);
    let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
    assert_eq!(xs.len(), 2);
    assert!(xs.contains(&1));
    assert!(xs.contains(&3));
    assert!(!xs.contains(&2));
}

#[test]
fn query_without_filter_excludes_entities_having_the_marker() {
    let mut world = World::new();
    world.spawn().insert(Position(1, 1)).insert(Marker);
    world.spawn().insert(Position(2, 2)); // no Marker
    let q = Query::<&Position, Without<Marker>>::from_world(&world);
    let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
    assert_eq!(xs, vec![2]);
}

#[test]
fn query_and_filter_requires_all_branches() {
    let mut world = World::new();
    // Velocity present, Marker absent — matches.
    world.spawn().insert(Position(1, 1)).insert(Velocity(0, 0));
    // Velocity present but Marker present — excluded by Without.
    world
        .spawn()
        .insert(Position(2, 2))
        .insert(Velocity(0, 0))
        .insert(Marker);
    // Velocity absent — excluded by With.
    world.spawn().insert(Position(3, 3));

    let q = Query::<&Position, And<(With<Velocity>, Without<Marker>)>>::from_world(&world);
    let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
    assert_eq!(xs, vec![1]);
}

#[test]
fn query_or_filter_matches_any_branch() {
    let mut world = World::new();
    world.spawn().insert(Position(1, 1)).insert(Velocity(0, 0)); // Velocity
    world.spawn().insert(Position(2, 2)).insert(Marker); // Marker
    world.spawn().insert(Position(3, 3)); // neither — excluded

    let q = Query::<&Position, Or<(With<Velocity>, With<Marker>)>>::from_world(&world);
    let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
    assert_eq!(xs.len(), 2);
    assert!(xs.contains(&1));
    assert!(xs.contains(&2));
}

#[test]
fn query_nested_and_of_or_filter_composes() {
    // Filter: With<Velocity> AND (With<Marker> OR With<A>).
    let mut world = World::new();
    // Velocity + Marker — matches.
    world
        .spawn()
        .insert(Position(1, 1))
        .insert(Velocity(0, 0))
        .insert(Marker);
    // Velocity + A — matches the Or via its other branch.
    world
        .spawn()
        .insert(Position(2, 2))
        .insert(Velocity(0, 0))
        .insert(A(0));
    // Velocity only — neither Or branch matches.
    world.spawn().insert(Position(3, 3)).insert(Velocity(0, 0));
    // Marker only, no Velocity — fails the outer And.
    world.spawn().insert(Position(4, 4)).insert(Marker);

    let q =
        Query::<&Position, And<(With<Velocity>, Or<(With<Marker>, With<A>)>)>>::from_world(&world);
    let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
    assert_eq!(xs.len(), 2);
    assert!(xs.contains(&1));
    assert!(xs.contains(&2));
}

#[test]
fn query_filter_applies_to_iter_mut() {
    let mut world = World::new();
    let marked = world.spawn().insert(Position(1, 1)).insert(Marker).id();
    let plain = world.spawn().insert(Position(2, 2)).id();
    {
        let mut q = Query::<&mut Position, With<Marker>>::from_world(&world);
        for mut p in q.iter_mut() {
            p.0 += 100;
        }
    }
    assert_eq!(world.get::<Position>(marked).unwrap().0, 101);
    assert_eq!(world.get::<Position>(plain).unwrap().0, 2); // untouched
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_mut_data_with_same_component_filter_panics() {
    // `With<Position>` reports a read of Position; combined with the
    // `&mut Position` data shape that's a write+read self-conflict,
    // caught at `from_world` before any borrow.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<&mut Position, With<Position>>::from_world(&world);
}

#[test]
fn query_mut_data_without_same_component_does_not_conflict() {
    // `Without<Position>` reports no access, so `&mut Position` data
    // + `Without<Position>` passes the self-conflict check. With no
    // Position entities the driver is empty and `matches` is never
    // called — it iterates cleanly to zero rather than panicking.
    let mut world = World::new();
    world.spawn().insert(Velocity(0, 0));
    let mut q = Query::<&mut Position, Without<Position>>::from_world(&world);
    assert_eq!(q.iter_mut().count(), 0);
}

#[test]
#[should_panic(expected = "already mutably borrowed")]
fn query_mut_data_without_same_component_panics_at_from_world() {
    // The flip side of the test above. When `Position`'s storage is
    // *non-empty*, `from_world` first takes a `RefMut` on its cell
    // (`D::init_state`), then `Without<Position>::init_state` tries a
    // shared borrow of the same cell — issue #65 moved filter-state
    // fetching into `from_world`, so the `RefCell` "already mutably
    // borrowed" panic now fires at *construction*, before iteration.
    // The query is nonsensical (it could never yield anything), but the
    // failure mode is exactly what `Without`'s no-access decision implies.
    //
    // REGRESSION GUARD — there is deliberately **no** `iter`/`iter_mut`
    // call below: the panic must come from `from_world` itself. If a
    // future refactor moves filter-state fetching back to a per-iter
    // local, construction stops borrowing the cell, this body no longer
    // panics, and `#[should_panic]` fails loudly — pinning the panic
    // *point*, not just the message. Do not add an `iter` call here.
    let mut world = World::new();
    world.spawn().insert(Position(1, 1));
    let _q = Query::<&mut Position, Without<Position>>::from_world(&world);
}

#[test]
fn filtered_query_wires_up_as_system_param() {
    // The `F` generic threads through `IntoSystem` like any other
    // part of the query type — the runner builds it via `fetch`.
    let mut world = World::new();
    world.spawn().insert(Position(1, 1)).insert(Marker);
    world.spawn().insert(Position(2, 2));
    fn bump_marked(mut q: Query<&mut Position, With<Marker>>) {
        for mut p in q.iter_mut() {
            p.0 += 10;
        }
    }
    let mut sys = IntoSystem::into_system(bump_marked);
    sys(&world);
    let xs: Vec<i32> = Query::<&Position>::from_world(&world)
        .iter()
        .map(|p| p.0)
        .collect();
    assert!(xs.contains(&11)); // marked entity moved
    assert!(xs.contains(&2)); // unmarked entity untouched
}
