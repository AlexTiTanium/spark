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

// ---- Query::get / Query::get_mut (issue #72) -------------------------------

// `Query::get(&Position)` returns the live component for the asked-for entity
// and `None` when the entity is missing it, despawned, or never allocated.
#[test]
fn query_get_single_read_returns_present_and_none_for_missing() {
    let mut world = World::new();
    let a = world.spawn().insert(Position(1, 0)).id();
    let b = world.spawn().insert(Position(2, 0)).id();
    let c = world.spawn().id(); // no Position

    let q = Query::<&Position>::from_world(&world);
    assert_eq!(q.get(a), Some(&Position(1, 0)));
    assert_eq!(q.get(b), Some(&Position(2, 0)));
    assert_eq!(q.get(c), None); // alive but no Position
}

// Stale and never-allocated handles both collapse to `None` via the storage's
// generation check — same machinery `World::get` uses.
#[test]
fn query_get_returns_none_for_stale_and_unallocated() {
    let mut world = World::new();
    let alive = world.spawn().insert(Position(1, 0)).id();
    let doomed = world.spawn().insert(Position(2, 0)).id();
    world.despawn(doomed);

    let q = Query::<&Position>::from_world(&world);
    assert_eq!(q.get(alive), Some(&Position(1, 0)));
    assert!(q.get(doomed).is_none()); // despawned
    // A handcrafted handle for a slot that was never allocated also returns
    // `None` — sparse[index] is out of bounds.
    let never = Entity {
        index: 999,
        generation: 0,
    };
    assert!(q.get(never).is_none());
}

// `get_mut` on `&mut T` yields a `Mut` whose write persists past the query.
#[test]
fn query_get_mut_writes_through_present_only() {
    let mut world = World::new();
    let a = world.spawn().insert(Position(5, 0)).id();
    let b = world.spawn().id(); // no Position

    {
        let mut q = Query::<&mut Position>::from_world(&world);
        q.get_mut(a).unwrap().0 += 10;
        assert!(q.get_mut(b).is_none());
    }
    assert_eq!(world.get::<Position>(a).unwrap().0, 15);
}

// `get_mut` on a stale id returns `None` via the same generation check that
// `iter_mut`'s `DenseMut::get` uses — no panic, no silent write to the
// recycled slot.
#[test]
fn query_get_mut_returns_none_for_stale_handle() {
    let mut world = World::new();
    let a = world.spawn().insert(Position(1, 0)).id();
    world.despawn(a);
    let _b = world.spawn().insert(Position(2, 0)).id(); // reuses slot 0

    let mut q = Query::<&mut Position>::from_world(&world);
    assert!(q.get_mut(a).is_none()); // a now points at a recycled slot
}

// Two-tuple join: returns the pair only when both required components are
// present, `None` if either is missing.
#[test]
fn query_get_tuple_join_is_all_or_nothing() {
    let mut world = World::new();
    let mover = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(2, 0))
        .id();
    let still = world.spawn().insert(Position(3, 0)).id();
    let drifting = world.spawn().insert(Velocity(4, 0)).id();

    let q = Query::<(&Position, &Velocity)>::from_world(&world);
    assert_eq!(q.get(mover), Some((&Position(1, 0), &Velocity(2, 0))));
    assert!(q.get(still).is_none()); // missing Velocity
    assert!(q.get(drifting).is_none()); // missing Position
}

// Multi-mut join: both components mutable, both `Mut` handles write through.
#[test]
fn query_get_mut_multi_mut_tuple_writes_each() {
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(0, 0))
        .insert(Velocity(1, 0))
        .id();

    {
        let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
        let (mut pos, mut vel) = q.get_mut(e).unwrap();
        pos.0 += vel.0;
        vel.0 *= 2;
    }
    assert_eq!(world.get::<Position>(e).unwrap().0, 1);
    assert_eq!(world.get::<Velocity>(e).unwrap().0, 2);
}

// Entity-prefixed shape: yields the id alongside the component. A stale
// entity fails the required component's generation check, so the row is
// `None` — no separate alive-check needed.
#[test]
fn query_get_entity_prefixed_yields_id_with_components() {
    let mut world = World::new();
    let e = world.spawn().insert(Position(7, 0)).id();
    world.despawn(e);
    let f = world.spawn().insert(Position(9, 0)).id();

    let q = Query::<(Entity, &Position)>::from_world(&world);
    assert_eq!(q.get(f), Some((f, &Position(9, 0))));
    assert!(q.get(e).is_none()); // stale: recycled slot fails generation check
}

// `Query<Entity>` standalone alive-checks against the snapshot — a stale id
// must return `None` even though the shape names no component.
#[test]
fn query_get_entity_standalone_alive_checks_via_snapshot() {
    let mut world = World::new();
    let a = world.spawn().id();
    let b = world.spawn().id();
    world.despawn(a);

    let q = Query::<Entity>::from_world(&world);
    assert_eq!(q.get(b), Some(b));
    assert!(q.get(a).is_none()); // not in the snapshot
    // Never-allocated handle also rejected.
    let never = Entity {
        index: 9999,
        generation: 0,
    };
    assert!(q.get(never).is_none());
}

// `Query<Option<&T>>` standalone alive-checks too — the issue's "None for
// stale" criterion applies even for shapes with no required component.
#[test]
fn query_get_option_standalone_alive_checks_via_snapshot() {
    let mut world = World::new();
    let with_pos = world.spawn().insert(Position(5, 0)).id();
    let without = world.spawn().id();
    let stale = world.spawn().id();
    world.despawn(stale);

    let q = Query::<Option<&Position>>::from_world(&world);
    assert_eq!(q.get(with_pos), Some(Some(&Position(5, 0))));
    assert_eq!(q.get(without), Some(None)); // live, no Position
    assert!(q.get(stale).is_none()); // despawned — not Some(None)
}

// `Query<Option<&mut T>>` standalone: precise change marking via the snapshot
// alive-check, otherwise mirrors the read variant.
#[test]
fn query_get_mut_option_standalone_writes_through_and_rejects_stale() {
    let mut world = World::new();
    let with_pos = world.spawn().insert(Position(1, 0)).id();
    let without = world.spawn().id();
    let stale = world.spawn().id();
    world.despawn(stale);

    {
        let mut q = Query::<Option<&mut Position>>::from_world(&world);
        if let Some(Some(mut p)) = q.get_mut(with_pos) {
            p.0 += 100;
        }
        assert!(matches!(q.get_mut(without), Some(None))); // live, no Position
        assert!(q.get_mut(stale).is_none()); // despawned
    }
    assert_eq!(world.get::<Position>(with_pos).unwrap().0, 101);
}

// Optional in a join: trailing `Option<&_>` yields a `None` value when the
// entity lacks it; the required first element still gates the row.
#[test]
fn query_get_optional_in_join_yields_none_value_for_missing_optional() {
    let mut world = World::new();
    let a = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(2, 0))
        .id();
    let b = world.spawn().insert(Position(3, 0)).id(); // no Velocity
    let c = world.spawn().insert(Velocity(9, 0)).id(); // no Position

    let q = Query::<(&Position, Option<&Velocity>)>::from_world(&world);
    assert_eq!(q.get(a), Some((&Position(1, 0), Some(&Velocity(2, 0)))));
    assert_eq!(q.get(b), Some((&Position(3, 0), None))); // optional absent → Some(None)
    assert!(q.get(c).is_none()); // missing required Position
}

// `With<T>` filter participates in `get` — an entity that passes the data
// shape but fails the filter collapses to `None`.
#[test]
fn query_get_with_filter_rejects_when_predicate_false() {
    let mut world = World::new();
    let marked = world.spawn().insert(Position(1, 0)).insert(Marker).id();
    let bare = world.spawn().insert(Position(2, 0)).id(); // no Marker

    let q = Query::<&Position, With<Marker>>::from_world(&world);
    assert_eq!(q.get(marked), Some(&Position(1, 0)));
    assert!(q.get(bare).is_none()); // has Position, lacks Marker — filter rejects
}

// `Without<T>` filter participates the same way — symmetric to `With`.
#[test]
fn query_get_without_filter_rejects_marked_entities() {
    let mut world = World::new();
    let marked = world.spawn().insert(Position(1, 0)).insert(Marker).id();
    let bare = world.spawn().insert(Position(2, 0)).id();

    let q = Query::<&Position, Without<Marker>>::from_world(&world);
    assert_eq!(q.get(bare), Some(&Position(2, 0)));
    assert!(q.get(marked).is_none()); // Without rejects the Marker-bearing one
}

// `Or` filter: a candidate passes if either arm matches. The filter participates
// even though the data shape's required element gates aliveness on its own.
#[test]
fn query_get_or_filter_passes_if_any_arm_matches() {
    let mut world = World::new();
    let with_a = world.spawn().insert(Position(1, 0)).insert(A(0)).id();
    let with_b = world.spawn().insert(Position(2, 0)).insert(B(0)).id();
    let neither = world.spawn().insert(Position(3, 0)).id();

    let q = Query::<&Position, Or<(With<A>, With<B>)>>::from_world(&world);
    assert_eq!(q.get(with_a), Some(&Position(1, 0)));
    assert_eq!(q.get(with_b), Some(&Position(2, 0)));
    assert!(q.get(neither).is_none()); // neither With<A> nor With<B> matches
}

// Reading via `get_mut` then dropping the handle without a `DerefMut` write
// must *not* stamp `changed_tick` — change marking stays precise across the
// single-fetch path, exactly as it does across `iter_mut`. Inspects the
// storage's `changed_tick_for` directly so the assertion does not depend on
// the scheduler's baseline plumbing.
//
// The clock must move between the read-only fetch and the write so a stamp
// is observable; we advance it by inserting `Position` on a second entity
// (every `insert` bumps the storage's `current_tick`).
#[test]
fn query_get_mut_does_not_mark_changed_unless_written() {
    let mut world = World::new();
    let e = world.spawn().insert(Position(0, 0)).id();
    let tick_after_insert = world
        .storage::<Position>()
        .unwrap()
        .changed_tick_for(e)
        .unwrap();

    // Read-only get_mut: drop without DerefMut. Must leave `changed_tick`
    // exactly where insert left it.
    {
        let mut q = Query::<&mut Position>::from_world(&world);
        let pos = q.get_mut(e).unwrap();
        assert_eq!(pos.0, 0); // Deref only.
    }
    assert_eq!(
        world
            .storage::<Position>()
            .unwrap()
            .changed_tick_for(e)
            .unwrap(),
        tick_after_insert,
        "read-without-write must not stamp changed_tick"
    );

    // Advance Position's clock by inserting on a fresh entity — every
    // `insert` calls `advance_tick` on that storage.
    let _ = world.spawn().insert(Position(99, 99)).id();
    let tick_after_bump = world.storage::<Position>().unwrap().current_tick();
    assert_ne!(tick_after_bump, tick_after_insert);

    // Write through the handle — DerefMut stamps at the new current_tick.
    {
        let mut q = Query::<&mut Position>::from_world(&world);
        q.get_mut(e).unwrap().0 = 7;
    }
    assert_eq!(
        world
            .storage::<Position>()
            .unwrap()
            .changed_tick_for(e)
            .unwrap(),
        tick_after_bump,
        "DerefMut write must stamp at current_tick"
    );
}

// `get` and `iter` agree on the entities they return — for every entity the
// query yields, `get(id)` returns the same item; for every entity outside it,
// `get` returns `None`.
#[test]
fn query_get_agrees_with_iter_for_filtered_query() {
    let mut world = World::new();
    let _a = world.spawn().insert(Position(1, 0)).insert(Marker).id();
    let bare = world.spawn().insert(Position(2, 0)).id(); // no Marker, filtered out
    let _c = world.spawn().insert(Position(3, 0)).insert(Marker).id();

    let q = Query::<(Entity, &Position), With<Marker>>::from_world(&world);
    let iter_set: std::collections::HashMap<Entity, &Position> = q.iter().collect();
    assert_eq!(iter_set.len(), 2);
    for (entity, pos) in &iter_set {
        assert_eq!(q.get(*entity), Some((*entity, *pos)));
    }
    assert!(q.get(bare).is_none()); // filtered out — must agree with iter
}

// Arity-3 join: every required element gates the row, the `?`-chain
// short-circuits on the first missing component.
#[test]
fn query_get_three_component_join_is_all_or_nothing() {
    let mut world = World::new();
    let all = world.spawn().insert(A(1)).insert(B(2)).insert(C(3)).id();
    let missing_c = world.spawn().insert(A(4)).insert(B(5)).id();
    let missing_b = world.spawn().insert(A(6)).insert(C(7)).id();

    let q = Query::<(&A, &B, &C)>::from_world(&world);
    assert_eq!(q.get(all), Some((&A(1), &B(2), &C(3))));
    assert!(q.get(missing_c).is_none()); // `?` short-circuits at C
    assert!(q.get(missing_b).is_none()); // `?` short-circuits at B
}

// Entity-prefixed shape with a `&mut T` — exercises the `W` arm of
// `lookup_mut_one!` inside `impl_one_combo_entity!`'s @gen, which the read
// variant test (`query_get_entity_prefixed_yields_id_with_components`) does
// not cover.
#[test]
fn query_get_mut_entity_prefixed_writes_through() {
    let mut world = World::new();
    let e = world.spawn().insert(Position(5, 0)).id();

    {
        let mut q = Query::<(Entity, &mut Position)>::from_world(&world);
        let (id, mut pos) = q.get_mut(e).unwrap();
        assert_eq!(id, e); // the prefix rides the asked-for id
        pos.0 += 100;
    }
    assert_eq!(world.get::<Position>(e).unwrap().0, 105);
}

// Mut-required + optional-read in a join through `get_mut` — exercises
// `lookup_mut_one!(W, …)` for the driver slot and `(O, …)` for the trailing
// slot together.
#[test]
fn query_get_mut_required_with_optional_trailing_coexist() {
    let mut world = World::new();
    let with_vel = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(2, 0))
        .id();
    let no_vel = world.spawn().insert(Position(3, 0)).id();

    {
        let mut q = Query::<(&mut Position, Option<&Velocity>)>::from_world(&world);
        // Optional present: write through the required slot, optional rides.
        let (mut pos, vel) = q.get_mut(with_vel).unwrap();
        pos.0 += vel.unwrap().0;
        // Optional absent: still a row, optional is `None`, write proceeds.
        let (mut pos2, vel2) = q.get_mut(no_vel).unwrap();
        assert!(vel2.is_none());
        pos2.0 += 10;
    }
    assert_eq!(world.get::<Position>(with_vel).unwrap().0, 3);
    assert_eq!(world.get::<Position>(no_vel).unwrap().0, 13);
}

// `Changed<T>` filter through `get`: an entity whose `changed_tick` predates
// the parked baseline must be rejected, and one whose tick post-dates it
// must pass. We drive the baseline directly via an observer system's
// `last_seen` vec, matching the staging in `change_detection.rs`.
#[test]
fn query_get_changed_filter_rejects_unmodified_entity() {
    use crate::Changed;
    use crate::access::Access;

    let mut world = World::new();
    // Spawn the bystander first so its insert stamps the earlier tick; the
    // writer's later insert advances the clock and stamps a strictly newer
    // tick. The baseline we park sits between them.
    let bystander = world.spawn().insert(Position(99, 0)).id();
    let bystander_tick = world.storage::<Position>().unwrap().current_tick();
    let writer = world.spawn().insert(Position(0, 0)).id();

    // Park `bystander_tick` as the baseline: the writer's later insert
    // stamps a tick > baseline → passes Changed; bystander's insert tick
    // equals baseline → fails Changed (the strict-`<` in `is_changed_since`).
    let pos_tid = std::any::TypeId::of::<Position>();
    let mut observer_baseline = vec![(pos_tid, bystander_tick)];
    let observer_access = Access::new(); // empty: no clock advance on observe

    world.run_system(&observer_access, &mut observer_baseline, &mut |w| {
        let q = Query::<&Position, Changed<Position>>::from_world(w);
        assert!(q.get(writer).is_some(), "writer must pass Changed filter");
        assert!(
            q.get(bystander).is_none(),
            "bystander must fail Changed filter (its tick equals baseline)"
        );
    });
}

// Absent storage case: when no entity has ever held `T`, `init_state` returns
// `None`. `get` must yield `None` for any input without panicking.
#[test]
fn query_get_returns_none_when_storage_absent() {
    let mut world = World::new();
    // World holds entities, but no Position has ever been inserted, so the
    // storage `Option<Ref<…>>` is `None`.
    let e = world.spawn().insert(Marker).id();

    let q = Query::<&Position>::from_world(&world);
    assert!(q.get(e).is_none()); // alive, but no Position storage
    let never = Entity {
        index: 9999,
        generation: 0,
    };
    assert!(q.get(never).is_none()); // also a clean `None`, no panic
}

// `Added<T>` is a structurally distinct filter from `Changed<T>` — it reads
// `added_tick` (set only on first attach), not `changed_tick`. The matches
// branch in `lookup`/`drive_ref` is the same, so a `get` test exercises it
// the same way the `Changed` test does.
#[test]
fn query_get_added_filter_rejects_attached_before_baseline() {
    use crate::Added;
    use crate::access::Access;

    let mut world = World::new();
    // Bystander attached before the baseline tick → fails `Added`.
    let bystander = world.spawn().insert(Position(99, 0)).id();
    let bystander_tick = world.storage::<Position>().unwrap().current_tick();
    // Writer attached *after* — its `added_tick` is strictly newer.
    let writer = world.spawn().insert(Position(0, 0)).id();

    let pos_tid = std::any::TypeId::of::<Position>();
    let mut observer_baseline = vec![(pos_tid, bystander_tick)];
    let observer_access = Access::new();

    world.run_system(&observer_access, &mut observer_baseline, &mut |w| {
        let q = Query::<&Position, Added<Position>>::from_world(w);
        assert!(q.get(writer).is_some(), "writer must pass Added filter");
        assert!(
            q.get(bystander).is_none(),
            "bystander attached at baseline must fail Added"
        );
    });
}

// `And<(...)>` filter through `get` — both arms must match. Symmetric to the
// `Or` test above; exercises the combinator's `matches` dispatch through the
// `get` path (distinct from single-predicate `With`/`Without`).
#[test]
fn query_get_and_filter_requires_both_predicates() {
    use crate::filter::And;

    let mut world = World::new();
    let both = world
        .spawn()
        .insert(Position(1, 0))
        .insert(A(0))
        .insert(B(0))
        .id();
    let only_a = world.spawn().insert(Position(2, 0)).insert(A(0)).id();
    let only_b = world.spawn().insert(Position(3, 0)).insert(B(0)).id();
    let neither = world.spawn().insert(Position(4, 0)).id();

    let q = Query::<&Position, And<(With<A>, With<B>)>>::from_world(&world);
    assert_eq!(q.get(both), Some(&Position(1, 0)));
    assert!(q.get(only_a).is_none()); // missing the With<B> arm
    assert!(q.get(only_b).is_none()); // missing the With<A> arm
    assert!(q.get(neither).is_none()); // missing both arms
}

// Mixed-mut tuple `(&mut A, &B)::get_mut` — `lookup_mut_one!(W, …)` for the
// first slot and `(R, …)` for the second, distinct from the all-mut and
// all-read tuples already covered.
#[test]
fn query_get_mut_mixed_mut_read_tuple_writes_through_mut_slot() {
    let mut world = World::new();
    let with_both = world
        .spawn()
        .insert(Position(10, 0))
        .insert(Velocity(3, 0))
        .id();
    let only_pos = world.spawn().insert(Position(20, 0)).id();

    {
        let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
        let (mut pos, vel) = q.get_mut(with_both).unwrap();
        pos.0 += vel.0;
        assert!(q.get_mut(only_pos).is_none()); // required &Velocity missing
    }
    assert_eq!(world.get::<Position>(with_both).unwrap().0, 13);
    assert_eq!(world.get::<Position>(only_pos).unwrap().0, 20); // untouched
}

// ---- get_many ----

// The batched read returns one slot per id, in the order asked. A present id
// yields `Some`, a live-but-componentless id yields `None`, exactly mirroring
// `get` one slot at a time. The single-element call also exercises the `N == 1`
// boundary of the const-generic array form.
#[test]
fn query_get_many_returns_slot_per_id_in_order() {
    let mut world = World::new();
    let a = world.spawn().insert(Position(1, 0)).id();
    let b = world.spawn().insert(Position(2, 0)).id();
    let ghost = world.spawn().id(); // alive, but no Position

    let q = Query::<&Position>::from_world(&world);
    let [first, second, third] = q.get_many([a, b, ghost]);
    assert_eq!(first, Some(&Position(1, 0)));
    assert_eq!(second, Some(&Position(2, 0)));
    assert!(third.is_none()); // missing component → None, like `get`

    // N == 1 boundary: a single-element array is still one ordered slot.
    let [only] = q.get_many([a]);
    assert_eq!(only, Some(&Position(1, 0)));
}

// `get_many` is just `N` independent `get`s: a repeated id reads the same data
// back into both slots (each is a shared `&` borrow), and the two structurally
// distinct rejections — a despawned/stale handle (generation mismatch) and a
// never-allocated handle (index out of range) — both collapse to `None`.
#[test]
fn query_get_many_allows_duplicate_ids_and_rejects_stale_and_never() {
    let mut world = World::new();
    let a = world.spawn().insert(Position(7, 0)).id();
    let doomed = world.spawn().insert(Position(0, 0)).id();
    world.despawn(doomed); // stale handle from here on
    let never = Entity {
        index: 9999,
        generation: 0,
    };

    let q = Query::<&Position>::from_world(&world);
    let [first, second, stale, oob] = q.get_many([a, a, doomed, never]);
    assert_eq!(first, Some(&Position(7, 0)));
    assert_eq!(second, Some(&Position(7, 0))); // duplicate id is harmless
    assert!(stale.is_none()); // despawned → generation mismatch
    assert!(oob.is_none()); // never allocated → index out of range, distinct path
}

// Each slot runs the full join independently: a slot is `Some` only when its
// entity has every required component, `None` (just for that slot) otherwise.
#[test]
fn query_get_many_join_is_all_or_nothing_per_slot() {
    let mut world = World::new();
    let both = world
        .spawn()
        .insert(Position(1, 0))
        .insert(Velocity(2, 0))
        .id();
    let pos_only = world.spawn().insert(Position(3, 0)).id();

    let q = Query::<(&Position, &Velocity)>::from_world(&world);
    let [a, b] = q.get_many([both, pos_only]);
    assert_eq!(a, Some((&Position(1, 0), &Velocity(2, 0))));
    assert!(b.is_none()); // missing the &Velocity half — only this slot is None
}

// The filter participates per slot, same as in `get`: an entity that satisfies
// the data shape but fails the filter is `None` in its own slot only.
#[test]
fn query_get_many_filter_rejects_per_slot() {
    let mut world = World::new();
    let marked = world.spawn().insert(Position(1, 0)).insert(Marker).id();
    let bare = world.spawn().insert(Position(2, 0)).id(); // no Marker

    let q = Query::<&Position, With<Marker>>::from_world(&world);
    let [a, b] = q.get_many([marked, bare]);
    assert_eq!(a, Some(&Position(1, 0)));
    assert!(b.is_none()); // has Position, lacks Marker — filter rejects this slot
}

// `get_many::<0>` over an empty array is a valid call and returns an empty
// array without touching the world — the `N == 0` boundary of the const generic.
#[test]
fn query_get_many_empty_array_is_empty() {
    let world = World::new();
    let q = Query::<&Position>::from_world(&world);
    let out: [Option<&Position>; 0] = q.get_many([]);
    assert_eq!(out, []);
}

// Standalone `Option<&T>` keeps the nested `Some(None)` vs flat `None`
// distinction inside an array slot: a live-but-componentless entity is
// `Some(None)`, a despawned one is `None`. A regression collapsing either into
// the other inside `array::map` would be caught here, mirroring the
// single-entity `query_get_option_standalone_alive_checks_via_snapshot`.
#[test]
fn query_get_many_option_standalone_distinguishes_some_none_from_none() {
    let mut world = World::new();
    let with_pos = world.spawn().insert(Position(5, 0)).id();
    let without = world.spawn().id(); // live, no Position
    let stale = world.spawn().id();
    world.despawn(stale);

    let q = Query::<Option<&Position>>::from_world(&world);
    let [a, b, c] = q.get_many([with_pos, without, stale]);
    assert_eq!(a, Some(Some(&Position(5, 0))));
    assert_eq!(b, Some(None)); // live, no component → nested Some(None)
    assert!(c.is_none()); // despawned → flat None, not Some(None)
}

// Standalone `Query<Entity>` has no component storage to gate aliveness — it
// alive-checks each id against the frozen live snapshot. `get_many` must take
// that distinct path per slot: a live id reads back as itself, a despawned or
// never-allocated id is `None`.
#[test]
fn query_get_many_entity_standalone_uses_snapshot_alive_check() {
    let mut world = World::new();
    let a = world.spawn().id();
    let b = world.spawn().id();
    world.despawn(a);
    let never = Entity {
        index: 9999,
        generation: 0,
    };

    let q = Query::<Entity>::from_world(&world);
    let [ra, rb, rn] = q.get_many([a, b, never]);
    assert!(ra.is_none()); // despawned → absent from the snapshot
    assert_eq!(rb, Some(b)); // live id reads back as itself
    assert!(rn.is_none()); // never-allocated → None
}
