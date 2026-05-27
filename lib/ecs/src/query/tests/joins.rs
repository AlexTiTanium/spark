use super::*;

use crate::access::QueryAccess;
use crate::query::dense_mut::DenseMut;
use crate::system::IntoSystem;
use crate::{Query, QueryData, World};
#[test]
fn query_two_tuple_mut_drives_and_writes_through() {
    // The canonical movement example. Position and Velocity have equal
    // populations here (3 each), so the tie breaks to the first element:
    // Position (mut) drives, Velocity (shared) is sparse-looked-up. With
    // unequal populations the smaller would drive — see `driver_cost_tests`.
    let (world, entities) = world_with_three_movers();
    {
        let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
        for (mut pos, vel) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
        }
    }
    assert_eq!(*world.get::<Position>(entities[0]).unwrap(), Position(1, 0));
    assert_eq!(
        *world.get::<Position>(entities[1]).unwrap(),
        Position(10, 11)
    );
    assert_eq!(
        *world.get::<Position>(entities[2]).unwrap(),
        Position(21, 21)
    );
}

#[test]
fn query_two_tuple_skips_entity_missing_second_component() {
    // E0 has both. E1 only has Position. The join must skip E1
    // even though Position drives the walk.
    let mut world = World::new();
    let e0 = world
        .spawn()
        .insert(Position(1, 1))
        .insert(Velocity(2, 2))
        .id();
    let e1 = world.spawn().insert(Position(99, 99)).id();
    {
        let mut q = Query::<(&mut Position, &Velocity)>::from_world(&world);
        for (mut pos, vel) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
        }
    }
    assert_eq!(*world.get::<Position>(e0).unwrap(), Position(3, 3));
    assert_eq!(*world.get::<Position>(e1).unwrap(), Position(99, 99));
}

#[test]
fn query_for_unknown_component_yields_empty_iter() {
    // No entity has ever held a Marker → the storage doesn't even
    // exist. The query must compile and produce an empty iterator,
    // not panic.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));

    let q = Query::<&Marker>::from_world(&world);
    assert_eq!(q.iter().count(), 0);

    let mut q_mut = Query::<&mut Marker>::from_world(&world);
    assert_eq!(q_mut.iter_mut().count(), 0);
}

#[test]
fn two_shared_queries_over_same_type_coexist() {
    let (world, _) = world_with_three_movers();
    let q_a = Query::<&Position>::from_world(&world);
    let q_b = Query::<&Position>::from_world(&world);
    assert_eq!(q_a.iter().count(), q_b.iter().count());
}

#[test]
#[should_panic(expected = "already borrowed")]
fn two_mut_queries_over_same_type_panic_on_second_fetch() {
    let (world, _) = world_with_three_movers();
    let _q_a = Query::<&mut Position>::from_world(&world);
    let _q_b = Query::<&mut Position>::from_world(&world);
}
#[test]
fn query_three_tuple_joins_three_storages() {
    let mut world = World::new();
    // E0 has A, B, C — should be yielded.
    world.spawn().insert(A(1)).insert(B(2)).insert(C(3));
    // E1 has A and B but no C — must be skipped.
    world.spawn().insert(A(10)).insert(B(20));
    // E2 has all three plus an extra D — should be yielded.
    world
        .spawn()
        .insert(A(100))
        .insert(B(200))
        .insert(C(300))
        .insert(D(400));

    // Path B: iterator yields data only — no `Entity` in the pair.
    let q = Query::<(&A, &B, &C)>::from_world(&world);
    let mut yielded = Vec::new();
    for (a, b, c) in q.iter() {
        yielded.push((a.0, b.0, c.0));
    }
    assert_eq!(yielded.len(), 2);
    assert!(yielded.contains(&(1, 2, 3)));
    assert!(yielded.contains(&(100, 200, 300)));
    // E1's values (A=10) must never appear — it was missing C.
    assert!(yielded.iter().all(|(a, _, _)| *a != 10));
}

#[test]
fn query_four_tuple_joins_four_storages() {
    let mut world = World::new();
    // E0 has all four — yielded.
    world
        .spawn()
        .insert(A(1))
        .insert(B(2))
        .insert(C(3))
        .insert(D(4));
    // E1 missing D — must be skipped.
    world.spawn().insert(A(10)).insert(B(20)).insert(C(30));
    // E2 has all four — yielded.
    world
        .spawn()
        .insert(A(100))
        .insert(B(200))
        .insert(C(300))
        .insert(D(400));

    let q = Query::<(&A, &B, &C, &D)>::from_world(&world);
    let mut yielded = Vec::new();
    for (a, b, c, d) in q.iter() {
        yielded.push((a.0, b.0, c.0, d.0));
    }
    assert_eq!(yielded.len(), 2);
    assert!(yielded.contains(&(1, 2, 3, 4)));
    assert!(yielded.contains(&(100, 200, 300, 400)));
}

#[test]
fn query_three_tuple_with_mut_driver_writes_through() {
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(0, 0))
        .insert(Velocity(2, 3))
        .insert(Marker)
        .id();
    // Missing Marker — skipped by the join even though Position
    // drives.
    world
        .spawn()
        .insert(Position(100, 100))
        .insert(Velocity(7, 7));
    {
        let mut q = Query::<(&mut Position, &Velocity, &Marker)>::from_world(&world);
        for (mut pos, vel, _marker) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
        }
    }
    assert_eq!(*world.get::<Position>(e).unwrap(), Position(2, 3));
}

#[test]
fn query_as_system_param_via_into_system() {
    // Smoke-test that the SystemParam impl wires up automatically
    // through #9's `IntoSystem` machinery. No explicit fetch call;
    // the system fn declares `Query<…>` and the runner builds it.
    let (world, entities) = world_with_three_movers();
    fn step(mut q: Query<(&mut Position, &Velocity)>) {
        for (mut pos, vel) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
        }
    }
    let mut sys = IntoSystem::into_system(step);
    sys(&world);
    sys(&world);
    assert_eq!(*world.get::<Position>(entities[0]).unwrap(), Position(2, 0));
    assert_eq!(
        *world.get::<Position>(entities[1]).unwrap(),
        Position(10, 12)
    );
    assert_eq!(
        *world.get::<Position>(entities[2]).unwrap(),
        Position(22, 22)
    );
}

// -------- Arity-2 multi-mut: (&mut A, &mut B) --------

#[test]
fn query_two_mut_tuple_writes_through_both_sides() {
    let (world, entities) = world_with_three_movers();
    {
        let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
        for (mut pos, mut vel) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
            // Mutate B too — proves the second slot really is `&mut`.
            vel.0 *= 2;
            vel.1 *= 2;
        }
    }
    assert_eq!(*world.get::<Position>(entities[0]).unwrap(), Position(1, 0));
    assert_eq!(
        *world.get::<Position>(entities[1]).unwrap(),
        Position(10, 11)
    );
    assert_eq!(
        *world.get::<Position>(entities[2]).unwrap(),
        Position(21, 21)
    );
    assert_eq!(*world.get::<Velocity>(entities[0]).unwrap(), Velocity(2, 0));
    assert_eq!(*world.get::<Velocity>(entities[1]).unwrap(), Velocity(0, 2));
    assert_eq!(*world.get::<Velocity>(entities[2]).unwrap(), Velocity(2, 2));
}

#[test]
fn query_two_mut_tuple_skips_entity_missing_second_component() {
    let mut world = World::new();
    let e0 = world
        .spawn()
        .insert(Position(1, 1))
        .insert(Velocity(2, 2))
        .id();
    let e1 = world.spawn().insert(Position(99, 99)).id();
    {
        let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
        for (mut pos, mut vel) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
            vel.0 = -1;
            vel.1 = -1;
        }
    }
    assert_eq!(*world.get::<Position>(e0).unwrap(), Position(3, 3));
    assert_eq!(*world.get::<Velocity>(e0).unwrap(), Velocity(-1, -1));
    assert_eq!(*world.get::<Position>(e1).unwrap(), Position(99, 99));
    assert!(world.get::<Velocity>(e1).is_none());
}

#[test]
fn query_two_mut_tuple_empty_when_either_storage_absent() {
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
    assert_eq!(q.iter_mut().count(), 0);
}

// -------- Self-conflict detection --------

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_two_mut_tuple_same_type_panics_with_named_component() {
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(&mut Position, &mut Position)>::from_world(&world);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_mut_plus_read_same_type_panics_with_named_component() {
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(&mut Position, &Position)>::from_world(&world);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_arity_three_self_conflict_panics() {
    // Arity-3 macro must propagate `collect_access` to every
    // element. A silent miss would slip past the conflict check
    // and double-borrow the driver storage cell.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0)).insert(Velocity(0, 0));
    let _q = Query::<(&mut Position, &Velocity, &Position)>::from_world(&world);
}

#[test]
fn query_self_conflict_panic_originates_before_refcell_borrow() {
    // The conflict shapes must panic from
    // `QueryAccess::assert_no_self_conflict`, not the `RefCell`
    // borrow inside `init_state`. A future refactor that
    // accidentally reordered the check to run after `init_state`
    // would surface as "already borrowed" instead.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));

    for kind in ["write_write", "write_read", "read_write"] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
            "write_write" => {
                let _q = Query::<(&mut Position, &mut Position)>::from_world(&world);
            }
            "write_read" => {
                let _q = Query::<(&mut Position, &Position)>::from_world(&world);
            }
            "read_write" => {
                let _q = Query::<(&Position, &mut Position)>::from_world(&world);
            }
            _ => unreachable!(),
        }));
        let payload = result.expect_err("expected panic");
        let msg = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&'static str>().copied())
            .unwrap_or("");
        assert!(
            msg.contains("conflicting access to component"),
            "{kind}: wrong panic message: {msg}"
        );
        assert!(
            !msg.contains("already borrowed"),
            "{kind}: panic came from RefCell, not the self-conflict check: {msg}"
        );
    }
}

#[test]
fn query_two_mut_tuple_collect_access_is_conflict_free() {
    // Two writes of distinct types — no self-conflict.
    let mut access = QueryAccess::default();
    <(&mut Position, &mut Velocity) as QueryData>::collect_access(&mut access);
    access.assert_no_self_conflict();
}

// -------- Wider multi-mut + mut-not-first (the unified macro) --------

#[test]
fn query_arity_three_multi_mut_writes_through_all_sides() {
    // `(&mut A, &mut B, &mut C)` — three storages, all mutable.
    // Driver A's safe `iter_mut`; B and C looked up per entity via
    // their own `DenseMut` views. Each entity touched once by the
    // driver, so each view's `get` is called at most once per
    // entity.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(1, 2))
        .insert(Velocity(10, 20))
        .insert(Marker)
        .id();
    {
        let mut q = Query::<(&mut Position, &mut Velocity, &mut Marker)>::from_world(&world);
        for (mut pos, mut vel, _marker) in q.iter_mut() {
            pos.0 += vel.0;
            pos.1 += vel.1;
            vel.0 = -5;
            vel.1 = -5;
        }
    }
    assert_eq!(*world.get::<Position>(e).unwrap(), Position(11, 22));
    assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(-5, -5));
}

#[test]
fn query_arity_four_multi_mut_writes_through_all_sides() {
    // Arity 4, all mutable. Same logic as arity 3.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(A(1))
        .insert(B(2))
        .insert(C(3))
        .insert(D(4))
        .id();
    {
        let mut q = Query::<(&mut A, &mut B, &mut C, &mut D)>::from_world(&world);
        for (mut a, mut b, mut c, mut d) in q.iter_mut() {
            a.0 += 100;
            b.0 += 100;
            c.0 += 100;
            d.0 += 100;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 101);
    assert_eq!(world.get::<B>(e).unwrap().0, 102);
    assert_eq!(world.get::<C>(e).unwrap().0, 103);
    assert_eq!(world.get::<D>(e).unwrap().0, 104);
}

#[test]
fn query_arity_five_mixed_writes_through_mut_positions() {
    // Arity-5 smoke test: confirms `impl_all_tuple!(A, B, C, D, E)`
    // expands cleanly and that a mixed combination at the new
    // arity behaves like the lower arities (driver A is read,
    // muts at B / D, reads at C / E).
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(A(1))
        .insert(B(2))
        .insert(C(3))
        .insert(D(4))
        .insert(E(5))
        .id();
    {
        let mut q = Query::<(&A, &mut B, &C, &mut D, &E)>::from_world(&world);
        for (a, mut b, c, mut d, e_item) in q.iter_mut() {
            b.0 = a.0 + c.0 + e_item.0; // 1 + 3 + 5 = 9
            d.0 = a.0 + c.0 + e_item.0; // 9
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 1); // unchanged
    assert_eq!(world.get::<B>(e).unwrap().0, 9);
    assert_eq!(world.get::<C>(e).unwrap().0, 3); // unchanged
    assert_eq!(world.get::<D>(e).unwrap().0, 9);
    assert_eq!(world.get::<E>(e).unwrap().0, 5); // unchanged
}

#[test]
fn query_read_driver_with_mut_non_driver_writes_through() {
    // `(&A, &mut B)` — read driver, mut non-driver. Previously
    // deferred ("write as `(&mut B, &A)` instead"); now ships.
    // Driver A's safe `iter`; B looked up per entity via `DenseMut`.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(7, 7))
        .insert(Velocity(1, 2))
        .id();
    {
        let mut q = Query::<(&Position, &mut Velocity)>::from_world(&world);
        for (pos, mut vel) in q.iter_mut() {
            vel.0 += pos.0;
            vel.1 += pos.1;
        }
    }
    assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(8, 9));
    // Position untouched.
    assert_eq!(*world.get::<Position>(e).unwrap(), Position(7, 7));
}

#[test]
fn query_mixed_mut_arity_three_writes_only_through_mut_positions() {
    // `(&A, &mut B, &C)` — only B is mutable. Driver A reads, C
    // reads, B writes.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(2, 3))
        .insert(Velocity(10, 10))
        .insert(Marker)
        .id();
    {
        let mut q = Query::<(&Position, &mut Velocity, &Marker)>::from_world(&world);
        for (pos, mut vel, _marker) in q.iter_mut() {
            vel.0 += pos.0;
            vel.1 += pos.1;
        }
    }
    assert_eq!(*world.get::<Velocity>(e).unwrap(), Velocity(12, 13));
    // Position, Marker untouched.
    assert_eq!(*world.get::<Position>(e).unwrap(), Position(2, 3));
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_arity_three_multi_mut_self_conflict_panics() {
    // `(&mut A, &mut A, &mut B)` — A written twice. Caught by the
    // self-conflict check at `Query::from_world` time.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0)).insert(Velocity(0, 0));
    let _q = Query::<(&mut Position, &mut Position, &mut Velocity)>::from_world(&world);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_mut_not_first_self_conflict_panics() {
    // `(&A, &mut A)` — reversed-order conflict (read driver, mut
    // non-driver, same component). Now reachable as a query shape
    // since the unified macro covers it; previously the access-
    // level test was the only coverage of the reversed direction.
    let mut world = World::new();
    world.spawn().insert(Position(0, 0));
    let _q = Query::<(&Position, &mut Position)>::from_world(&world);
}

// -------- Coverage for previously-untested mixed shape combos --------

#[test]
fn query_arity_three_mixed_combinations_write_only_through_mut_positions() {
    // Exercises the four arity-3 combinations not covered by the
    // dedicated tests above: `(&A, &B, &mut C)`,
    // `(&mut A, &mut B, &C)`, `(&mut A, &B, &mut C)`, and
    // `(&A, &mut B, &mut C)`. Each block mutates only the `&mut`
    // positions and verifies non-`&mut` positions stayed put.
    let mut world = World::new();
    let e = world.spawn().insert(A(1)).insert(B(2)).insert(C(3)).id();

    // (&A, &B, &mut C): only C is mutable.
    {
        let mut q = Query::<(&A, &B, &mut C)>::from_world(&world);
        for (a, b, mut c) in q.iter_mut() {
            c.0 = a.0 + b.0;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 1);
    assert_eq!(world.get::<B>(e).unwrap().0, 2);
    assert_eq!(world.get::<C>(e).unwrap().0, 3); // 1 + 2

    // (&mut A, &mut B, &C): A and B mutable, C read.
    {
        let mut q = Query::<(&mut A, &mut B, &C)>::from_world(&world);
        for (mut a, mut b, c) in q.iter_mut() {
            a.0 = c.0 * 10;
            b.0 = c.0 * 20;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 30);
    assert_eq!(world.get::<B>(e).unwrap().0, 60);
    assert_eq!(world.get::<C>(e).unwrap().0, 3); // unchanged

    // (&mut A, &B, &mut C): muts at outer positions.
    {
        let mut q = Query::<(&mut A, &B, &mut C)>::from_world(&world);
        for (mut a, b, mut c) in q.iter_mut() {
            a.0 = b.0 + 100;
            c.0 = b.0 + 200;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 160);
    assert_eq!(world.get::<B>(e).unwrap().0, 60); // unchanged
    assert_eq!(world.get::<C>(e).unwrap().0, 260);

    // (&A, &mut B, &mut C): read driver, two muts.
    {
        let mut q = Query::<(&A, &mut B, &mut C)>::from_world(&world);
        for (a, mut b, mut c) in q.iter_mut() {
            b.0 = a.0 + 1;
            c.0 = a.0 + 2;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 160); // unchanged
    assert_eq!(world.get::<B>(e).unwrap().0, 161);
    assert_eq!(world.get::<C>(e).unwrap().0, 162);
}

#[test]
fn query_arity_four_mixed_combinations_write_only_through_mut_positions() {
    // Four representative mixed combinations out of the 14
    // arity-4 mixes not covered elsewhere. Each block mutates
    // only the `&mut` positions and verifies non-`&mut` positions
    // stayed put.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(A(1))
        .insert(B(2))
        .insert(C(3))
        .insert(D(4))
        .id();

    // (&mut A, &mut B, &C, &D): muts at first two positions.
    {
        let mut q = Query::<(&mut A, &mut B, &C, &D)>::from_world(&world);
        for (mut a, mut b, c, d) in q.iter_mut() {
            a.0 = c.0 + d.0;
            b.0 = c.0 * d.0;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 7);
    assert_eq!(world.get::<B>(e).unwrap().0, 12);
    assert_eq!(world.get::<C>(e).unwrap().0, 3);
    assert_eq!(world.get::<D>(e).unwrap().0, 4);

    // (&A, &mut B, &mut C, &mut D): read driver, three muts.
    {
        let mut q = Query::<(&A, &mut B, &mut C, &mut D)>::from_world(&world);
        for (a, mut b, mut c, mut d) in q.iter_mut() {
            b.0 = a.0 + 100;
            c.0 = a.0 + 200;
            d.0 = a.0 + 300;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 7); // unchanged
    assert_eq!(world.get::<B>(e).unwrap().0, 107);
    assert_eq!(world.get::<C>(e).unwrap().0, 207);
    assert_eq!(world.get::<D>(e).unwrap().0, 307);

    // (&mut A, &B, &mut C, &mut D): muts at positions 0, 2, 3.
    {
        let mut q = Query::<(&mut A, &B, &mut C, &mut D)>::from_world(&world);
        for (mut a, b, mut c, mut d) in q.iter_mut() {
            a.0 = b.0 + 1000;
            c.0 = b.0 + 2000;
            d.0 = b.0 + 3000;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 1107);
    assert_eq!(world.get::<B>(e).unwrap().0, 107); // unchanged
    assert_eq!(world.get::<C>(e).unwrap().0, 2107);
    assert_eq!(world.get::<D>(e).unwrap().0, 3107);

    // (&A, &mut B, &C, &mut D): alternating mut/read.
    {
        let mut q = Query::<(&A, &mut B, &C, &mut D)>::from_world(&world);
        for (a, mut b, c, mut d) in q.iter_mut() {
            b.0 = a.0 - c.0;
            d.0 = a.0 - c.0;
        }
    }
    assert_eq!(world.get::<A>(e).unwrap().0, 1107); // unchanged
    assert_eq!(world.get::<B>(e).unwrap().0, -1000);
    assert_eq!(world.get::<C>(e).unwrap().0, 2107); // unchanged
    assert_eq!(world.get::<D>(e).unwrap().0, -1000);
}

#[test]
#[should_panic(expected = "conflicting access to component")]
fn query_arity_four_self_conflict_panics() {
    // Arity-4 macro propagates `collect_access` to every position.
    // A regression that stopped emitting one of the access calls
    // would let `(&mut A, &B, &C, &mut A)` slip past the check
    // and double-borrow A's storage cell.
    let mut world = World::new();
    world
        .spawn()
        .insert(A(0))
        .insert(B(0))
        .insert(C(0))
        .insert(D(0));
    let _q = Query::<(&mut A, &B, &C, &mut A)>::from_world(&world);
}

// -------- DenseMut direct coverage --------

#[test]
#[allow(unsafe_code, reason = "exercises DenseMut::get's generation check")]
fn dense_mut_get_rejects_stale_handle_via_generation_check() {
    // Through the `World` API the generation check never fires —
    // despawn cascades clean every storage. The check is defense
    // in depth for direct callers; exercise it here by
    // constructing mismatched arrays by hand.
    use crate::entity::EntityAllocator;
    let mut alloc = EntityAllocator::new();
    let live = alloc.allocate();
    let mut dense = vec![Position(7, 7)];
    let mut changed = vec![0_u32];
    let sparse = vec![Some(0_u32)];
    let entity_index = vec![live];
    let view = DenseMut::<Position>::new(&mut dense, &mut changed, &sparse, &entity_index, 1);
    // SAFETY: single call per entity — no aliasing.
    let live_ref = unsafe { view.get(live) };
    assert!(live_ref.is_some());

    // Manufacture a stale handle: same `index`, different
    // `generation`. Simulates the corruption a direct
    // `ComponentStorage` user could produce by bypassing despawn.
    alloc.destroy(live);
    let fresh = alloc.allocate();
    assert_eq!(live.index, fresh.index);
    assert_ne!(live, fresh);
    let view = DenseMut::<Position>::new(&mut dense, &mut changed, &sparse, &entity_index, 1);
    // SAFETY: distinct entity from the call above (different
    // generation); single call.
    let stale_ref = unsafe { view.get(fresh) };
    assert!(
        stale_ref.is_none(),
        "generation check should reject the stale handle"
    );
}

#[test]
fn dense_join_aliasing_stress_writes_each_slot_once() {
    // Many-entity `(&mut A, &mut B)` join: the non-driver `&mut B` is
    // fetched per entity through `DenseMut::get`, the crate's only
    // `unsafe fn`. Writing through *both* handles on every iteration
    // exercises the "each dense slot is handed out at most once"
    // contract at scale — the property the crate-scoped Miri job
    // (`cargo +nightly miri test -p spark-ecs`) machine-checks against
    // raw-pointer aliasing. A double-borrow of one slot, or an
    // off-by-one in the dense-index lookup the `query/` split touches,
    // surfaces here as a Miri error or a wrong post-condition rather
    // than as silent UB.
    let mut world = World::new();
    let mut ids = Vec::new();
    for i in 0..256 {
        ids.push(world.spawn().insert(A(i)).insert(B(i * 10)).id());
    }

    let mut q = Query::<(&mut A, &mut B)>::from_world(&world);
    let mut visited = 0usize;
    for (mut a, mut b) in q.iter_mut() {
        a.0 += 1;
        b.0 += 1;
        visited += 1;
    }
    // Release the query's exclusive storage borrows before reading back.
    drop(q);
    assert_eq!(visited, ids.len(), "every A∩B entity visited exactly once");

    // Each slot written exactly once: A(i) → i + 1, B(i * 10) → i * 10 + 1.
    // A second write to any slot (the aliasing bug this guards) would
    // show up as `+ 2` here.
    for (i, &e) in ids.iter().enumerate() {
        let i = i32::try_from(i).unwrap();
        assert_eq!(world.get::<A>(e).unwrap().0, i + 1);
        assert_eq!(world.get::<B>(e).unwrap().0, i * 10 + 1);
    }
}
