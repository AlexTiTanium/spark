//! Headless integration of the M3 Issue C bits — [`Commands`],
//! per-stage flushing, and a custom runner that ticks N frames in
//! place of the real [`WindowPlugin`](spark_window::WindowPlugin).
//!
//! Mirrors the `tick_n` pattern from issue #12: replace the
//! window-driven runner with a closure that calls
//! `run_stage(PreUpdate) → run_stage(Update) →
//! run_stage(PostUpdate)` N times, then asserts post-tick state
//! through `app.world()`. This is the testing pattern the engine
//! ships with for CI; it never opens a window.

use spark_core::{Application, EngineError, Plugin, Stage};
use spark_ecs::{Commands, Component, Query};

#[derive(Debug, PartialEq, Component)]
struct Position(i32, i32);

#[derive(Debug, PartialEq, Component)]
struct Velocity(i32, i32);

/// Seeds two entities via `Commands` in Startup; the flush at the
/// stage boundary makes them visible to the per-frame stages.
fn spawn_two(mut commands: Commands) {
    commands
        .spawn()
        .insert(Position(0, 0))
        .insert(Velocity(1, 0));
    commands
        .spawn()
        .insert(Position(10, 10))
        .insert(Velocity(0, 2));
}

/// Canonical movement system: `Query<(&mut Position, &Velocity)>`.
fn integrate(mut q: Query<(&mut Position, &Velocity)>) {
    for (mut pos, vel) in q.iter_mut() {
        pos.0 += vel.0;
        pos.1 += vel.1;
    }
}

/// Test-only plugin: registers the seeds + the movement system, no
/// runner — the test installs its own headless tick-N runner.
struct DemoPlugin;
impl Plugin for DemoPlugin {
    fn build(&self, app: &mut Application) {
        app.add_system(Stage::Startup, spawn_two)
            .add_system(Stage::Update, integrate);
    }
}

/// Returns a runner that ticks `PreUpdate → Update → PostUpdate`
/// exactly `n` times, then returns. Drop-in replacement for
/// `WindowPlugin` in tests.
fn tick_n(n: u32) -> impl FnOnce(Application) -> Result<(), EngineError> {
    move |mut app| {
        for _ in 0..n {
            app.run_stage(Stage::PreUpdate);
            app.run_stage(Stage::Update);
            app.run_stage(Stage::PostUpdate);
        }
        Ok(())
    }
}

#[test]
fn ten_ticks_advance_positions_by_velocity_times_ten() {
    // Manual-tick variant — skips `app.run()` (which would move the
    // Application into the runner and leave the test-level `app`
    // empty). This proves `run_stage` ticks + flush works end-to-end;
    // the runner-via-`tick_n` path is covered by the two tests below.
    const TICKS: u32 = 10;
    let mut app = Application::new();
    app.add_plugin(DemoPlugin);

    // Manual Startup — no `add_startup_system` closures here, so
    // calling the stage directly suffices.
    app.run_stage(Stage::Startup);

    for _ in 0..TICKS {
        app.run_stage(Stage::PreUpdate);
        app.run_stage(Stage::Update);
        app.run_stage(Stage::PostUpdate);
    }

    // After Startup flush, two entities live; Update ran TICKS times.
    // Entity 1: pos started (0,0), vel (1,0). After 10 ticks: (10, 0).
    // Entity 2: pos started (10,10), vel (0,2). After 10 ticks: (10, 30).
    let world = app.world();
    let q = Query::<&Position>::from_world(world);
    let positions: Vec<(i32, i32)> = q.iter().map(|p| (p.0, p.1)).collect();
    assert_eq!(positions.len(), 2);
    assert!(
        positions.contains(&(10, 0)),
        "entity 1 should have advanced (1, 0) × 10 ticks → (10, 0); got {positions:?}"
    );
    assert!(
        positions.contains(&(10, 30)),
        "entity 2 should have advanced (0, 2) × 10 ticks → (10, 30); got {positions:?}"
    );
}

#[test]
fn commands_spawn_in_startup_visible_in_first_frame() {
    // First-frame visibility is the contract: Startup's flush happens
    // *before* the runner's first tick, so PreUpdate sees the
    // entities `spawn_two` queued.
    let mut app = Application::new();
    app.add_plugin(DemoPlugin);

    let count_cell = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let count_cell_clone = count_cell.clone();
    app.add_system(Stage::PreUpdate, move |q: Query<&Position>| {
        count_cell_clone.set(q.iter().count());
    });
    app.set_runner(tick_n(1));
    app.run().unwrap();

    assert_eq!(
        count_cell.get(),
        2,
        "Startup entities must be visible in the first PreUpdate"
    );
}

#[test]
fn commands_despawn_during_update_visible_in_post_update() {
    // Despawn queued in Update → flushed at Update boundary →
    // invisible by the time PostUpdate runs.
    let mut app = Application::new();
    app.add_plugin(DemoPlugin);

    // Update: despawn everything. Need the entity ids; capture them
    // via a Startup-then-frame side channel. Simpler: spawn one entity
    // in Startup through a separate path, then despawn it in Update.
    let id_cell = std::rc::Rc::new(std::cell::Cell::new(None));
    let id_cell_a = id_cell.clone();
    app.add_system(Stage::Startup, move |mut commands: Commands| {
        let id = commands.spawn().insert(Position(99, 99)).id();
        id_cell_a.set(Some(id));
    });
    let id_cell_b = id_cell.clone();
    app.add_system(Stage::Update, move |mut commands: Commands| {
        if let Some(id) = id_cell_b.get() {
            commands.despawn(id);
        }
    });

    let post_count = std::rc::Rc::new(std::cell::Cell::new(0_usize));
    let post_count_clone = post_count.clone();
    app.add_system(Stage::PostUpdate, move |q: Query<&Position>| {
        // After Update's flush, the (99, 99) entity should be
        // gone. The two demo entities from `spawn_two` survive.
        post_count_clone.set(q.iter().filter(|p| p.0 == 99 && p.1 == 99).count());
    });
    app.set_runner(tick_n(1));
    app.run().unwrap();

    assert_eq!(
        post_count.get(),
        0,
        "(99, 99) entity should have despawned before PostUpdate"
    );
}
