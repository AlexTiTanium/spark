use std::cell::RefCell;
use std::rc::Rc;

use super::*;

use crate::filter::{Or, With, Without};
use crate::system::IntoSystem;
use crate::{Commands, Entity, Query, ReadOnlyQueryData, World};
#[test]
fn query_ref_single_component_walks_every_entity() {
    let (world, _) = world_with_three_movers();
    let q = Query::<&Position>::from_world(&world);
    assert_eq!(q.iter().count(), 3);
    let xs: Vec<i32> = q.iter().map(|p| p.0).collect();
    assert_eq!(xs, vec![0, 10, 20]);
}

#[test]
fn query_mut_single_component_mutates_in_place() {
    let (world, _) = world_with_three_movers();
    {
        let mut q = Query::<&mut Position>::from_world(&world);
        for mut p in q.iter_mut() {
            p.0 += 100;
        }
    }
    let after: Vec<i32> = Query::<&Position>::from_world(&world)
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(after, vec![100, 110, 120]);
}

#[test]
fn into_iter_ref_sugar_walks_every_entity() {
    let (world, _) = world_with_three_movers();
    let q = Query::<&Position>::from_world(&world);
    // `for … in &q` is the `IntoIterator for &Query` sugar — same
    // result as `q.iter()`, just without the explicit call.
    let mut xs = Vec::new();
    for p in &q {
        xs.push(p.0);
    }
    assert_eq!(xs, vec![0, 10, 20]);
}

#[test]
fn into_iter_mut_sugar_mutates_in_place() {
    let (world, _) = world_with_three_movers();
    {
        let mut q = Query::<&mut Position>::from_world(&world);
        // `for … in &mut q` is the `IntoIterator for &mut Query` sugar.
        for mut p in &mut q {
            p.0 += 100;
        }
    }
    let after: Vec<i32> = Query::<&Position>::from_world(&world)
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(after, vec![100, 110, 120]);
}

#[test]
fn query_two_tuple_yields_intersection_only() {
    let mut world = World::new();
    // Has both Position and Velocity — in the join.
    let _movers = [
        world
            .spawn()
            .insert(Position(0, 0))
            .insert(Velocity(1, 0))
            .id(),
        world
            .spawn()
            .insert(Position(5, 5))
            .insert(Velocity(0, 1))
            .id(),
    ];
    // Has only Position — must be skipped.
    let _lonely = world.spawn().insert(Position(99, 99)).id();
    // Has only Velocity — must be skipped.
    let _phantom = world.spawn().insert(Velocity(7, 7)).id();

    // Path B: the iterator yields data only, no `Entity`. Identify
    // each yielded pair by its values (every entity has unique
    // spawn data in this test).
    let q = Query::<(&Position, &Velocity)>::from_world(&world);
    let pairs: Vec<(Position, Velocity)> = q
        .iter()
        .map(|(p, v)| (Position(p.0, p.1), Velocity(v.0, v.1)))
        .collect();
    assert_eq!(pairs.len(), 2);
    assert!(pairs.contains(&(Position(0, 0), Velocity(1, 0))));
    assert!(pairs.contains(&(Position(5, 5), Velocity(0, 1))));
    // `lonely`'s Position(99, 99) is unjoined — its values must
    // not appear in the join output.
    assert!(!pairs.iter().any(|(p, _)| p == &Position(99, 99)));
}

// -------- Optional fetch: Option<&T> / Option<&mut T> (issue #70) -----

/// A join with an optional second element keeps every row the required
/// element drives — present rows carry `Some`, absent rows carry `None`.
#[test]
fn query_optional_ref_yields_some_when_present_none_when_absent() {
    let mut world = World::new();
    let both = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(2, 0))
        .id();
    let lonely = world.spawn().insert(Position(3, 0)).id(); // no Velocity

    let q = Query::<(&Position, Option<&Velocity>)>::from_world(&world);
    let rows: Vec<(i32, Option<i32>)> = q.iter().map(|(p, v)| (p.0, v.map(|v| v.0))).collect();

    assert_eq!(rows.len(), 2); // both rows kept — Option never gates
    assert!(rows.contains(&(1, Some(2)))); // `both` carries Velocity
    assert!(rows.contains(&(3, None))); // `lonely` is still yielded
    let _ = (both, lonely);
}

/// Standing alone, `Query<Option<&T>>` visits every live entity, with
/// or without `T` — the same no-candidate / live-set path as
/// `Query<Entity>`.
#[test]
fn query_optional_standalone_visits_every_live_entity() {
    let mut world = World::new();
    world.spawn().insert(Velocity(2, 0));
    world.spawn().insert(Position(1, 0)); // no Velocity
    world.spawn(); // no components at all

    let q = Query::<Option<&Velocity>>::from_world(&world);
    assert_eq!(q.iter().count(), 3); // every live entity, with or without Velocity
    assert_eq!(q.iter().flatten().count(), 1); // exactly one carries Velocity
}

/// Standalone mutable mirror: `Query<Option<&mut T>>` visits every live
/// entity (snapshot-driven) and writes through the present ones — a
/// distinct branch from the tuple-trailing `Option<&mut T>` cases.
#[test]
fn query_optional_standalone_mut_visits_all_and_writes_through() {
    let mut world = World::new();
    let has_v = world.spawn().insert(Velocity(5, 0)).id();
    world.spawn().insert(Position(1, 0)); // no Velocity
    world.spawn(); // no components

    {
        let mut q = Query::<Option<&mut Velocity>>::from_world(&world);
        assert_eq!(q.iter_mut().count(), 3); // every live entity
        for mut vel in q.iter_mut().flatten() {
            vel.0 = 99; // write-through on the present one only
        }
    }
    assert_eq!(world.get::<Velocity>(has_v).unwrap().0, 99);
}

/// `Option<&mut T>` writes through on entities that have `T`, and leaves
/// the rest untouched while still yielding them.
#[test]
fn query_optional_mut_writes_through_present_only() {
    let mut world = World::new();
    let poisoned = world
        .spawn()
        .insert(Position(100, 0))
        .insert(Velocity(5, 0))
        .id();
    let clean = world.spawn().insert(Position(50, 0)).id(); // no Velocity

    let mut yielded = 0;
    {
        let mut q = Query::<(&mut Position, Option<&mut Velocity>)>::from_world(&world);
        for (mut pos, vel) in q.iter_mut() {
            yielded += 1;
            if let Some(mut v) = vel {
                pos.0 -= v.0; // apply…
                v.0 = 0; // …and consume
            }
        }
    }
    assert_eq!(yielded, 2); // both rows visited
    assert_eq!(world.get::<Position>(poisoned).unwrap().0, 95); // 100 - 5
    assert_eq!(world.get::<Velocity>(poisoned).unwrap().0, 0); // consumed
    assert_eq!(world.get::<Position>(clean).unwrap().0, 50); // untouched
}

/// `Option<&T>` is read-only, so an all-read optional shape implements
/// `ReadOnlyQueryData` and `.iter()` is available.
#[test]
fn query_optional_read_shape_is_read_only() {
    fn assert_read_only<D: ReadOnlyQueryData>() {}
    assert_read_only::<(&Position, Option<&Velocity>)>();
    assert_read_only::<Option<&Position>>();
}

/// The optional reports its access, so `(&A, Option<&mut A>)` trips the
/// per-query self-conflict check — a write+read conflict on `Position`,
/// caught at construction before any storage is borrowed. (Issue #70 pins
/// `(Option<&mut A>, &A)`; optional-first doesn't compile under the
/// required-first rule, so this is the equivalent reachable shape.)
#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_optional_mut_self_conflict_panics() {
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(&Position, Option<&mut Position>)>::from_world(&world);
}

/// Driver and trailing optional both *write* the same component
/// (`(&mut A, Option<&mut A>)`) — the write+write branch of the conflict
/// check, distinct from the write+read case above; both reach it through
/// the `O`/`OW` arms of `access_call!`.
#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_optional_mut_driver_and_opt_both_write_same_type_panics() {
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(&mut Position, Option<&mut Position>)>::from_world(&world);
}

/// Arity-3 with a trailing optional: the required intersection drives,
/// the optional rides along as `Some`/`None`.
#[test]
fn query_optional_arity_three_required_intersection_plus_optional() {
    let mut world = World::new();
    // Has Position + Velocity + Marker.
    world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(1, 0))
        .insert(Marker);
    // Has Position + Velocity, no Marker.
    world.spawn().insert(Position(2, 0)).insert(Velocity(2, 0));
    // Has only Position — excluded (Velocity is required).
    world.spawn().insert(Position(3, 0));

    let q = Query::<(&Position, &Velocity, Option<&Marker>)>::from_world(&world);
    let rows: Vec<(i32, bool)> = q.iter().map(|(p, _v, m)| (p.0, m.is_some())).collect();

    assert_eq!(rows.len(), 2); // only the Position∩Velocity intersection
    assert!(rows.contains(&(1, true))); // Marker present
    assert!(rows.contains(&(2, false))); // Marker absent — still yielded
    assert!(!rows.iter().any(|(x, _)| *x == 3)); // no Velocity → excluded
}

/// The optional's *storage* never existing (no entity ever had it) is a
/// distinct branch from "this entity lacks it": `init_state` returns
/// `None`, so every lookup yields `None` — without panicking.
#[test]
fn query_optional_ref_absent_storage_yields_all_none() {
    let mut world = World::new();
    world.spawn().insert(Position(1, 0));
    world.spawn().insert(Position(2, 0));
    // Velocity storage was never created.
    let q = Query::<(&Position, Option<&Velocity>)>::from_world(&world);
    let rows: Vec<(i32, bool)> = q.iter().map(|(p, v)| (p.0, v.is_some())).collect();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, has_v)| !has_v)); // None for every row
}

/// Same absent-storage branch for the mutable optional (`view` is `None`).
#[test]
fn query_optional_mut_absent_storage_yields_all_none() {
    let mut world = World::new();
    world.spawn().insert(Position(10, 0));
    // Velocity storage was never created.
    let mut q = Query::<(&mut Position, Option<&mut Velocity>)>::from_world(&world);
    let mut count = 0;
    for (pos, vel) in q.iter_mut() {
        assert_eq!(pos.0, 10); // the required driver still yields its value
        assert!(vel.is_none());
        count += 1;
    }
    assert_eq!(count, 1);
}

/// Standalone `Query<Option<&T>>` on an empty world drives an empty
/// snapshot — zero rows, no panic.
#[test]
fn query_optional_standalone_on_empty_world_yields_nothing() {
    let world = World::new();
    let q = Query::<Option<&Velocity>>::from_world(&world);
    assert_eq!(q.iter().count(), 0);
}

/// An optional composes with a filter: the filter narrows the set, the
/// optional rides along on whatever survives.
#[test]
fn query_optional_with_filter_narrows_by_filter_optional_rides_along() {
    let mut world = World::new();
    // Marker + Velocity → kept, Velocity Some.
    world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(10, 0))
        .insert(Marker);
    // Marker, no Velocity → kept, Velocity None.
    world.spawn().insert(Position(2, 0)).insert(Marker);
    // Velocity but no Marker → excluded by the filter.
    world.spawn().insert(Position(3, 0)).insert(Velocity(30, 0));

    let q = Query::<(&Position, Option<&Velocity>), With<Marker>>::from_world(&world);
    let rows: Vec<(i32, Option<i32>)> = q.iter().map(|(p, v)| (p.0, v.map(|v| v.0))).collect();

    assert_eq!(rows.len(), 2); // only the two Marker-holders
    assert!(rows.contains(&(1, Some(10))));
    assert!(rows.contains(&(2, None)));
    assert!(!rows.iter().any(|(x, _)| *x == 3)); // no Marker → excluded
}

/// `Without` offers no candidate, so the data element drives (Data(0)) and
/// the filter narrows per entity via `matches` — distinct from the `With`
/// case where the filter can drive. The optional still rides along.
#[test]
fn query_optional_without_filter_data_drives_optional_rides() {
    let mut world = World::new();
    // No Marker → kept; Velocity Some.
    world.spawn().insert(Position(1, 0)).insert(Velocity(10, 0));
    // No Marker, no Velocity → kept; Velocity None.
    world.spawn().insert(Position(2, 0));
    // Has Marker → excluded by Without<Marker>.
    world
        .spawn()
        .insert(Position(3, 0))
        .insert(Velocity(30, 0))
        .insert(Marker);

    let q = Query::<(&Position, Option<&Velocity>), Without<Marker>>::from_world(&world);
    let rows: Vec<(i32, Option<i32>)> = q.iter().map(|(p, v)| (p.0, v.map(|v| v.0))).collect();

    assert_eq!(rows.len(), 2); // the two Marker-free entities
    assert!(rows.contains(&(1, Some(10))));
    assert!(rows.contains(&(2, None)));
    assert!(!rows.iter().any(|(x, _)| *x == 3)); // has Marker → excluded
}

/// An `Or` filter that wins driver selection feeds entities through the
/// `DriveSource::External` path. This pins that the optional resolves to
/// the correct `Some`/`None` *value* under external drive — the existing
/// driver-cost test only counts steps, not values.
#[test]
fn query_optional_or_filter_external_drive_resolves_values() {
    let mut world = World::new();
    // Matches via the Velocity arm only → Marker None.
    world.spawn().insert(Position(1, 0)).insert(Velocity(1, 0));
    // Matches via the Marker arm only → Marker Some.
    world.spawn().insert(Position(2, 0)).insert(Marker);
    // Matches both arms → Marker Some, visited once.
    world
        .spawn()
        .insert(Position(3, 0))
        .insert(Velocity(3, 0))
        .insert(Marker);
    // Matches neither → excluded.
    world.spawn().insert(Position(4, 0));

    let q = Query::<(&Position, Option<&Marker>), Or<(With<Velocity>, With<Marker>)>>::from_world(
        &world,
    );
    let rows: Vec<(i32, bool)> = q.iter().map(|(p, m)| (p.0, m.is_some())).collect();

    assert_eq!(rows.len(), 3); // the Or union, deduplicated
    assert!(rows.contains(&(1, false))); // Velocity arm — Marker absent
    assert!(rows.contains(&(2, true))); // Marker arm — Marker present
    assert!(rows.contains(&(3, true))); // both arms — present, once
    assert!(!rows.iter().any(|(x, _)| *x == 4));
}

/// Mut driver + read optional (`(&mut A, Option<&B>)`) — a different macro
/// path from `(&mut A, Option<&mut B>)`. The driver writes through; the
/// optional read rides along.
#[test]
fn query_optional_read_with_mut_driver_writes_through() {
    let mut world = World::new();
    let mover = world
        .spawn()
        .insert(Position(5, 0))
        .insert(Velocity(3, 0))
        .id();
    let still = world.spawn().insert(Position(9, 0)).id(); // no Velocity

    {
        let mut q = Query::<(&mut Position, Option<&Velocity>)>::from_world(&world);
        for (mut pos, vel) in q.iter_mut() {
            if let Some(v) = vel {
                pos.0 += v.0;
            }
        }
    }
    assert_eq!(world.get::<Position>(mover).unwrap().0, 8); // 5 + 3
    assert_eq!(world.get::<Position>(still).unwrap().0, 9); // untouched
}

/// Arity-3 with TWO trailing optionals — both `O` positions exercised
/// together; the required first element drives all rows.
#[test]
fn query_optional_arity_three_two_trailing_optionals() {
    let mut world = World::new();
    world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(10, 0))
        .insert(Marker); // both Some
    world.spawn().insert(Position(2, 0)).insert(Velocity(20, 0)); // Velocity Some, Marker None
    world.spawn().insert(Position(3, 0)); // both None

    let q = Query::<(&Position, Option<&Velocity>, Option<&Marker>)>::from_world(&world);
    let rows: Vec<(i32, Option<i32>, bool)> = q
        .iter()
        .map(|(p, v, m)| (p.0, v.map(|v| v.0), m.is_some()))
        .collect();

    assert_eq!(rows.len(), 3); // all three — Position is the required driver
    assert!(rows.contains(&(1, Some(10), true)));
    assert!(rows.contains(&(2, Some(20), false)));
    assert!(rows.contains(&(3, None, false)));
}

/// Writing through `Option<&mut T>` marks the component changed (via the
/// same `Mut` guard as `&mut T`); a present-but-unwritten row does not.
#[test]
fn query_optional_mut_marks_changed_only_when_written() {
    let mut world = World::new();
    let written = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(5, 0))
        .id();
    let untouched = world
        .spawn()
        .insert(Position(2, 0))
        .insert(Velocity(7, 0))
        .id();
    // Advance Velocity's clock past both inserts (a third Velocity bumps
    // it), so a write later stamps a STRICTLY higher tick. Without this the
    // write would land on the same tick as the last insert and the
    // assertion below would pass only by coincidence of insertion order.
    world.spawn().insert(Velocity(0, 0));
    // Capture the pre-write marks (after the bump).
    let before_written = world
        .storage::<Velocity>()
        .unwrap()
        .changed_tick_for(written);
    let before_untouched = world
        .storage::<Velocity>()
        .unwrap()
        .changed_tick_for(untouched);

    {
        let mut q = Query::<(&Position, Option<&mut Velocity>)>::from_world(&world);
        for (pos, vel) in q.iter_mut() {
            if pos.0 == 1
                && let Some(mut v) = vel
            {
                v.0 = 99; // DerefMut → marks Velocity changed for `written`
            }
            // `untouched`'s Velocity is Some but never written through.
        }
    }
    let velo = world.storage::<Velocity>().unwrap();
    assert!(velo.changed_tick_for(written) > before_written); // marked
    assert_eq!(velo.changed_tick_for(untouched), before_untouched); // untouched
}

/// Arity-3 with TWO trailing *mutable* optionals — exercises the
/// `build_elem!(OW, …)` + `non_driver_lookup!(OW, …)` `DenseMut` lookup for
/// two optional positions at once, with write-through on the present one.
#[test]
fn query_optional_arity_three_two_trailing_mut_optionals() {
    let mut world = World::new();
    let both = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(10, 0))
        .insert(Marker)
        .id();
    let vel_only = world
        .spawn()
        .insert(Position(2, 0))
        .insert(Velocity(20, 0))
        .id();
    let neither = world.spawn().insert(Position(3, 0)).id();

    let mut markers_seen = 0;
    {
        let mut q =
            Query::<(&mut Position, Option<&mut Velocity>, Option<&mut Marker>)>::from_world(
                &world,
            );
        for (mut pos, vel, marker) in q.iter_mut() {
            pos.0 += 100; // required driver writes through
            if marker.is_some() {
                markers_seen += 1;
            }
            if let Some(mut v) = vel {
                v.0 += 1; // OW write-through
            }
        }
    }
    assert_eq!(markers_seen, 1); // only `both` has a Marker
    assert_eq!(world.get::<Position>(both).unwrap().0, 101); // driver wrote all 3
    assert_eq!(world.get::<Position>(vel_only).unwrap().0, 102);
    assert_eq!(world.get::<Position>(neither).unwrap().0, 103);
    assert_eq!(world.get::<Velocity>(both).unwrap().0, 11); // OW wrote the present ones
    assert_eq!(world.get::<Velocity>(vel_only).unwrap().0, 21);
}

// -------- entity-as-data: Query<Entity> / Query<(Entity, …)> --------

#[test]
fn query_entity_yields_every_live_entity_including_componentless() {
    let mut world = World::new();
    let a = world.spawn().insert(Position(0, 0)).id();
    let b = world.spawn().id(); // no components at all
    let c = world.spawn().insert(Velocity(1, 1)).id();

    let ids: Vec<Entity> = Query::<Entity>::from_world(&world).iter().collect();
    assert_eq!(ids.len(), 3);
    assert!(ids.contains(&a));
    assert!(ids.contains(&b)); // yielded despite holding no components
    assert!(ids.contains(&c));
}

#[test]
fn query_entity_excludes_already_despawned_entity() {
    let (mut world, [a, b, c]) = world_with_three_movers();
    world.despawn(b);
    let ids: Vec<Entity> = Query::<Entity>::from_world(&world).iter().collect();
    assert_eq!(ids, vec![a, c]); // slot-index order; b's slot is free
}

#[test]
fn query_entity_with_filter_keeps_only_matching() {
    let mut world = World::new();
    let m1 = world.spawn().insert(Position(1, 1)).insert(Marker).id();
    let _plain = world.spawn().insert(Position(2, 2)).id(); // no Marker
    let m2 = world.spawn().insert(Marker).id(); // Marker, no Position

    // Entity drives off the live snapshot, then `With<Marker>` filters
    // per entity — correct even though the marker isn't in the item.
    let ids: Vec<Entity> = Query::<Entity, With<Marker>>::from_world(&world)
        .iter()
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&m1));
    assert!(ids.contains(&m2));
}

#[test]
fn query_entity_position_tuple_yields_id_and_component() {
    let (world, [a, b, c]) = world_with_three_movers();
    let mut pairs: Vec<(Entity, i32)> = Query::<(Entity, &Position)>::from_world(&world)
        .iter()
        .map(|(e, p)| (e, p.0))
        .collect();
    pairs.sort_by_key(|(_, x)| *x);
    assert_eq!(pairs, vec![(a, 0), (b, 10), (c, 20)]);
}

#[test]
fn query_entity_mut_tuple_writes_through_and_reports_id() {
    let (world, [a, _, _]) = world_with_three_movers();
    {
        let mut q = Query::<(Entity, &mut Position)>::from_world(&world);
        for (e, mut p) in q.iter_mut() {
            if e == a {
                p.0 = 999;
            }
        }
    }
    assert_eq!(world.get::<Position>(a).unwrap().0, 999);
}

#[test]
fn query_entity_arity_two_tuple_yields_id_and_both_components() {
    let (world, _) = world_with_three_movers();
    let rows: Vec<(Entity, i32, i32)> = Query::<(Entity, &Position, &Velocity)>::from_world(&world)
        .iter()
        .map(|(e, p, v)| (e, p.0, v.0))
        .collect();
    assert_eq!(rows.len(), 3);
    // The id in each row owns the components it's paired with.
    for (e, px, _) in rows {
        assert_eq!(world.get::<Position>(e).unwrap().0, px);
    }
}

#[test]
fn query_entity_tuple_skips_entity_missing_a_component() {
    let mut world = World::new();
    let full = world
        .spawn()
        .insert(Position(1, 1))
        .insert(Velocity(2, 2))
        .id();
    let _pos_only = world.spawn().insert(Position(3, 3)).id();
    // Position drives; the Velocity-less entity is dropped by the join.
    let ids: Vec<Entity> = Query::<(Entity, &Position, &Velocity)>::from_world(&world)
        .iter()
        .map(|(e, _, _)| e)
        .collect();
    assert_eq!(ids, vec![full]);
}

#[test]
fn query_entity_mixed_mut_tuple_writes_only_through_mut() {
    let (world, _) = world_with_three_movers();
    {
        // Position drives (mut), Velocity is a read non-driver.
        let mut q = Query::<(Entity, &mut Position, &Velocity)>::from_world(&world);
        for (_e, mut p, v) in q.iter_mut() {
            p.0 += v.0;
        }
    }
    let xs: Vec<i32> = Query::<&Position>::from_world(&world)
        .iter()
        .map(|p| p.0)
        .collect();
    // a: 0+1, b: 10+0, c: 20+1
    assert_eq!(xs, vec![1, 10, 21]);
}

#[test]
fn query_entity_tuple_as_system_param_via_into_system() {
    let (world, [a, _, _]) = world_with_three_movers();
    fn step(mut q: Query<(Entity, &mut Position)>) {
        for (_e, mut p) in q.iter_mut() {
            p.0 += 1;
        }
    }
    let mut sys = IntoSystem::into_system(step);
    sys(&world);
    assert_eq!(world.get::<Position>(a).unwrap().0, 1);
}

#[test]
fn query_entity_tuple_for_in_ref_sugar_yields_id_and_component() {
    let (world, _) = world_with_three_movers();
    let q = Query::<(Entity, &Position)>::from_world(&world);
    let mut count = 0;
    for (e, p) in &q {
        // The yielded id owns the yielded component.
        assert_eq!(world.get::<Position>(e).unwrap().0, p.0);
        count += 1;
    }
    assert_eq!(count, 3);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_entity_tuple_self_conflict_panics_naming_component() {
    // Entity is invisible to the conflict check; the write+read of
    // Position still collides and the panic names Position.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(Entity, &mut Position, &Position)>::from_world(&world);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_entity_mut_with_same_component_filter_panics() {
    // `With<Position>` reports a read; the `&mut Position` element
    // writes it. Entity contributes nothing, so the conflict stands.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(Entity, &mut Position), With<Position>>::from_world(&world);
}

#[test]
fn query_entity_coexists_with_commands_and_snapshot_excludes_new_spawn() {
    // The snapshot (taken when the query is fetched, before the body
    // runs) is what lets `Query<Entity>` and `Commands` share a
    // signature: the Vec releases the allocator borrow, so the later
    // `cmd.spawn()`'s `borrow_mut` doesn't panic. The spawned id
    // postdates the snapshot, so it is not yielded this frame.
    //
    // `IntoSystem` requires a `'static` body, so the observed state is
    // shared into the closure via owned `Rc` clones rather than borrows.
    let mut world = World::new();
    let pre = world.spawn().id();

    let seen = Rc::new(RefCell::new(Vec::<Entity>::new()));
    let spawned = Rc::new(RefCell::new(None));
    {
        let seen = Rc::clone(&seen);
        let spawned = Rc::clone(&spawned);
        let observe = move |q: Query<Entity>, mut cmd: Commands| {
            let fresh = cmd.spawn().id(); // allocated synchronously…
            *spawned.borrow_mut() = Some(fresh);
            for id in q.iter() {
                // …but the query walks the construction-time snapshot.
                seen.borrow_mut().push(id);
            }
        };
        let mut sys = IntoSystem::into_system(observe);
        sys(&world); // no "already borrowed" panic
    }
    world.flush_commands();

    let seen = seen.borrow();
    let spawned = spawned.borrow().unwrap();
    assert_eq!(*seen, vec![pre]);
    assert!(!seen.contains(&spawned));
    assert!(world.is_alive(spawned)); // it really was created
}

#[test]
fn query_entity_snapshot_still_yields_commands_despawned_entity() {
    // `Commands::despawn` is deferred to the next flush, so the doomed
    // entity stays alive — and in the snapshot — for the rest of the
    // frame, and is still yielded.
    let mut world = World::new();
    let doomed = world.spawn().id();
    let other = world.spawn().id();

    let seen = Rc::new(RefCell::new(Vec::<Entity>::new()));
    {
        let seen = Rc::clone(&seen);
        let observe = move |q: Query<Entity>, mut cmd: Commands| {
            cmd.despawn(doomed);
            for id in q.iter() {
                seen.borrow_mut().push(id);
            }
        };
        let mut sys = IntoSystem::into_system(observe);
        sys(&world);
    }
    {
        let captured = seen.borrow();
        assert!(captured.contains(&doomed)); // deferred → still yielded
        assert!(captured.contains(&other));
    }
    assert!(world.is_alive(doomed)); // not gone until flush

    world.flush_commands();
    assert!(!world.is_alive(doomed));
}

#[test]
fn query_entity_on_empty_world_yields_nothing() {
    // The snapshot path must cope with zero live entities — an empty
    // `Vec`, not a panic.
    let world = World::new();
    assert_eq!(Query::<Entity>::from_world(&world).iter().count(), 0);
}

#[test]
fn query_entity_fully_mut_tuple_writes_through_both_components() {
    // `(Entity, &mut A, &mut B)` — the all-mut entity tuple. The
    // non-driver `&mut B` goes through `DenseMut` under an entity
    // prefix, a path no other entity test exercises.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(1, 2))
        .insert(Velocity(10, 20))
        .id();
    {
        let mut q = Query::<(Entity, &mut Position, &mut Velocity)>::from_world(&world);
        for (id, mut p, mut v) in q.iter_mut() {
            assert_eq!(id, e);
            p.0 += v.0; // 1 + 10 = 11
            v.0 = 0;
        }
    }
    assert_eq!(world.get::<Position>(e).unwrap().0, 11);
    assert_eq!(world.get::<Velocity>(e).unwrap().0, 0);
}

#[test]
fn query_entity_arity_three_component_read_tuple_joins_and_skips() {
    // `(Entity, &A, &B, &C)` — the widest entity tuple, and the only
    // shape that exercises `impl_all_tuple_entity!(A, B, C)`'s
    // `ReadOnlyQueryData` arm. Also drives the `for … in &q` sugar on a
    // 3-component entity tuple (its `iter_ref` body).
    let mut world = World::new();
    let full = world.spawn().insert(A(1)).insert(B(2)).insert(C(3)).id();
    world.spawn().insert(A(9)).insert(B(9)); // no C — join must skip it

    let rows: Vec<(Entity, i32, i32, i32)> = Query::<(Entity, &A, &B, &C)>::from_world(&world)
        .iter()
        .map(|(id, a, b, c)| (id, a.0, b.0, c.0))
        .collect();
    assert_eq!(rows, vec![(full, 1, 2, 3)]);

    // `&q` sugar over the same shape — id owns the components it's with.
    let q = Query::<(Entity, &A, &B, &C)>::from_world(&world);
    let mut count = 0;
    for (id, a, _b, _c) in &q {
        assert_eq!(world.get::<A>(id).unwrap().0, a.0);
        count += 1;
    }
    assert_eq!(count, 1);
}

#[test]
fn query_entity_tuple_for_in_mut_sugar_writes_through() {
    // `for … in &mut q` over an entity tuple — the `IntoIterator for
    // &mut Query` path (`iter`), distinct from the `&q` (`iter_ref`)
    // path the read tests cover.
    let (world, _) = world_with_three_movers();
    {
        let mut q = Query::<(Entity, &mut Position)>::from_world(&world);
        for (id, mut p) in &mut q {
            assert!(world.is_alive(id)); // the threaded id is a live handle
            p.0 += 1;
        }
    }
    let xs: Vec<i32> = Query::<&Position>::from_world(&world)
        .iter()
        .map(|p| p.0)
        .collect();
    assert_eq!(xs, vec![1, 11, 21]);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_entity_double_mut_same_type_panics() {
    // Write+write of the same component, behind an Entity prefix: the
    // entity-prefixed `collect_access` must still report both writes so
    // the self-conflict check fires (proves no `access_call!` was
    // dropped in the entity macro expansion).
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(Entity, &mut Position, &mut Position)>::from_world(&world);
}

#[test]
fn query_entity_tuple_without_filter_excludes_marked_entities() {
    // Filter threading through an entity tuple: the outer (threaded) id
    // the filter tests must be the driver's entity, so `Without<Marker>`
    // drops the marked one and the surviving id/value pair is correct.
    let mut world = World::new();
    let plain = world.spawn().insert(Position(1, 1)).id();
    let _marked = world.spawn().insert(Position(2, 2)).insert(Marker).id();

    let rows: Vec<(Entity, i32)> =
        Query::<(Entity, &Position), Without<Marker>>::from_world(&world)
            .iter()
            .map(|(e, p)| (e, p.0))
            .collect();
    assert_eq!(rows, vec![(plain, 1)]);
}

#[test]
fn query_entity_read_tuple_iter_and_iter_mut_agree() {
    // `.iter()` on an all-read entity tuple takes the `@readonly`
    // `iter_ref` path; `.iter_mut()` takes the `@gen` `iter` path. They
    // are distinct macro expansions and must yield identical pairs — the
    // only test that exercises `@gen` for an *all-read* entity tuple.
    let (world, [a, b, c]) = world_with_three_movers();

    let via_iter: Vec<(Entity, i32)> = Query::<(Entity, &Position)>::from_world(&world)
        .iter()
        .map(|(e, p)| (e, p.0))
        .collect();
    let mut q = Query::<(Entity, &Position)>::from_world(&world);
    let via_iter_mut: Vec<(Entity, i32)> = q.iter_mut().map(|(e, p)| (e, p.0)).collect();

    assert_eq!(via_iter, via_iter_mut);
    assert_eq!(via_iter, vec![(a, 0), (b, 10), (c, 20)]);
}

#[test]
fn query_entity_tuple_excludes_immediately_despawned_entity() {
    // `world.despawn` (immediate, unlike `Commands::despawn`) removes
    // components before freeing the slot, so a tuple query built after it
    // drives off a storage that no longer holds the despawned entity.
    // Pins that despawn-ordering invariant for the entity-tuple path.
    let (mut world, [a, b, c]) = world_with_three_movers();
    world.despawn(b);
    let pairs: Vec<(Entity, i32)> = Query::<(Entity, &Position)>::from_world(&world)
        .iter()
        .map(|(e, p)| (e, p.0))
        .collect();
    assert_eq!(pairs, vec![(a, 0), (c, 20)]); // b gone (swap-removed)
}

#[test]
fn query_entity_tuple_for_unknown_component_yields_empty() {
    // `Marker` storage was never created — the entity-prefixed driver
    // must reach the `None` arm of `first_elem_driver!` and yield empty, not
    // panic (a distinct macro path from the join-skip case).
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let q = Query::<(Entity, &Marker)>::from_world(&world);
    assert_eq!(q.iter().count(), 0);
    let mut q_mut = Query::<(Entity, &mut Marker)>::from_world(&world);
    assert_eq!(q_mut.iter_mut().count(), 0);
}

#[test]
fn query_entity_after_respawn_yields_new_generation_not_stale() {
    // The snapshot must carry each slot's *current* generation: after a
    // despawn+respawn into the same slot, `Query<Entity>` yields the new
    // handle and not the stale one.
    let mut world = World::new();
    let a = world.spawn().id(); // slot 0, gen 0
    world.despawn(a);
    let b = world.spawn().id(); // slot 0 reused, gen 1
    assert_eq!(a.index, b.index);
    assert_ne!(a, b);

    let ids: Vec<Entity> = Query::<Entity>::from_world(&world).iter().collect();
    assert_eq!(ids, vec![b]);
    assert!(!ids.contains(&a)); // stale handle excluded
}
