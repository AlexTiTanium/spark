//! Integration test exercising the canonical movement scenario from
//! M3 Issue B: spawn many entities with `Position` + `Velocity`, run
//! the join system that integrates one onto the other, and assert
//! every joined entity advanced exactly once per tick — and that
//! entities lacking `Velocity` (the lonely side) stay put.
//!
//! This is the test `ECS_DESIGN.md` stage 5 calls for: "movement system
//! across 1k entities". It only touches the public surface of
//! `spark-ecs` — `World`, `Query`, `Entity`, `Component` — so it
//! exercises the crate the way a downstream consumer (and, eventually,
//! the `spark` binary) will.

use spark_ecs::{Entity, Query, World};

// `Component` is a blanket-impl marker today (any `T: 'static` is a
// component). The `spark-ecs-macros` PR will switch it to an explicit
// `#[derive(Component)]` + `Send + Sync + 'static` bound — at which
// point these structs need that derive. Mentioned so a future reader
// copy-pasting this test as a template knows what to expect.
#[derive(Debug, PartialEq)]
struct Position(i64, i64);

#[derive(Debug, PartialEq)]
struct Velocity(i64, i64);

/// Spawns `n` movers (with both `Position` and `Velocity`) and `n / 4`
/// drifters (with only `Position`). Returns the parallel id vectors so
/// the test can assert per-entity outcomes.
fn build_world(n: i64) -> (World, Vec<Entity>, Vec<Entity>) {
    let mut world = World::new();
    let mut movers = Vec::with_capacity(usize::try_from(n).unwrap());
    let mut drifters = Vec::new();

    for i in 0..n {
        let entity = world
            .spawn()
            .insert(Position(i, -i))
            .insert(Velocity(1, 2))
            .id();
        movers.push(entity);

        // Every fourth entity is a drifter — Position only, no Velocity.
        // The join must skip them.
        if i % 4 == 0 {
            let drifter = world.spawn().insert(Position(i * 1000, i * 2000)).id();
            drifters.push(drifter);
        }
    }

    (world, movers, drifters)
}

fn step_once(world: &World) {
    let mut q = Query::<(&mut Position, &Velocity)>::from_world(world);
    // Path B: iter yields just `(&mut Position, &Velocity)` — no entity.
    for (pos, vel) in q.iter_mut() {
        pos.0 += vel.0;
        pos.1 += vel.1;
    }
}

#[test]
fn thousand_movers_advance_every_tick() {
    const N: i64 = 1_000;
    const TICKS: i64 = 5;

    let (world, movers, drifters) = build_world(N);

    for _ in 0..TICKS {
        step_once(&world);
    }

    // Each mover started at (i, -i) and accumulated (TICKS, 2*TICKS).
    for (i, entity) in movers.iter().enumerate() {
        let i = i64::try_from(i).unwrap();
        let pos = world.get::<Position>(*entity).unwrap();
        assert_eq!(
            *pos,
            Position(i + TICKS, -i + 2 * TICKS),
            "mover {entity:?} after {TICKS} ticks should have advanced exactly TICKS times"
        );
    }

    // Drifters have no Velocity — the join skipped them, so their
    // Position is the seed value untouched. Drifters were spawned for
    // every fourth mover with seed `(i * 1000, i * 2000)`.
    for (drifter_idx, entity) in drifters.iter().enumerate() {
        let mover_idx = i64::try_from(drifter_idx).unwrap() * 4;
        let pos = world.get::<Position>(*entity).unwrap();
        assert_eq!(
            *pos,
            Position(mover_idx * 1000, mover_idx * 2000),
            "drifter {entity:?} must stay at its spawn position",
        );
    }
}

#[test]
fn read_only_join_count_matches_intersection() {
    let (world, _, _) = build_world(50);
    let q = Query::<(&Position, &Velocity)>::from_world(&world);
    // Every mover has both components; 50 movers spawned.
    assert_eq!(q.iter().count(), 50);
}
