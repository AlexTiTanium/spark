//! Integration coverage for the workload authoring layer (issue #34):
//! the `#[derive(WorkloadLabel)]` macro, handle/label `.after`/`.before`
//! ordering at both levels, `.any_order_with`, the conflict-policy and
//! cycle error messages, and lazy label resolution.
//!
//! These exercise only the public API, the way a plugin would.

use spark_ecs::{ResMut, Resource, Schedule, WorkloadLabel, World};

/// Records the order systems run in, so a test can assert it.
#[derive(Resource, Default)]
struct Log(Vec<&'static str>);

/// A resource two systems can fight over, to provoke a conflict.
#[derive(Resource)]
struct Shared(u32);

/// Runs `f`, returning its panic message as a `String`. Silences the
/// default hook so the captured-panic tests don't spew to stderr.
fn panic_message(f: impl FnOnce()) -> String {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    std::panic::set_hook(previous);
    let payload = result.expect_err("expected a panic");
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&'static str>()
                .map(|s| (*s).to_string())
        })
        .unwrap_or_default()
}

// ── derive(WorkloadLabel) ───────────────────────────────────────────────

#[test]
fn derive_generates_distinct_ids_per_variant() {
    #[derive(WorkloadLabel)]
    enum Grid {
        Supply,
        Distribute,
        Cleanup,
    }
    assert_ne!(Grid::Supply.id(), Grid::Distribute.id());
    assert_ne!(Grid::Distribute.id(), Grid::Cleanup.id());
    assert_eq!(Grid::Supply.id(), Grid::Supply.id());
}

#[test]
fn derive_names_are_qualified() {
    #[derive(WorkloadLabel)]
    enum Sim {
        Input,
        Physics,
    }
    assert_eq!(Sim::Input.name(), "Sim::Input");
    assert_eq!(Sim::Physics.name(), "Sim::Physics");
}

#[test]
fn derive_ids_differ_across_enums_with_same_variant_index() {
    #[derive(WorkloadLabel)]
    enum A {
        First,
    }
    #[derive(WorkloadLabel)]
    enum B {
        First,
    }
    // Same variant index (0), different enum TypeId ⇒ different identity.
    assert_ne!(A::First.id(), B::First.id());
}

// ── system ordering by handle ───────────────────────────────────────────

#[test]
fn after_orders_systems_within_a_workload() {
    #[derive(WorkloadLabel)]
    enum W {
        A,
    }
    fn first(mut log: ResMut<Log>) {
        log.0.push("first");
    }
    fn second(mut log: ResMut<Log>) {
        log.0.push("second");
    }

    let mut world = World::new();
    world.add_resource(Log::default());
    let mut schedule = Schedule::new();
    schedule.add_workload(W::A, |w| {
        let f = w.add_system(first);
        w.add_system(second).after(f);
    });
    schedule.run(&mut world);
    assert_eq!(world.resource::<Log>().0, vec!["first", "second"]);
}

#[test]
fn before_orders_systems_against_registration_order() {
    #[derive(WorkloadLabel)]
    enum W {
        A,
    }
    fn early(mut log: ResMut<Log>) {
        log.0.push("early");
    }
    fn late(mut log: ResMut<Log>) {
        log.0.push("late");
    }

    let mut world = World::new();
    world.add_resource(Log::default());
    let mut schedule = Schedule::new();
    schedule.add_workload(W::A, |w| {
        // `early` is registered first but ordered to run before `late`
        // via a `.before` declared on `late` itself.
        let e = w.add_system(early);
        w.add_system(late).before(e); // late → early
    });
    schedule.run(&mut world);
    assert_eq!(world.resource::<Log>().0, vec!["late", "early"]);
}

#[test]
fn diamond_join_waits_for_both_branches() {
    #[derive(WorkloadLabel)]
    enum Assets {
        Load,
    }
    fn files(mut log: ResMut<Log>) {
        log.0.push("files");
    }
    fn meshes(mut log: ResMut<Log>) {
        log.0.push("meshes");
    }
    fn textures(mut log: ResMut<Log>) {
        log.0.push("textures");
    }
    fn upload(mut log: ResMut<Log>) {
        log.0.push("upload");
    }

    let mut world = World::new();
    world.add_resource(Log::default());
    let mut schedule = Schedule::new();
    schedule.add_workload(Assets::Load, |w| {
        let files = w.add_system(files);
        let meshes = w.add_system(meshes).after(files);
        let textures = w.add_system(textures).after(files).any_order_with(meshes);
        // `.after(meshes).after(textures)` accumulates: upload waits on both.
        w.add_system(upload).after(meshes).after(textures);
    });
    schedule.run(&mut world);

    let order = &world.resource::<Log>().0;
    let pos = |name| order.iter().position(|&n| n == name).unwrap();
    assert_eq!(order[0], "files"); // root first
    assert_eq!(order[3], "upload"); // join last
    assert!(pos("files") < pos("meshes") && pos("files") < pos("textures"));
    assert!(pos("meshes") < pos("upload") && pos("textures") < pos("upload"));
}

// ── workload ordering by label, lazy resolution ─────────────────────────

#[test]
fn workloads_run_in_label_order_with_forward_reference() {
    #[derive(WorkloadLabel)]
    enum Grid {
        Supply,
        Distribute,
    }
    fn supply(mut log: ResMut<Log>) {
        log.0.push("supply");
    }
    fn distribute(mut log: ResMut<Log>) {
        log.0.push("distribute");
    }

    let mut world = World::new();
    world.add_resource(Log::default());
    let mut schedule = Schedule::new();
    // Distribute is registered first but ordered after Supply — the label
    // resolves lazily at build, so the forward reference is fine.
    schedule
        .add_workload(Grid::Distribute, |w| {
            w.add_system(distribute);
        })
        .after(Grid::Supply);
    schedule.add_workload(Grid::Supply, |w| {
        w.add_system(supply);
    });
    schedule.run(&mut world);
    assert_eq!(world.resource::<Log>().0, vec!["supply", "distribute"]);
}

// ── any_order_with silences a conflict ───────────────────────────────────

#[test]
fn any_order_with_silences_a_system_conflict() {
    #[derive(WorkloadLabel)]
    enum W {
        A,
    }
    fn sweep(mut s: ResMut<Shared>) {
        s.0 += 1;
    }
    fn compact(mut s: ResMut<Shared>) {
        s.0 += 1;
    }

    let mut world = World::new();
    world.add_resource(Shared(0));
    let mut schedule = Schedule::new();
    schedule.add_workload(W::A, |w| {
        let sweep = w.add_system(sweep);
        w.add_system(compact).any_order_with(sweep);
    });
    schedule.run(&mut world); // no panic
    assert_eq!(world.resource::<Shared>().0, 2);
}

#[test]
fn any_order_with_silences_a_workload_conflict() {
    #[derive(WorkloadLabel)]
    enum W {
        Grid,
        Workers,
    }
    fn grid_tick(mut s: ResMut<Shared>) {
        s.0 += 1;
    }
    fn workers_tick(mut s: ResMut<Shared>) {
        s.0 += 10;
    }

    let mut world = World::new();
    world.add_resource(Shared(0));
    let mut schedule = Schedule::new();
    schedule.add_workload(W::Grid, |w| {
        w.add_system(grid_tick);
    });
    schedule
        .add_workload(W::Workers, |w| {
            w.add_system(workers_tick);
        })
        .any_order_with(W::Grid); // both write Shared; any order is fine
    schedule.run(&mut world); // no panic
    assert_eq!(world.resource::<Shared>().0, 11);
}

// ── conflict-policy error messages (pinned) ─────────────────────────────

#[test]
fn undeclared_system_conflict_is_a_registration_error() {
    #[derive(WorkloadLabel)]
    enum W {
        A,
    }
    fn writer(mut s: ResMut<Shared>) {
        s.0 += 1;
    }
    fn other(mut s: ResMut<Shared>) {
        s.0 += 1;
    }

    let message = panic_message(|| {
        let mut world = World::new();
        world.add_resource(Shared(0));
        let mut schedule = Schedule::new();
        schedule.add_workload(W::A, |w| {
            w.add_system(writer);
            w.add_system(other); // conflicts on Shared, no order declared
        });
        schedule.run(&mut world);
    });
    assert!(message.contains("both write resource"), "{message}");
    assert!(message.contains("no order is declared"), "{message}");
    assert!(message.contains(".any_order_with(handle)"), "{message}");
}

#[test]
fn undeclared_workload_conflict_is_a_registration_error() {
    #[derive(WorkloadLabel)]
    enum W {
        Distribute,
        Tick,
    }
    fn distribute(mut s: ResMut<Shared>) {
        s.0 += 1;
    }
    fn tick(mut s: ResMut<Shared>) {
        s.0 += 1;
    }

    let message = panic_message(|| {
        let mut world = World::new();
        world.add_resource(Shared(0));
        let mut schedule = Schedule::new();
        schedule.add_workload(W::Distribute, |w| {
            w.add_system(distribute);
        });
        schedule.add_workload(W::Tick, |w| {
            w.add_system(tick);
        });
        schedule.run(&mut world);
    });
    assert!(message.contains("conflict on write/write of"), "{message}");
    assert!(
        message.contains("no order is declared between them"),
        "{message}"
    );
    assert!(
        message.contains(".any_order_with(WorkloadLabel)"),
        "{message}"
    );
}

// ── cycle detection (pinned) ────────────────────────────────────────────

#[test]
fn workload_cycle_is_reported() {
    #[derive(WorkloadLabel)]
    enum W {
        A,
        B,
    }
    fn noop() {}

    let message = panic_message(|| {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule
            .add_workload(W::A, |w| {
                w.add_system(noop);
            })
            .after(W::B);
        schedule
            .add_workload(W::B, |w| {
                w.add_system(noop);
            })
            .after(W::A); // A after B and B after A ⇒ cycle
        schedule.run(&mut world);
    });
    assert!(
        message.contains("Cycle detected in workload ordering"),
        "{message}"
    );
    assert!(message.contains("W::A"), "{message}");
}

#[test]
fn system_cycle_is_reported() {
    #[derive(WorkloadLabel)]
    enum W {
        A,
    }
    fn one() {}
    fn two() {}

    let message = panic_message(|| {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule.add_workload(W::A, |w| {
            let first = w.add_system(one);
            w.add_system(two).after(first).before(first); // first → two → first
        });
        schedule.run(&mut world);
    });
    assert!(
        message.contains("Cycle detected in system ordering"),
        "{message}"
    );
}

// ── unknown label ───────────────────────────────────────────────────────

#[test]
fn ordering_against_an_unregistered_label_is_an_error() {
    #[derive(WorkloadLabel)]
    enum Real {
        Here,
    }
    #[derive(WorkloadLabel)]
    enum Ghost {
        Missing,
    }
    fn noop() {}

    let message = panic_message(|| {
        let mut world = World::new();
        let mut schedule = Schedule::new();
        schedule
            .add_workload(Real::Here, |w| {
                w.add_system(noop);
            })
            .after(Ghost::Missing); // never registered
        schedule.run(&mut world);
    });
    assert!(message.contains("Unknown workload label"), "{message}");
    assert!(message.contains("Ghost::Missing"), "{message}");
}

// ── regression: review-found edge cases ─────────────────────────────────

#[test]
fn any_order_pairs_with_a_backward_edge_resolve_to_a_valid_order() {
    // Three systems all write Shared (pairwise conflict). One explicit
    // backward edge (`s2.before(s0)`) plus two `.any_order_with`
    // acknowledgements. There IS a consistent order (s2 before s0, the
    // any-order pairs in either order), so this must run cleanly — the
    // declared `.after`/`.before` graph is acyclic, and `.any_order_with`
    // adds no edges that could contradict it.
    #[derive(WorkloadLabel)]
    enum Cleanup {
        Sweep,
    }
    fn s0(mut s: ResMut<Shared>) {
        s.0 += 1;
    }
    fn s1(mut s: ResMut<Shared>) {
        s.0 += 1;
    }
    fn s2(mut s: ResMut<Shared>) {
        s.0 += 1;
    }

    let mut world = World::new();
    world.add_resource(Shared(0));
    let mut schedule = Schedule::new();
    schedule.add_workload(Cleanup::Sweep, |w| {
        let a = w.add_system(s0);
        let b = w.add_system(s1).any_order_with(a);
        w.add_system(s2).before(a).any_order_with(b); // s2 before s0
    });
    schedule.run(&mut world); // no panic — a valid order exists
    assert_eq!(world.resource::<Shared>().0, 3); // all three ran exactly once
}

#[test]
#[cfg(debug_assertions)] // the cross-workload guard is debug-only
fn a_handle_used_in_another_workload_panics_in_debug() {
    use spark_ecs::SystemRef; // only this debug-only test needs the bare handle type

    #[derive(WorkloadLabel)]
    enum A {
        First,
    }
    #[derive(WorkloadLabel)]
    enum B {
        Second,
    }
    fn one() {}
    fn two() {}

    let message = panic_message(|| {
        let mut schedule = Schedule::new();
        // Detach the builder to a plain `SystemRef` (via `into`) so the
        // handle outlives the closure — the only way to smuggle it out.
        let mut stolen: Option<SystemRef> = None;
        schedule.add_workload(A::First, |w| {
            stolen = Some(w.add_system(one).into());
        });
        schedule.add_workload(B::Second, |w| {
            // `stolen` belongs to A::First — feeding it here is the footgun.
            w.add_system(two).after(stolen.expect("captured above"));
        });
    });
    assert!(message.contains("different workload"), "{message}");
}

#[test]
fn registering_a_label_twice_is_rejected() {
    #[derive(WorkloadLabel)]
    enum Grid {
        Supply,
    }
    fn one() {}
    fn two() {}

    let message = panic_message(|| {
        let mut schedule = Schedule::new();
        schedule.add_workload(Grid::Supply, |w| {
            w.add_system(one);
        });
        schedule.add_workload(Grid::Supply, |w| {
            w.add_system(two);
        });
    });
    assert!(message.contains("registered twice"), "{message}");
    assert!(message.contains("Grid::Supply"), "{message}");
}
