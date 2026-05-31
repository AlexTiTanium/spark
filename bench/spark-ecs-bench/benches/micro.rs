//! Three micro-benchmarks over the core ECS hot paths, comparing
//! `spark-ecs` against every rival wired in behind `external`.
//!
//! - `spawn` — build [`ENTITY_COUNT`] entities, each with a `Position` and
//!   a `Velocity`, into a fresh world. Measures storage allocation.
//! - `iter` — read-only sum over `&Position`. Measures query traversal.
//! - `iter_mut` — `pos += vel` over `(&mut Position, &Velocity)`. Measures
//!   the mutable write path (for `spark-ecs`, the `Mut<T>` change-marker).
//!
//! `iter` / `iter_mut` build the populated world and the query/state
//! *outside* the timed closure wherever the ECS's borrow model allows, so
//! only traversal is measured. The two exceptions — `hecs`'s `query_mut`
//! and `flax`'s `borrow`, which take `&mut World` / re-borrow per call —
//! are commented at their call sites.
//!
//! Each group reports throughput (entities/s) alongside wall time. Run the
//! Spark-only suite with `cargo bench --bench micro`; add `--features
//! external` for the full cross-ECS sweep.

#![allow(clippy::cast_precision_loss)] // index→f32 seeds; precision irrelevant

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};

use spark_ecs::Query;
use spark_ecs_bench::{ENTITY_COUNT, Position, Velocity, spark_world};

/// `spawn`: build `ENTITY_COUNT` two-component entities into a new world.
fn spawn(c: &mut Criterion) {
    let mut group = c.benchmark_group("spawn");
    group.throughput(Throughput::Elements(ENTITY_COUNT as u64));

    group.bench_function("spark-ecs", |b| {
        b.iter(|| black_box(spark_world(ENTITY_COUNT)));
    });

    #[cfg(feature = "external")]
    {
        use spark_ecs_bench::rivals;
        group.bench_function("hecs", |b| {
            b.iter(|| black_box(rivals::hecs::world(ENTITY_COUNT)));
        });
        group.bench_function("bevy_ecs", |b| {
            b.iter(|| black_box(rivals::bevy::world(ENTITY_COUNT)));
        });
        group.bench_function("shipyard", |b| {
            b.iter(|| black_box(rivals::shipyard::world(ENTITY_COUNT)));
        });
        group.bench_function("flax", |b| {
            b.iter(|| black_box(rivals::flax::world(ENTITY_COUNT)));
        });
    }

    group.finish();
}

/// `iter`: read-only sum over `&Position`, traversal only.
fn iter(c: &mut Criterion) {
    let mut group = c.benchmark_group("iter");
    group.throughput(Throughput::Elements(ENTITY_COUNT as u64));

    let world = spark_world(ENTITY_COUNT);
    let query = Query::<&Position>::from_world(&world);
    group.bench_function("spark-ecs", |b| {
        b.iter(|| {
            let mut sum = 0.0f32;
            for p in query.iter() {
                sum += p.x + p.y + p.z;
            }
            black_box(sum);
        });
    });
    drop(query);

    #[cfg(feature = "external")]
    external::iter(&mut group);

    group.finish();
}

/// `iter_mut`: `pos += vel` over `(&mut Position, &Velocity)`, traversal only.
fn iter_mut(c: &mut Criterion) {
    let mut group = c.benchmark_group("iter_mut");
    group.throughput(Throughput::Elements(ENTITY_COUNT as u64));

    let world = spark_world(ENTITY_COUNT);
    let mut query = Query::<(&mut Position, &Velocity)>::from_world(&world);
    group.bench_function("spark-ecs", |b| {
        b.iter(|| {
            let mut acc = 0.0f32;
            for (mut pos, vel) in query.iter_mut() {
                pos.x += vel.x; // DerefMut → stamps the change tick (PR #62 path)
                pos.y += vel.y;
                pos.z += vel.z;
                acc += pos.x; // read-back forces the writes; guards against DCE
            }
            black_box(acc);
        });
    });
    drop(query);

    #[cfg(feature = "external")]
    external::iter_mut(&mut group);

    group.finish();
}

/// Rival-ECS traversal benchmarks, compiled only under `--features external`.
///
/// Component types and world builders come from `spark_ecs_bench::rivals`;
/// this module only holds the per-ECS iteration logic.
#[cfg(feature = "external")]
mod external {
    use std::hint::black_box;

    use criterion::BenchmarkGroup;
    use criterion::measurement::WallTime;

    use spark_ecs_bench::rivals;

    /// Read-only `&Position` traversal for every rival ECS.
    pub fn iter(group: &mut BenchmarkGroup<WallTime>) {
        // hecs: cache the query borrow outside the loop (first `.iter()`
        // scans archetypes once, later samples reuse it).
        {
            use rivals::hecs::Position;
            let world = rivals::hecs::world(super::ENTITY_COUNT);
            let mut query = world.query::<&Position>();
            group.bench_function("hecs", |b| {
                b.iter(|| {
                    let mut sum = 0.0f32;
                    for p in &mut query {
                        sum += p.x + p.y + p.z;
                    }
                    black_box(sum);
                });
            });
        }

        // bevy_ecs: cached `QueryState`, built once outside the loop.
        {
            use rivals::bevy::Position;
            let mut world = rivals::bevy::world(super::ENTITY_COUNT);
            let mut query = world.query::<&Position>();
            group.bench_function("bevy_ecs", |b| {
                b.iter(|| {
                    let mut sum = 0.0f32;
                    for p in query.iter(&world) {
                        sum += p.x + p.y + p.z;
                    }
                    black_box(sum);
                });
            });
        }

        // shipyard: borrow the View once outside the loop.
        {
            use ::shipyard::{IntoIter, View};
            use rivals::shipyard::Position;
            let world = rivals::shipyard::world(super::ENTITY_COUNT);
            let positions = world.borrow::<View<Position>>().unwrap();
            group.bench_function("shipyard", |b| {
                b.iter(|| {
                    let mut sum = 0.0f32;
                    for p in positions.iter() {
                        sum += p.x + p.y + p.z;
                    }
                    black_box(sum);
                });
            });
        }

        // flax: `Query` built outside; `borrow(&world)` is re-issued per
        // sample (flax's borrow is the per-access step, like hecs).
        {
            use ::flax::Query;
            use rivals::flax::position;
            let world = rivals::flax::world(super::ENTITY_COUNT);
            let mut query = Query::new(position());
            group.bench_function("flax", |b| {
                b.iter(|| {
                    let mut sum = 0.0f32;
                    for p in &mut query.borrow(&world) {
                        sum += p.x + p.y + p.z;
                    }
                    black_box(sum);
                });
            });
        }
    }

    /// `pos += vel` traversal for every rival ECS. `acc += pos.x` reads each
    /// post-write value to defeat dead-code elimination, applied uniformly.
    pub fn iter_mut(group: &mut BenchmarkGroup<WallTime>) {
        // hecs: `query_mut` takes `&mut World`, so it can't be hoisted —
        // re-issuing per sample is inherent to hecs's exclusive-borrow model.
        {
            use rivals::hecs::{Position, Velocity};
            let mut world = rivals::hecs::world(super::ENTITY_COUNT);
            group.bench_function("hecs", |b| {
                b.iter(|| {
                    let mut acc = 0.0f32;
                    for (pos, vel) in world.query_mut::<(&mut Position, &Velocity)>() {
                        pos.x += vel.x;
                        pos.y += vel.y;
                        pos.z += vel.z;
                        acc += pos.x;
                    }
                    black_box(acc);
                });
            });
        }

        // bevy_ecs: cached `QueryState`, built once outside the loop.
        {
            use rivals::bevy::{Position, Velocity};
            let mut world = rivals::bevy::world(super::ENTITY_COUNT);
            let mut query = world.query::<(&mut Position, &Velocity)>();
            group.bench_function("bevy_ecs", |b| {
                b.iter(|| {
                    let mut acc = 0.0f32;
                    for (mut pos, vel) in query.iter_mut(&mut world) {
                        pos.x += vel.x;
                        pos.y += vel.y;
                        pos.z += vel.z;
                        acc += pos.x;
                    }
                    black_box(acc);
                });
            });
        }

        // shipyard: borrow ViewMut<Position> + View<Velocity> once outside.
        {
            use ::shipyard::{IntoIter, View, ViewMut};
            use rivals::shipyard::{Position, Velocity};
            let world = rivals::shipyard::world(super::ENTITY_COUNT);
            let mut positions = world.borrow::<ViewMut<Position>>().unwrap();
            let velocities = world.borrow::<View<Velocity>>().unwrap();
            group.bench_function("shipyard", |b| {
                b.iter(|| {
                    let mut acc = 0.0f32;
                    for (pos, vel) in (&mut positions, &velocities).iter() {
                        pos.x += vel.x;
                        pos.y += vel.y;
                        pos.z += vel.z;
                        acc += pos.x;
                    }
                    black_box(acc);
                });
            });
        }

        // flax: `Query` with a mutable position view, built once outside.
        {
            use ::flax::Query;
            use rivals::flax::{position, velocity};
            let world = rivals::flax::world(super::ENTITY_COUNT);
            let mut query = Query::new((position().as_mut(), velocity()));
            group.bench_function("flax", |b| {
                b.iter(|| {
                    let mut acc = 0.0f32;
                    for (pos, vel) in &mut query.borrow(&world) {
                        pos.x += vel.x;
                        pos.y += vel.y;
                        pos.z += vel.z;
                        acc += pos.x;
                    }
                    black_box(acc);
                });
            });
        }
    }
}

criterion_group!(benches, spawn, iter, iter_mut);
criterion_main!(benches);
