//! Integration test exercising the spawn/insert/despawn lifecycle as a
//! downstream consumer would: only the public API of `spark-ecs` is
//! visible here, no `#[cfg(test)]` cousins. Mirrors the scenario named
//! in the M3 Issue A verification plan: 100 entities, attach `Position`
//! to even-indexed ones, despawn half, assert the storage stays packed
//! and the cascade through `dyn AnyStorage` cleared every component.

use spark_ecs::{Entity, World};

#[derive(Debug, PartialEq)]
struct Position {
    x: i32,
    y: i32,
}

#[derive(Debug)]
struct Walkable;

#[test]
fn hundred_entities_position_on_evens_despawn_half() {
    let mut world = World::new();

    // Spawn 100 entities. Even-indexed get a Position; every entity
    // gets a Walkable marker so the despawn cascade has to walk two
    // storages.
    let mut entities: Vec<Entity> = Vec::with_capacity(100);
    for i in 0..100i32 {
        let mut builder = world.spawn();
        if i % 2 == 0 {
            builder = builder.insert(Position { x: i, y: -i });
        }
        let entity = builder.insert(Walkable).id();
        entities.push(entity);
    }

    // Sanity: components inserted where expected.
    for (i, entity) in entities.iter().enumerate() {
        let has_position = world.get::<Position>(*entity).is_some();
        assert_eq!(has_position, i % 2 == 0, "i = {i}");
        assert!(
            world.get::<Walkable>(*entity).is_some(),
            "every entity should have Walkable at this point"
        );
    }

    // Despawn the first half (entities 0..50).
    for entity in entities.iter().take(50) {
        assert!(world.despawn(*entity));
    }

    // Stale handles fail is_alive cleanly.
    for entity in entities.iter().take(50) {
        assert!(!world.is_alive(*entity));
        assert!(world.get::<Position>(*entity).is_none());
        assert!(world.get::<Walkable>(*entity).is_none());
    }

    // Surviving half is intact, including only the originally-even
    // entries still carrying Position.
    let mut surviving_positions = 0_usize;
    let mut surviving_walkables = 0_usize;
    for (i, entity) in entities.iter().enumerate().skip(50) {
        assert!(world.is_alive(*entity));
        if i % 2 == 0 {
            let pos = world.get::<Position>(*entity).unwrap();
            let i_i32 = i32::try_from(i).unwrap();
            assert_eq!(pos.x, i_i32);
            assert_eq!(pos.y, -i_i32);
            surviving_positions += 1;
        } else {
            assert!(world.get::<Position>(*entity).is_none());
        }
        assert!(world.get::<Walkable>(*entity).is_some());
        surviving_walkables += 1;
    }
    // Even indices in 50..100 → 50, 52, …, 98 = 25 entries.
    assert_eq!(surviving_positions, 25);
    assert_eq!(surviving_walkables, 50);

    // A fresh spawn reuses one of the despawned slots — the new
    // entity must not inherit the previous tenant's components.
    let recycled = world.spawn().id();
    assert!(world.is_alive(recycled));
    assert!(world.get::<Position>(recycled).is_none());
    assert!(world.get::<Walkable>(recycled).is_none());
}
