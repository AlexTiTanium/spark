//! Change-detection sub-demo — `Changed<T>` / `Added<T>` in the live
//! sandbox, plus a deterministic test suite pinning the behaviour across
//! the situations a game actually hits.
//!
//! The live demo is a battery roster: [`recharge_low`] tops up only the
//! batteries below full (a conditional write, so the `Mut<Charge>` marks
//! *only* the ones it actually changes), and [`report_recharged`] reacts
//! with `Query<&Charge, Changed<Charge>>` — logging how many moved this
//! tick. As the pack fills, the count falls to zero and stays there:
//! proof that change detection is *precise*, not "everything every tick".
//!
//! The `#[cfg(test)]` suite drives a real [`Application`] frame by frame
//! and asserts the exact `Changed` / `Added` counts each frame — one test
//! per situation (write/no-write alternation, one-shot `Added`, precise
//! subset writes, per-component independence, `Commands` spawns, remove +
//! re-add, filter composition). Each asserts a full per-frame sequence, so
//! a regression in *when* a component is considered changed fails loudly.

use spark_ecs::{Commands, Query};
use spark_log::{debug, info};

use spark_ecs::Changed;

use super::components::Charge;

/// Seeds the battery roster: three cells at 30 / 60 / 100 percent.
/// Queued via `Commands` in `Startup` so the flush makes them visible
/// before the first `Update`.
pub(super) fn seed_batteries(mut commands: Commands) {
    commands.spawn().insert(Charge(30));
    commands.spawn().insert(Charge(60));
    commands.spawn().insert(Charge(100));
    info!("sandbox/ecs: seed_batteries queued 3 Charge cells (Commands)");
}

/// Tops up every battery below 100 by 20 points (saturating). Iterating
/// `Query<&mut Charge>` yields a `Mut<Charge>`; we write through it only
/// for the cells that are actually low, so only those are marked changed.
pub(super) fn recharge_low(mut q: Query<&mut Charge>) {
    let mut topped = 0;
    for mut charge in q.iter_mut() {
        if charge.0 < 100 {
            charge.0 = (charge.0 + 20).min(100);
            topped += 1;
        }
    }
    debug!(topped, "sandbox/ecs: recharge_low — Query<&mut Charge>");
}

/// Reacts to whatever `recharge_low` actually moved this tick, via
/// `Changed<Charge>`. The count falls to 0 once the pack is full — the
/// precise-marking payoff.
pub(super) fn report_recharged(q: Query<&Charge, Changed<Charge>>) {
    let recharged = q.iter().count();
    info!(
        recharged,
        "sandbox/ecs: report_recharged — Query<&Charge, Changed<Charge>>"
    );
}

#[cfg(test)]
mod tests {
    use spark_core::{Application, Stage};
    use spark_ecs::{Added, And, Changed, Commands, Component, Query, Res, ResMut, Resource, With};

    // Test-local roster — independent of the live-demo components so a
    // change here never perturbs the running sandbox.
    #[derive(Component)]
    struct Hp(u32);
    #[derive(Component)]
    struct Pos(i32);
    #[derive(Component)]
    struct Vel; // marker — its presence + own clock is all the test needs
    #[derive(Component)]
    struct Tag;

    #[derive(Resource)]
    struct Frame(u32);
    /// Records the per-frame observed match count, so a test can assert
    /// the whole sequence at once.
    #[derive(Resource)]
    struct Seen(Vec<usize>);

    fn tick(mut f: ResMut<Frame>) {
        f.0 += 1;
    }

    /// Drives `Stage::Update` `frames` times and returns the recorded
    /// per-frame `Seen` sequence.
    fn run_frames(app: &mut Application, frames: u32) -> Vec<usize> {
        for _ in 0..frames {
            app.run_stage(Stage::Update);
        }
        app.world().resource::<Seen>().0.clone()
    }

    #[test]
    fn changed_fires_only_on_write_frames() {
        // `bump` writes Hp only on even frame numbers; `observe` counts how
        // many Hp changed since it last ran. The expected sequence:
        //   f1: bump skips (f=1), but the first run still sees both
        //       pre-existing entities (clocks start at 1, baseline 0) → 2
        //   f2: bump writes (f=2) → 2
        //   f3: bump skips (f=3), nothing changed since f2 → 0
        //   f4: bump writes (f=4) → 2
        fn bump(f: Res<Frame>, mut q: Query<&mut Hp>) {
            if f.0.is_multiple_of(2) {
                for mut hp in q.iter_mut() {
                    hp.0 += 1;
                }
            }
        }
        fn observe(q: Query<&Hp, Changed<Hp>>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Frame(0)).add_resource(Seen(Vec::new()));
        app.world_mut().spawn().insert(Hp(10));
        app.world_mut().spawn().insert(Hp(20));
        app.add_system(Stage::Update, tick)
            .add_system(Stage::Update, bump)
            .add_system(Stage::Update, observe);

        assert_eq!(run_frames(&mut app, 4), vec![2, 2, 0, 2]);
    }

    #[test]
    fn added_is_one_shot() {
        // A build-time entity is `Added` exactly once (first run), then
        // never again — even though it keeps existing.
        fn observe(q: Query<&Hp, Added<Hp>>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Seen(Vec::new()));
        app.world_mut().spawn().insert(Hp(100));
        app.add_system(Stage::Update, observe);

        assert_eq!(run_frames(&mut app, 3), vec![1, 0, 0]);
    }

    #[test]
    fn iter_mut_marks_only_the_subset_written() {
        // `recharge` tops up only Hp below 100; as cells fill, fewer are
        // marked changed each frame. Precise marking means the count
        // tracks the writes exactly, not the entity total.
        fn recharge(mut q: Query<&mut Hp>) {
            for mut hp in q.iter_mut() {
                if hp.0 < 100 {
                    hp.0 = (hp.0 + 20).min(100);
                }
            }
        }
        fn observe(q: Query<&Hp, Changed<Hp>>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Seen(Vec::new()));
        // Cells at 20 / 60 / 80, topped up +20/frame until full. The
        // observed count tracks how many were *actually* written each
        // frame (precise marking), not the entity total:
        //   f1: all 3 written (also first-run-sees-pre-existing) → 3
        //   f2: 40→60, 80→100, 100 skip          → 2
        //   f3: 60→80, full, full                → 1
        //   f4: 80→100                           → 1
        //   f5: all full                         → 0
        app.world_mut().spawn().insert(Hp(20));
        app.world_mut().spawn().insert(Hp(60));
        app.world_mut().spawn().insert(Hp(80));
        app.add_system(Stage::Update, recharge)
            .add_system(Stage::Update, observe);

        assert_eq!(run_frames(&mut app, 5), vec![3, 2, 1, 1, 0]);
    }

    #[test]
    fn changed_is_per_component_independent() {
        // Writing Pos never makes `Changed<Vel>` fire. Each component's
        // clock is its own.
        fn move_pos(mut q: Query<&mut Pos>) {
            for mut p in q.iter_mut() {
                p.0 += 1;
            }
        }
        fn observe_vel(q: Query<&Vel, Changed<Vel>>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Seen(Vec::new()));
        app.world_mut().spawn().insert(Pos(0)).insert(Vel);
        app.add_system(Stage::Update, move_pos)
            .add_system(Stage::Update, observe_vel);

        // f1: Vel is pre-existing → seen once. f2+: Vel never written →
        // never changed again, despite Pos moving every frame.
        assert_eq!(run_frames(&mut app, 3), vec![1, 0, 0]);
    }

    #[test]
    fn commands_spawn_seen_by_added_next_frame() {
        // A `Commands` spawn flushes after the stage's systems; the
        // `Added` observer (registered last) sees each spawn on the
        // following frame — one new entity per frame here.
        fn spawn_one(mut commands: Commands) {
            commands.spawn().insert(Tag);
        }
        fn observe(q: Query<&Tag, Added<Tag>>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Seen(Vec::new()));
        app.add_system(Stage::Update, spawn_one)
            .add_system(Stage::Update, observe); // last before the flush

        // f1: nothing exists when observe runs → 0; flush creates #1.
        // f2: sees #1 as Added; flush creates #2.
        // f3: sees #2 as Added (not #1 — one-shot); flush creates #3.
        assert_eq!(run_frames(&mut app, 3), vec![0, 1, 1]);
    }

    #[test]
    fn remove_then_readd_fires_added_again() {
        // `Added` is one-shot per attach: stripping `Hp` and re-attaching
        // it makes `Added` fire a second time. The strip / re-add are done
        // through `world_mut()` between frames (the `Commands` API has no
        // component-remove yet), standing in for a despawn-and-respawn.
        fn observe(q: Query<&Hp, Added<Hp>>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Seen(Vec::new()));
        let e = app.world_mut().spawn().insert(Hp(100)).id();
        app.add_system(Stage::Update, observe);

        app.run_stage(Stage::Update); // f1: pre-existing Hp → Added once
        app.world_mut().remove::<Hp>(e); // strip the component
        app.run_stage(Stage::Update); // f2: no Hp present → 0
        app.world_mut().insert(e, Hp(50)); // re-attach (fresh add)
        app.run_stage(Stage::Update); // f3: re-added → Added fires again

        assert_eq!(app.world().resource::<Seen>().0, vec![1, 0, 1]);
    }

    #[test]
    fn and_filter_composes_with_changed() {
        // `And<(With<Tag>, Changed<Hp>)>` — only tagged entities whose Hp
        // changed. The untagged-but-changed entity is excluded.
        // Factored out so the `Query` parameter stays readable (and clear
        // of `clippy::type_complexity`).
        type TaggedAndChanged = And<(With<Tag>, Changed<Hp>)>;
        fn bump_all(mut q: Query<&mut Hp>) {
            for mut hp in q.iter_mut() {
                hp.0 += 1;
            }
        }
        fn observe(q: Query<&Hp, TaggedAndChanged>, mut seen: ResMut<Seen>) {
            seen.0.push(q.iter().count());
        }
        let mut app = Application::new();
        app.add_resource(Seen(Vec::new()));
        app.world_mut().spawn().insert(Hp(1)).insert(Tag); // tagged
        app.world_mut().spawn().insert(Hp(1)); // untagged
        app.add_system(Stage::Update, bump_all)
            .add_system(Stage::Update, observe);

        // Both Hp change every frame, but only the tagged one passes the
        // `And`. (Frame 1 also sees the tagged one as pre-existing.)
        assert_eq!(run_frames(&mut app, 3), vec![1, 1, 1]);
    }
}
