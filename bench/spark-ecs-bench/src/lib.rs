#![doc = include_str!("../README.md")]

use spark_ecs::{Component, World};

/// Number of entities every benchmark spawns / iterates / measures.
///
/// Fixed across all benches and all ECS so the numbers are directly
/// comparable.
///
/// # Examples
///
/// ```
/// assert_eq!(spark_ecs_bench::ENTITY_COUNT, 10_000);
/// ```
pub const ENTITY_COUNT: usize = 10_000;

/// A mover's position — read by `iter`, written by `iter_mut`.
///
/// # Examples
///
/// ```
/// use spark_ecs_bench::Position;
///
/// let p = Position { x: 1.0, y: 2.0, z: 3.0 };
/// assert_eq!(p.x + p.y + p.z, 6.0);
/// ```
#[derive(Component, Clone, Copy)]
pub struct Position {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A mover's per-frame velocity — the read side of the `iter_mut`
/// integration `pos += vel`.
///
/// # Examples
///
/// ```
/// use spark_ecs_bench::{Position, Velocity};
///
/// // One step of the integration the `iter_mut` bench measures.
/// let mut pos = Position { x: 0.0, y: 0.0, z: 0.0 };
/// let vel = Velocity { x: 1.0, y: 2.0, z: 3.0 };
/// pos.x += vel.x;
/// pos.y += vel.y;
/// pos.z += vel.z;
/// assert_eq!((pos.x, pos.y, pos.z), (1.0, 2.0, 3.0));
/// ```
#[derive(Component, Clone, Copy)]
pub struct Velocity {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Builds a `World` populated with `count` movers, each carrying a
/// [`Position`] and a [`Velocity`].
///
/// Shared by the `iter` / `iter_mut` benches and the memory binary so
/// every measurement starts from the same populated world.
///
/// # Examples
///
/// ```
/// use spark_ecs::Query;
/// use spark_ecs_bench::{spark_world, Position};
///
/// let world = spark_world(3);
/// let query = Query::<&Position>::from_world(&world);
/// assert_eq!(query.iter().count(), 3);
/// ```
#[must_use]
pub fn spark_world(count: usize) -> World {
    let mut world = World::new();
    for i in 0..count {
        #[allow(clippy::cast_precision_loss)] // seed value only; precision irrelevant
        let f = i as f32;
        world
            .spawn()
            .insert(Position { x: f, y: f, z: f })
            .insert(Velocity {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            });
    }
    world
}

/// Rival-ECS scenario builders, compiled only under `--features external`.
///
/// Each rival lives in its own submodule with its own `Position` /
/// `Velocity` (the component contracts differ between crates) and a
/// `world(count)` builder that produces the same 10k-mover workload as
/// [`spark_world`]. Submodules are named so they don't shadow the rival
/// crate names; rival crates are referenced by absolute path
/// (`::hecs`, `::shipyard`, …).
#[cfg(feature = "external")]
#[allow(clippy::cast_precision_loss)] // index→f32 seeds across every builder
pub mod rivals {
    /// `bevy_ecs` (archetype-based, industry standard).
    pub mod bevy {
        use bevy_ecs::prelude::Component;

        #[derive(Component, Clone, Copy, Debug)]
        pub struct Position {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }
        #[derive(Component, Clone, Copy, Debug)]
        pub struct Velocity {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }

        /// Populates a `bevy_ecs` world with `count` movers.
        #[must_use]
        pub fn world(count: usize) -> bevy_ecs::world::World {
            let mut world = bevy_ecs::world::World::new();
            for i in 0..count {
                let f = i as f32;
                world.spawn((
                    Position { x: f, y: f, z: f },
                    Velocity {
                        x: 1.0,
                        y: 2.0,
                        z: 3.0,
                    },
                ));
            }
            world
        }
    }

    /// `hecs` (archetype-based, minimal API; plain structs are components).
    pub mod hecs {
        #[derive(Clone, Copy, Debug)]
        pub struct Position {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }
        #[derive(Clone, Copy, Debug)]
        pub struct Velocity {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }

        /// Populates a `hecs` world with `count` movers.
        #[must_use]
        pub fn world(count: usize) -> ::hecs::World {
            let mut world = ::hecs::World::new();
            for i in 0..count {
                let f = i as f32;
                world.spawn((
                    Position { x: f, y: f, z: f },
                    Velocity {
                        x: 1.0,
                        y: 2.0,
                        z: 3.0,
                    },
                ));
            }
            world
        }
    }

    /// `shipyard` (sparse-set, like `spark-ecs`; the closest architectural peer).
    pub mod shipyard {
        use ::shipyard::Component;

        #[derive(Component, Clone, Copy, Debug)]
        pub struct Position {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }
        #[derive(Component, Clone, Copy, Debug)]
        pub struct Velocity {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }

        /// Populates a `shipyard` world with `count` movers.
        #[must_use]
        pub fn world(count: usize) -> ::shipyard::World {
            let mut world = ::shipyard::World::new();
            for i in 0..count {
                let f = i as f32;
                world.add_entity((
                    Position { x: f, y: f, z: f },
                    Velocity {
                        x: 1.0,
                        y: 2.0,
                        z: 3.0,
                    },
                ));
            }
            world
        }
    }

    /// `flax` (archetype-based; components are static handles declared via
    /// the `component!` macro rather than plain fields).
    pub mod flax {
        use ::flax::{EntityBuilder, World, component};

        #[derive(Clone, Copy, Debug)]
        pub struct Position {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }
        #[derive(Clone, Copy, Debug)]
        pub struct Velocity {
            pub x: f32,
            pub y: f32,
            pub z: f32,
        }

        component! {
            pub position: Position,
            pub velocity: Velocity,
        }

        /// Populates a `flax` world with `count` movers.
        #[must_use]
        pub fn world(count: usize) -> World {
            let mut world = World::new();
            for i in 0..count {
                let f = i as f32;
                EntityBuilder::new()
                    .set(position(), Position { x: f, y: f, z: f })
                    .set(
                        velocity(),
                        Velocity {
                            x: 1.0,
                            y: 2.0,
                            z: 3.0,
                        },
                    )
                    .spawn(&mut world);
            }
            world
        }
    }
}
