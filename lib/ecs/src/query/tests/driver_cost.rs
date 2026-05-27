//! Deterministic cost-contract checks for issue #65: every shape drives a
//! number of *driver steps* proportional to its smallest candidate, not to
//! the live set or to which element was written first. Counting is exact
//! (a `#[cfg(test)]` per-advance counter), so these assertions are
//! noise-free — wall-clock benchmarks are deferred to #63.

use super::*;
use crate::filter::{Added, And, Changed, Or, With, Without};
use crate::query::take_driver_steps;
use crate::world::World;
use crate::{Entity, Query};

#[derive(Component)]
struct Big;
#[derive(Component)]
struct Small;
#[derive(Component)]
struct One;
/// Never inserted — its storage is absent (population 0).
#[derive(Component)]
struct Phantom;
/// A valued component, for tests that assert *written values*, not just
/// driver-step counts.
#[derive(Component)]
struct Val(i32);
#[derive(Component)]
struct Tag;

// 10_000 entities hold `Big`; the first 50 of those also hold `Small`; a
// single separate entity holds only `One`. So the populations are
// distinct — Big 10_000, Small 50, One 1 — and the live set is 10_001.
const BIG: usize = 10_000;
const SMALL: usize = 50;
const LIVE: usize = BIG + 1;

fn world() -> World {
    let mut w = World::new();
    for i in 0..BIG {
        let e = w.spawn().insert(Big).id();
        if i < SMALL {
            w.insert(e, Small);
        }
    }
    w.spawn().insert(One);
    w
}

/// Runs `body` (which must fully consume a query iterator) and returns how
/// many driver advances it cost.
fn steps(body: impl FnOnce()) -> usize {
    let _ = take_driver_steps(); // clear any residue
    body();
    take_driver_steps()
}

// ---- the contract matrix: driver steps ∝ smallest candidate ----

#[test]
fn entity_with_filter_drives_filter_population() {
    let w = world();
    let n = steps(|| {
        let q = Query::<Entity, With<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL); // result correctness
    });
    assert_eq!(n, SMALL); // not LIVE
}

#[test]
fn ref_with_filter_drives_smaller_of_data_and_filter() {
    let w = world();
    // Filter (50) beats data (10_000): the filter leads.
    let n = steps(|| {
        let q = Query::<&Big, With<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn tuple_drives_smallest_component_even_when_not_first() {
    let w = world();
    // `Small` is the *second* element yet drives — the headline win.
    let n = steps(|| {
        let q = Query::<(&Big, &Small)>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL); // not BIG
}

#[test]
fn tuple_first_element_smallest_keeps_direct_path() {
    let w = world();
    // `Small` first: today's direct path, same step count.
    let n = steps(|| {
        let q = Query::<(&Small, &Big)>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn entity_prefixed_tuple_drives_smallest_component() {
    let w = world();
    let n = steps(|| {
        let q = Query::<(Entity, &Big, &Small)>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn optional_element_offers_no_candidate_and_never_drives() {
    let w = world();
    // `Small` (50) is the smaller storage, but as `Option<&Small>` it
    // narrows nothing — it must NOT drive, or the 9_950 `Big`-only
    // entities would be wrongly dropped. `Big` drives all 10_000 rows.
    let n = steps(|| {
        let q = Query::<(&Big, Option<&Small>)>::from_world(&w);
        assert_eq!(q.iter().count(), BIG); // every Big entity, not just 50
    });
    assert_eq!(n, BIG); // drives Big, never the optional Small
}

#[test]
fn required_drives_with_optional_riding_along() {
    let w = world();
    // `Small` required drives (50); the huge `Big` rides as `Option`.
    let n = steps(|| {
        let q = Query::<(&Small, Option<&Big>)>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL); // not BIG, not LIVE
}

#[test]
fn optional_at_arity_three_never_drives_smallest_required_does() {
    let w = world();
    // `Small` (50) is the smallest *required* element and drives even from
    // the second position; `Option<&One>` offers no candidate. Cost is 50,
    // not BIG, not LIVE — and this exercises the `drive_ref(Data(k>0))`
    // slice path with an optional sitting in the slices array.
    let n = steps(|| {
        let q = Query::<(&Big, &Small, Option<&One>)>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn non_first_driver_resolves_optional_values_correctly() {
    // Value-correctness companion to the step-count test above: when a
    // non-first required element drives (`Small` at index 1), the optional
    // at index 2 must still resolve per the driven entities.
    let mut w = world();
    // One entity carries Big + Small + One, so its optional yields `Some`.
    w.spawn().insert(Big).insert(Small).insert(One);
    let q = Query::<(&Big, &Small, Option<&One>)>::from_world(&w);
    let (mut some, mut none) = (0usize, 0usize);
    for (_b, _s, one) in q.iter() {
        if one.is_some() {
            some += 1;
        } else {
            none += 1;
        }
    }
    assert_eq!(some, 1); // exactly the entity we just added
    assert_eq!(none, SMALL); // the original 50 Small-holders lack One
}

#[test]
fn filter_drives_optional_bearing_data_shape() {
    let w = world();
    // `With<Small>` (50) beats the `&Big` data candidate (10_000), so the
    // FILTER drives even though the data shape carries an optional. The
    // `Option<&One>` rides along (always `None` here) and never gates.
    let n = steps(|| {
        let q = Query::<(&Big, Option<&One>), With<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL); // filter-driven, not the 10_000 Big data set
}

#[test]
fn mut_tuple_drives_smallest_component() {
    let w = world();
    // `&mut Big` first, `&Small` second and smaller — the smaller leads,
    // `Big` is reached through the `DenseMut` lookup.
    let n = steps(|| {
        let mut q = Query::<(&mut Big, &Small)>::from_world(&w);
        assert_eq!(q.iter_mut().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn and_drives_smallest_arm() {
    let w = world();
    // min(|Small| 50, |One| 1) = 1. No entity has both, so 0 results —
    // but the driver still costs only the smallest arm.
    let n = steps(|| {
        let q = Query::<Entity, And<(With<Small>, With<One>)>>::from_world(&w);
        assert_eq!(q.iter().count(), 0);
    });
    assert_eq!(n, 1);
}

#[test]
fn and_with_data_drives_smallest_candidate_across_all() {
    let w = world();
    // min over &Big (10_000), Small (50), One (1) = 1.
    let n = steps(|| {
        let q = Query::<&Big, And<(With<Small>, With<One>)>>::from_world(&w);
        let _ = q.iter().count();
    });
    assert_eq!(n, 1);
}

#[test]
fn shallow_or_drives_deduplicated_union() {
    let w = world();
    // Small (50) and One (1) are disjoint → union is 51.
    let n = steps(|| {
        let q = Query::<Entity, Or<(With<Small>, With<One>)>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL + 1);
    });
    assert_eq!(n, SMALL + 1);
}

#[test]
fn changed_drives_component_population_never_live_set() {
    let w = world();
    // No system has run → baseline 0 → all 50 `Small` count as changed.
    // Driver is Small (50), never the live set, even paired with &Big.
    let n = steps(|| {
        let q = Query::<&Big, Changed<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn added_drives_component_population() {
    let w = world();
    let n = steps(|| {
        let q = Query::<&Big, Added<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

// ---- exempt shapes: no candidate ⇒ live-set drive (by necessity) ----

#[test]
fn standalone_entity_drives_live_set() {
    let w = world();
    let n = steps(|| {
        let q = Query::<Entity>::from_world(&w);
        assert_eq!(q.iter().count(), LIVE);
    });
    assert_eq!(n, LIVE);
}

#[test]
fn without_only_falls_back_to_live_set() {
    let w = world();
    // `Without` enumerates no candidate; with nothing positive elsewhere
    // it drives the live set and rejects per entity (exempt by design).
    let n = steps(|| {
        let q = Query::<Entity, Without<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), LIVE - SMALL);
    });
    assert_eq!(n, LIVE);
}

// ---- natural form ≤ best hand-rewrite (same driver steps) ----

#[test]
fn natural_form_matches_best_hand_rewrite() {
    let w = world();
    // `Query<Entity, With<Small>>` is the natural form; `Query<(Entity,
    // &Small)>` is the hand-rewrite a user reaches for today. Equal cost.
    let natural = steps(|| {
        let _ = Query::<Entity, With<Small>>::from_world(&w).iter().count();
    });
    let rewrite = steps(|| {
        let _ = Query::<(Entity, &Small)>::from_world(&w).iter().count();
    });
    assert_eq!(natural, rewrite);
    assert_eq!(natural, SMALL);
}

// ---- correctness: reordering the driver doesn't change the result set ----

#[test]
fn driver_choice_preserves_result_set() {
    let w = world();
    let forward: usize = Query::<(&Big, &Small)>::from_world(&w).iter().count();
    let reversed: usize = Query::<(&Small, &Big)>::from_world(&w).iter().count();
    assert_eq!(forward, reversed);
    assert_eq!(forward, SMALL);
}

// ---- value correctness through a filter-/reorder-chosen driver ----

// Six `Val` entities; indices 0 and 3 also carry `Tag`. So `Tag` (2)
// drives over `Val` (6) — exercising the `&mut` filter-driven path
// (`DenseMut` reached via an *external* filter driver, not a non-first
// data element) and asserting the *written values*, not just the count.
fn world_six_vals() -> (World, Vec<Entity>) {
    let mut w = World::new();
    let mut ids = Vec::new();
    for i in 0..6 {
        let e = w.spawn().insert(Val(i)).id();
        if i % 3 == 0 {
            w.insert(e, Tag);
        }
        ids.push(e);
    }
    (w, ids)
}

#[test]
fn filter_driven_mut_writes_only_matching_values() {
    let (w, ids) = world_six_vals();
    let n = steps(|| {
        let mut q = Query::<&mut Val, With<Tag>>::from_world(&w);
        for mut v in q.iter_mut() {
            v.0 += 100;
        }
    });
    assert_eq!(n, 2); // filter (Tag) drives, not all six Vals
    let vals: Vec<i32> = ids.iter().map(|&e| w.get::<Val>(e).unwrap().0).collect();
    assert_eq!(vals, vec![100, 1, 2, 103, 4, 5]); // only the Tag-holders bumped
}

#[test]
fn entity_prefixed_mut_drives_smallest_and_writes_through() {
    let (w, ids) = world_six_vals();
    let n = steps(|| {
        let mut q = Query::<(Entity, &mut Val, &Tag)>::from_world(&w);
        let mut seen = Vec::new();
        for (e, mut v, _tag) in q.iter_mut() {
            v.0 += 100;
            seen.push(e);
        }
        assert_eq!(seen.len(), 2);
    });
    assert_eq!(n, 2); // `Tag` (global index 2) drives the exclusive path
    let vals: Vec<i32> = ids.iter().map(|&e| w.get::<Val>(e).unwrap().0).collect();
    assert_eq!(vals, vec![100, 1, 2, 103, 4, 5]);
}

// ---- edge cases in the candidate machinery ----

#[test]
fn or_with_absent_arm_drives_only_the_present_arm() {
    let w = world();
    // `Phantom`'s storage is absent ⇒ its `candidate_slice` is `Some(&[])`,
    // so the union is `Small (50) ∪ ∅` = 50 — never the live set.
    let n = steps(|| {
        let q = Query::<Entity, Or<(With<Small>, With<Phantom>)>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL);
}

#[test]
fn and_with_negative_arm_drives_the_positive_arm() {
    let w = world();
    // `Without<Big>` offers no candidate (it `flatten`s out of
    // `And::candidate_slice`), so the positive `With<Small>` arm (50)
    // drives. Every Small-holder also has Big, so `matches` rejects all
    // 50 — 0 results, but the driver cost is the smallest positive arm.
    let n = steps(|| {
        let q = Query::<Entity, And<(With<Small>, Without<Big>)>>::from_world(&w);
        assert_eq!(q.iter().count(), 0);
    });
    assert_eq!(n, SMALL); // drove Small's 50, not the live set
}

#[test]
fn equal_population_tie_breaks_to_earliest_element() {
    // `L` and `R` cover the same 3 entities but in opposite dense order
    // (`R` inserted in reverse). Equal populations ⇒ the tie breaks to the
    // *first* element, so the yielded order is the first element's order.
    #[derive(Component)]
    struct L(i32);
    #[derive(Component)]
    struct R;

    let mut w = World::new();
    let e0 = w.spawn().insert(L(0)).id();
    let e1 = w.spawn().insert(L(1)).id();
    let e2 = w.spawn().insert(L(2)).id();
    w.insert(e2, R);
    w.insert(e1, R);
    w.insert(e0, R); // R's dense order is [e2, e1, e0]

    // `(&L, &R)`: tie → L (first) drives → L's dense order [0, 1, 2].
    let forward: Vec<i32> = Query::<(&L, &R)>::from_world(&w)
        .iter()
        .map(|(l, _)| l.0)
        .collect();
    assert_eq!(forward, vec![0, 1, 2]);

    // `(&R, &L)`: tie → R (first) drives → R's dense order [2, 1, 0].
    let reversed: Vec<i32> = Query::<(&R, &L)>::from_world(&w)
        .iter()
        .map(|(_, l)| l.0)
        .collect();
    assert_eq!(reversed, vec![2, 1, 0]);
}

// ---- the frozen plan replays correctly across multiple iterations ----

#[test]
fn or_filter_plan_replays_identically_on_second_iter() {
    let w = world();
    // The `Or` union is materialized once at construction; a second
    // `iter()` must borrow the same cached `Vec` and yield the same set
    // at the same cost — not rebuild it (cycle-1 perf fix) nor drift.
    let q = Query::<Entity, Or<(With<Small>, With<One>)>>::from_world(&w);
    let first: Vec<Entity> = q.iter().collect();
    let _ = take_driver_steps();
    let second: Vec<Entity> = q.iter().collect();
    let n2 = take_driver_steps();
    assert_eq!(first, second);
    assert_eq!(n2, SMALL + 1);
}

#[test]
fn non_first_data_driver_replays_on_second_iter_mut() {
    let w = world();
    // `Small` (2nd, smaller) drives; `Big` is reached via `DenseMut` from
    // the still-live `RefMut`. A second `iter_mut()` re-borrows through
    // the same guard and costs the same.
    let mut q = Query::<(&mut Big, &Small)>::from_world(&w);
    let first = q.iter_mut().count();
    let _ = take_driver_steps();
    let second = q.iter_mut().count();
    let n2 = take_driver_steps();
    assert_eq!(first, SMALL);
    assert_eq!(second, SMALL);
    assert_eq!(n2, SMALL);
}

#[test]
fn or_three_arms_drives_union_of_all_three() {
    // Exercises the arity-3 `candidate_len` sum and `candidate_materialize`
    // union (three disjoint arms: Small 50, One 1, Trio 3).
    #[derive(Component)]
    struct Trio;
    let mut w = world();
    for _ in 0..3 {
        w.spawn().insert(Trio);
    }
    let n = steps(|| {
        let q = Query::<Entity, Or<(With<Small>, With<One>, With<Trio>)>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL + 1 + 3);
    });
    assert_eq!(n, SMALL + 1 + 3);
}

#[test]
fn multi_mut_tuple_drives_smaller_and_writes_both() {
    // `(&mut Val, &mut Val2)` where `Val2` is smaller: it drives off its
    // own slice and *both* elements are written through `DenseMut`.
    #[derive(Component)]
    struct Val2(i32);
    let mut w = World::new();
    let mut ids = Vec::new();
    for i in 0..6 {
        let e = w.spawn().insert(Val(i)).id();
        if i % 3 == 0 {
            w.insert(e, Val2(i * 10)); // idx 0, 3 carry Val2
        }
        ids.push(e);
    }
    let n = steps(|| {
        let mut q = Query::<(&mut Val, &mut Val2)>::from_world(&w);
        for (mut v, mut v2) in q.iter_mut() {
            v.0 += 1;
            v2.0 += 100;
        }
    });
    assert_eq!(n, 2); // Val2 (2) drives, not Val (6)
    // The two joined entities had both written; the rest are untouched.
    assert_eq!(w.get::<Val>(ids[0]).unwrap().0, 1);
    assert_eq!(w.get::<Val2>(ids[0]).unwrap().0, 100);
    assert_eq!(w.get::<Val>(ids[3]).unwrap().0, 4);
    assert_eq!(w.get::<Val2>(ids[3]).unwrap().0, 130);
    assert_eq!(w.get::<Val>(ids[1]).unwrap().0, 1); // Val(1), no Val2 → unchanged
    assert_eq!(w.get::<Val>(ids[2]).unwrap().0, 2); // unchanged
}

#[test]
fn or_mut_with_overlapping_arms_visits_each_entity_once() {
    // Soundness pin: an entity in BOTH `Or` arms must be visited once,
    // never twice — `Or::candidate_materialize`'s dedup is what keeps
    // `DenseMut::get` from handing out two aliasing `&mut OD` to the same
    // dense slot. If the dedup regressed, this would over-count steps and
    // double-increment (or be UB under Miri).
    #[derive(Component)]
    struct OA;
    #[derive(Component)]
    struct OB;
    #[derive(Component)]
    struct OD(i32);

    let mut w = World::new();
    let e0 = w.spawn().insert(OA).insert(OB).insert(OD(1)).id(); // in BOTH arms
    let e1 = w.spawn().insert(OA).insert(OD(2)).id();
    let e2 = w.spawn().insert(OB).insert(OD(3)).id();
    // Extra OD-only entities so OD's population (8) exceeds the `Or`
    // candidate sum (|OA| + |OB| = 4) — forcing the `Or` union to be the
    // driver, which is the path under test. Each is paired with its
    // expected (untouched) value so the assertion needs no `usize as i32`.
    let others: Vec<(Entity, i32)> = (0..5)
        .map(|v| (w.spawn().insert(OD(10 + v)).id(), 10 + v))
        .collect();

    let n = steps(|| {
        let mut q = Query::<&mut OD, Or<(With<OA>, With<OB>)>>::from_world(&w);
        for mut d in q.iter_mut() {
            d.0 += 100;
        }
    });
    // Union {e0,e1} ∪ {e0,e2} dedups to {e0,e1,e2} = 3 driver steps — e0
    // appears once, so its `&mut OD` is handed out exactly once.
    assert_eq!(n, 3);
    assert_eq!(w.get::<OD>(e0).unwrap().0, 101); // visited exactly once
    assert_eq!(w.get::<OD>(e1).unwrap().0, 102);
    assert_eq!(w.get::<OD>(e2).unwrap().0, 103);
    for (e, expected) in others {
        assert_eq!(w.get::<OD>(e).unwrap().0, expected); // excluded, untouched
    }
}

#[test]
fn changed_driver_steps_equal_candidate_while_matches_trims() {
    // `Changed<Small>` drives `Small`'s full population (the candidate),
    // and the per-entity tick check trims the result *below* that — the
    // step count tracks the candidate, the result tracks the matches.
    use crate::Access;
    use crate::storage::AnyStorage;
    use std::any::TypeId;

    let mut w = World::new();
    let mut small_ids = Vec::new();
    for _ in 0..5 {
        small_ids.push(w.spawn().insert(Big).insert(Small).id());
    }
    for _ in 0..10 {
        w.spawn().insert(Big); // Big 15, Small 5 → Changed<Small> drives
    }

    // Baseline = Small's clock now: every current holder looks "seen".
    let baseline = w.storage::<Small>().unwrap().current_tick();
    let small_tid = TypeId::of::<Small>();

    // Re-insert Small on 2 → their `changed_tick` advances past baseline.
    w.insert(small_ids[0], Small);
    w.insert(small_ids[1], Small);

    let mut results = 0;
    let mut driver_steps = 0;
    let access = Access::new();
    let mut last_seen = vec![(small_tid, baseline)];
    w.run_system(&access, &mut last_seen, &mut |world| {
        let _ = take_driver_steps();
        let q = Query::<&Big, Changed<Small>>::from_world(world);
        results = q.iter().count();
        driver_steps = take_driver_steps();
    });
    assert_eq!(results, 2); // only the 2 re-inserted Smalls
    assert_eq!(driver_steps, 5); // but the driver walked all 5 Small, never Big's 15
}

// ---- `Without` as a per-entity reject within a driven query ----

#[test]
fn without_rejects_within_data_driven_query() {
    let w = world();
    // `Without<Small>` offers no candidate, so the positive `&Big`
    // (10_000) drives and `Without` rejects the 50 Small-holders per
    // entity. The exclusion shrinks the *result*, not the driver.
    let n = steps(|| {
        let q = Query::<&Big, Without<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), BIG - SMALL);
    });
    assert_eq!(n, BIG); // driver is the data element; `Without` can't shrink it
}

#[test]
fn entity_prefixed_without_rejects_per_entity() {
    let w = world();
    // `Query<(Entity, &Big), Without<Small>>`: `Entity` and `Without`
    // offer no candidate, so `&Big` drives and the id rides along while
    // `Without<Small>` rejects per entity.
    let n = steps(|| {
        let q = Query::<(Entity, &Big), Without<Small>>::from_world(&w);
        assert_eq!(q.iter().count(), BIG - SMALL);
    });
    assert_eq!(n, BIG);
}

#[test]
fn and_changed_with_without_drives_the_changed_arm() {
    let w = world();
    // `And<(Changed<Small>, Without<One>)>`: `Changed<Small>` surfaces
    // Small's 50 entities; `Without<One>` surfaces nothing — so the `And`
    // drives the change-filter arm (50), never the 10_000-element `&Big`.
    // No system ran, so baseline 0 makes every Small "changed"; none of
    // the Small-holders carry `One`, so all 50 survive both arms.
    let n = steps(|| {
        let q = Query::<&Big, And<(Changed<Small>, Without<One>)>>::from_world(&w);
        assert_eq!(q.iter().count(), SMALL);
    });
    assert_eq!(n, SMALL); // the Changed arm drives, not the data element
}
