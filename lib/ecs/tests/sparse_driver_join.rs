//! Integration coverage for multi-mut joins over partially-overlapping
//! component populations (issue #80 Phase 0 regression net).
//!
//! The multi-mut unit tests in `query/tests/joins.rs` use tiny or
//! single-entity sets, where the driving storage *is* the intersection.
//! This pins the harder case the `query.rs` → `query/` split most risked
//! breaking: a
//! `(&mut A, &mut B, &mut C)` join whose **driver** (the smallest storage,
//! chosen by `min_data_candidate`) is far sparser than the non-driver
//! storages, so most matched rows are reached by the per-entity
//! dense-index lookup (`DenseMut::get`), not by the driver's linear walk.
//!
//! The two contracts under test:
//!   1. every entity in the A∩B∩C intersection is written **exactly once**
//!      (a double-visit would show as `+ 2` on a field), and
//!   2. every entity **outside** the intersection — including rows the
//!      driver visits but the join rejects (e.g. a `C`-only entity the
//!      driver walks, then drops for lacking `A`/`B`) — is left untouched.
//!
//! Only the public surface is exercised, the way a game system would.

use spark_ecs::{Component, Entity, Query, World};

#[derive(Debug, PartialEq, Component)]
struct A(i64);
#[derive(Debug, PartialEq, Component)]
struct B(i64);
#[derive(Debug, PartialEq, Component)]
struct C(i64);

/// The entity groups a test cares about, captured at spawn so outcomes can
/// be asserted per-group afterwards.
struct Populations {
    /// A ∩ B ∩ C — the only group the join matches.
    full: Vec<Entity>,
    /// A ∩ B, no C — visited only if A or B drives (they don't here).
    ab: Vec<Entity>,
    /// A only.
    a_only: Vec<Entity>,
    /// B only.
    b_only: Vec<Entity>,
    /// C only — the driver (C) walks these, then drops them for lacking A/B.
    c_only: Vec<Entity>,
    /// A ∩ C, no B — also walked by the C driver, then dropped for lacking B.
    ac: Vec<Entity>,
}

/// Spawns a deliberately lopsided overlap so `C` is the smallest storage
/// and therefore the join driver:
///
/// ```text
/// |A| = full + ab + a_only + ac = 30 + 60 + 50 + 5 = 145
/// |B| = full + ab + b_only      = 30 + 60 + 40      = 130
/// |C| = full + c_only + ac      = 30 +  8 +  5      =  43   ← smallest ⇒ drives
/// ```
///
/// Every component is seeded to `0`, so "written once" reads as `1` and
/// "untouched" reads as `0`.
fn build_overlap() -> (World, Populations) {
    let mut world = World::new();
    // Each `map(...).collect()` borrows `world` only for its own statement,
    // so the spawns can share `&mut world` sequentially.
    let full: Vec<Entity> = (0..30)
        .map(|_| world.spawn().insert(A(0)).insert(B(0)).insert(C(0)).id())
        .collect();
    let ab: Vec<Entity> = (0..60)
        .map(|_| world.spawn().insert(A(0)).insert(B(0)).id())
        .collect();
    let a_only: Vec<Entity> = (0..50).map(|_| world.spawn().insert(A(0)).id()).collect();
    let b_only: Vec<Entity> = (0..40).map(|_| world.spawn().insert(B(0)).id()).collect();
    let c_only: Vec<Entity> = (0..8).map(|_| world.spawn().insert(C(0)).id()).collect();
    let ac: Vec<Entity> = (0..5)
        .map(|_| world.spawn().insert(A(0)).insert(C(0)).id())
        .collect();

    (
        world,
        Populations {
            full,
            ab,
            a_only,
            b_only,
            c_only,
            ac,
        },
    )
}

#[test]
fn sparse_driver_multi_mut_join_writes_only_the_intersection() {
    let (world, pops) = build_overlap();

    let mut q = Query::<(&mut A, &mut B, &mut C)>::from_world(&world);
    let mut visited = 0usize;
    for (mut a, mut b, mut c) in q.iter_mut() {
        a.0 += 1;
        b.0 += 1;
        c.0 += 1;
        visited += 1;
    }
    // Release the query's exclusive storage borrows before reading back.
    drop(q);

    // Exactly the A∩B∩C intersection was matched.
    assert_eq!(
        visited,
        pops.full.len(),
        "only the A∩B∩C intersection should be joined, regardless of which storage drove"
    );

    // Matched rows: every present field bumped exactly once.
    for &e in &pops.full {
        assert_eq!(
            world.get::<A>(e).unwrap().0,
            1,
            "full entity A written once"
        );
        assert_eq!(
            world.get::<B>(e).unwrap().0,
            1,
            "full entity B written once"
        );
        assert_eq!(
            world.get::<C>(e).unwrap().0,
            1,
            "full entity C written once"
        );
    }

    // Non-matched rows must be untouched — including `c_only`/`ac`, which the
    // C driver *visits* before the join rejects them: a rejected row must
    // write nothing through any of its `&mut` handles.
    for &e in &pops.ab {
        assert_eq!(world.get::<A>(e).unwrap().0, 0);
        assert_eq!(world.get::<B>(e).unwrap().0, 0);
    }
    for &e in &pops.a_only {
        assert_eq!(world.get::<A>(e).unwrap().0, 0);
    }
    for &e in &pops.b_only {
        assert_eq!(world.get::<B>(e).unwrap().0, 0);
    }
    for &e in &pops.c_only {
        assert_eq!(
            world.get::<C>(e).unwrap().0,
            0,
            "driver-visited but rejected row stays untouched"
        );
    }
    for &e in &pops.ac {
        assert_eq!(world.get::<A>(e).unwrap().0, 0);
        assert_eq!(
            world.get::<C>(e).unwrap().0,
            0,
            "row rejected for lacking B writes no C"
        );
    }
}

#[test]
fn read_only_overlap_join_counts_the_intersection() {
    // The read path (`iter`) over the same overlap must agree with the
    // mutable path on the intersection size.
    let (world, pops) = build_overlap();
    let q = Query::<(&A, &B, &C)>::from_world(&world);
    assert_eq!(q.iter().count(), pops.full.len());
}
