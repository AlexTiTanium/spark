use super::*;

use crate::access::Access;
use crate::{Changed, Query, World};
// -------- change detection: precise `Mut` marking --------

#[test]
fn join_does_not_overmark_driver_for_unjoined_entities() {
    // The headline fix: in `Query<(&mut Position, &mut Velocity)>` the
    // driver (Position) visits *every* Position entity, but the join
    // drops those lacking Velocity. With `Mut`, a dropped entity's
    // Position is never `DerefMut`'d, so it is NOT marked changed.
    let mut world = World::new();
    let both = world
        .spawn()
        .insert(Position(1, 1))
        .insert(Velocity(1, 1))
        .id(); // Position.changed = 2 (clock 1→2)
    let pos_only = world.spawn().insert(Position(5, 5)).id(); // changed = 3
    let _bump = world.spawn().insert(Position(0, 0)).id(); // clock → 4
    {
        let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(&world);
        for (mut p, mut v) in q.iter_mut() {
            p.0 += 1; // marks Position for the joined entity only
            v.0 += 1;
        }
    }
    let pos = world.storage::<Position>().unwrap();
    // `both` was written → marked at Position's clock (4).
    assert_eq!(pos.changed_tick_for(both), Some(4));
    // `pos_only` was visited by the driver but the join dropped it and
    // the body never wrote it → its mark stays at its insert tick (3).
    assert_eq!(pos.changed_tick_for(pos_only), Some(3));
}

#[test]
fn read_only_iteration_marks_nothing() {
    let mut world = World::new();
    let e = world.spawn().insert(Position(7, 7)).id(); // changed = 2
    {
        let q = Query::<&Position>::from_world(&world);
        assert_eq!(q.iter().count(), 1);
    }
    // The read path never takes a `Mut`, so nothing is marked.
    assert_eq!(
        world.storage::<Position>().unwrap().changed_tick_for(e),
        Some(2)
    );
}

#[test]
fn multi_mut_tuple_marks_only_the_written_component() {
    // `Query<(&mut Position, &mut Velocity)>`, body writes Position
    // (DerefMut) but only reads Velocity (Deref). Run through
    // `run_system` so BOTH clocks advance to 3 first — proving that
    // advancing the clock is *not* what marks: only the `DerefMut`
    // on the non-driver `Velocity` (via `DenseMut::get` → `Mut`) would,
    // and it never happens. Velocity stays at its insert tick.
    let mut world = World::new();
    let e = world
        .spawn()
        .insert(Position(1, 1)) // Position clock → 2
        .insert(Velocity(2, 2)) // Velocity clock → 2
        .id();
    let mut access = Access::new();
    access.components_mut().add_write::<Position>();
    access.components_mut().add_write::<Velocity>();
    let mut last_seen = Vec::new();
    world.run_system(&access, &mut last_seen, &mut |w| {
        let mut q = Query::<(&mut Position, &mut Velocity)>::from_world(w);
        for (mut pos, vel) in q.iter_mut() {
            pos.0 += vel.0; // DerefMut on pos; Deref-only on vel
        }
    });
    // Both clocks advanced 2 → 3; only the written component is stamped.
    assert_eq!(
        world.storage::<Position>().unwrap().changed_tick_for(e),
        Some(3) // written
    );
    assert_eq!(
        world.storage::<Velocity>().unwrap().changed_tick_for(e),
        Some(2) // read-only → stays at its insert tick
    );
}

#[test]
fn changed_query_detects_a_write_across_a_clock_wrap() {
    // End-to-end companion to `filter.rs`'s unit-level wrap tests: drive a real
    // `Query<&T, Changed<T>>` across a `current_tick` wrap and confirm a
    // post-wrap write is still detected through the full Query → filter →
    // storage path. A naive `tick > baseline` would miss it — the post-wrap
    // write tick (0) is numerically *below* the pre-wrap baseline (`u32::MAX`)
    // — while the wrapping-aware `is_changed_since` catches it. (The unit
    // `is_changed_since` boundary is pinned in `filter.rs`; this guards the
    // wiring that feeds it `current` / `tick` / `baseline`.)
    let pos = std::any::TypeId::of::<Position>();
    let mut world = World::new();
    world.spawn().insert(Position(0, 0)); // Position clock 1 → 2

    // Drive the clock to the wrap boundary.
    world
        .storage_mut::<Position>()
        .unwrap()
        .set_current_tick(u32::MAX);

    // Writer run: a write of `Position` advances the clock `u32::MAX` → `0`
    // (the wrap) and stamps the entity's `changed_tick` at the post-wrap `0`.
    let mut writer_access = Access::new();
    writer_access.components_mut().add_write::<Position>();
    let mut writer_baseline = Vec::new();
    world.run_system(&writer_access, &mut writer_baseline, &mut |w| {
        for mut p in Query::<&mut Position>::from_world(w).iter_mut() {
            p.0 += 1;
        }
    });
    assert_eq!(
        world.storage::<Position>().unwrap().current_tick(),
        0,
        "the write advanced the clock past u32::MAX, wrapping it to 0"
    );

    // Observer run: read `Position` filtered by `Changed`, with its baseline
    // parked just *before* the wrap (`u32::MAX`) — what the scheduler would
    // have recorded on the run before the wrap. Empty access ⇒ no clock
    // advance; the parked baseline is fed straight to `Changed::init_state`.
    let observer_access = Access::new();
    let mut observer_baseline = vec![(pos, u32::MAX)];
    let mut seen = 0usize;
    world.run_system(&observer_access, &mut observer_baseline, &mut |w| {
        seen = Query::<&Position, Changed<Position>>::from_world(w)
            .iter()
            .count();
    });
    assert_eq!(seen, 1, "post-wrap write detected across the clock wrap");
}
