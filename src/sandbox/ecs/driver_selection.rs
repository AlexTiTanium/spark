//! Driver-selection demo systems — every query drives off the **smallest
//! candidate set**, chosen once at `Query::from_world`, so iteration cost is
//! proportional to the *result*, not to which element was written first or
//! to the whole live set (issue #65).
//!
//! [`spawn_driver_demo`] seeds a deliberately skewed "grid-node" roster:
//! eight nodes carry `Telemetry`, but only two are `Critical` and two are
//! `Standby`. Each system below names a shape whose *smallest* part is one of
//! those rare markers, so the engine drives off the rare set — never the
//! eight-element `Telemetry` storage. The yielded result is identical to a
//! naive drive; only the work differs.
//!
//! The work saved (driver steps) isn't observable from the binary — the
//! driver-step counter is `#[cfg(test)]` in `spark-ecs` — so each system
//! instead **self-checks its result count** against `expected` and logs a
//! `verdict`, the same scoreboard shape the change-detection demo uses. Run
//! `RUST_LOG=spark=info cargo run -p spark` to see the live verdicts.
//!
//! First-tick gated, runs in `PreUpdate` after the `Startup` seed flushes —
//! the one-shot pattern from [`super::systems::report_initial`].

use spark_ecs::{And, Commands, Entity, Or, Query, Res, With, Without};
use spark_log::info;

use crate::sandbox::resources::TickCount;

use super::components::{Critical, Standby, Telemetry};

// Naming the combinator filters in a `type` alias keeps the `Query<…>`
// parameters readable and under clippy's `type_complexity` threshold — the
// same idiom `super::filters` uses.

/// Critical **and** on standby — the intersection an `And` drives via its
/// *smallest arm*, rejecting non-members per entity.
type CriticalAndStandby = And<(With<Critical>, With<Standby>)>;

/// Critical **or** on standby — the union an `Or` drives, deduplicated so a
/// node in both arms is visited once.
type CriticalOrStandby = Or<(With<Critical>, With<Standby>)>;

/// `"PASS"` when the live result count matches what the roster implies,
/// `"FAIL"` otherwise — the demo's self-check, logged beside `count`.
fn verdict(count: usize, expected: usize) -> &'static str {
    if count == expected { "PASS" } else { "FAIL" }
}

/// **`Commands`** — seeds the skewed driver-selection roster. Runs in
/// `Startup`; flushes before `PreUpdate` fires.
///
/// After the flush, storage holds (node → its components):
///
/// ```text
/// n0 : Telemetry(10), Critical
/// n1 : Telemetry(20), Critical, Standby
/// n2 : Telemetry(30), Standby
/// n3 : Telemetry(40)
/// n4 : Telemetry(50)
/// n5 : Telemetry(60)
/// n6 : Telemetry(70)
/// n7 : Telemetry(80)
/// ```
///
/// Populations: **Telemetry 8, Critical 2, Standby 2** (Critical ∪ Standby =
/// {n0, n1, n2} = 3 after dedup). The markers are far rarer than the data,
/// which is exactly what makes the driver choice visible below.
pub(super) fn spawn_driver_demo(mut commands: Commands) {
    commands.spawn().insert(Telemetry(10)).insert(Critical);
    commands
        .spawn()
        .insert(Telemetry(20))
        .insert(Critical)
        .insert(Standby);
    commands.spawn().insert(Telemetry(30)).insert(Standby);
    commands.spawn().insert(Telemetry(40));
    commands.spawn().insert(Telemetry(50));
    commands.spawn().insert(Telemetry(60));
    commands.spawn().insert(Telemetry(70));
    commands.spawn().insert(Telemetry(80));
    info!("sandbox/ecs/driver-selection: spawn_driver_demo queued 8 grid nodes (Commands)");
}

/// **Tuple — the smallest element drives, even written second.**
///
/// `Query<(&Telemetry, &Critical)>` joins the 8-element `Telemetry` storage
/// with the 2-element `Critical` storage. Before #65 the *first* element
/// (`Telemetry`) drove, walking all 8 and looking `Critical` up each time.
/// Now the smaller candidate drives: `Critical` (2) leads and `Telemetry` is
/// looked up per entity — 2 driver steps, not 8, for the same 2 results
/// (n0, n1, the only nodes with both).
///
/// **When it matters:** you write the shape in whatever order reads best;
/// the engine drives the cheap side regardless. Summing the looked-up
/// `Telemetry` readings (n0 = 10, n1 = 20 → 30) confirms the join fetches the
/// right data, not just the right count.
pub(super) fn tuple_non_first_drives(tick: Res<TickCount>, q: Query<(&Telemetry, &Critical)>) {
    if tick.0 != 0 {
        return;
    }
    let mut count = 0usize;
    let mut total = 0;
    for (telemetry, _critical) in q.iter() {
        total += telemetry.0; // reads the looked-up data element
        count += 1;
    }
    info!(
        count,
        expected = 2,
        total,
        expected_total = 30,
        verdict = verdict(count, 2),
        "sandbox/ecs/driver-selection: Query<(&Telemetry, &Critical)> — Critical (2) drives, not Telemetry (8)"
    );
}

/// **Filter drives — the live set is never walked.**
///
/// `Query<Entity, With<Critical>>` asks for ids, narrowed by a marker.
/// `Entity` offers no candidate (it has no storage), so the filter leads:
/// `With<Critical>` drives off `Critical`'s 2-entity storage rather than
/// snapshotting the whole live set and testing each id. 2 driver steps,
/// 2 results — independent of how many other entities exist in the world.
///
/// **When it matters:** you need the ids of a tagged set (to despawn them,
/// raise an event, store a relationship) and the tag is rare.
pub(super) fn filter_drives(tick: Res<TickCount>, q: Query<Entity, With<Critical>>) {
    if tick.0 != 0 {
        return;
    }
    let count = q.iter().count();
    info!(
        count,
        expected = 2,
        verdict = verdict(count, 2),
        "sandbox/ecs/driver-selection: Query<Entity, With<Critical>> — Critical (2) drives, not the live set"
    );
}

/// **`And` drives its smallest arm, then rejects per entity.**
///
/// Filter is [`CriticalAndStandby`] = `And<(With<Critical>, With<Standby>)>`.
/// `And` is bounded by its tightest arm, so it surfaces the smaller of
/// `Critical` (2) and `Standby` (2) as its candidate — here `Critical` (the
/// earlier arm, on a tie). That 2-node set drives; `matches` then keeps only
/// nodes that *also* have `Standby` → just n1. So 2 driver steps trim to 1
/// result.
///
/// **When it matters:** a conjunction like "critical **and** on standby"
/// drives off whichever side is rarer, never the union of both.
pub(super) fn and_smallest_arm_drives(
    tick: Res<TickCount>,
    q: Query<&Telemetry, CriticalAndStandby>,
) {
    if tick.0 != 0 {
        return;
    }
    let count = q.iter().count();
    info!(
        count,
        expected = 1,
        verdict = verdict(count, 1),
        "sandbox/ecs/driver-selection: And<(With<Critical>, With<Standby>)> — smallest arm drives, intersection kept (1)"
    );
}

/// **`Or` drives the deduplicated union.**
///
/// Filter is [`CriticalOrStandby`] = `Or<(With<Critical>, With<Standby>)>`.
/// The candidate is the *union* of the arms, materialized once at
/// construction and deduplicated — {n0, n1, n2}, with n1 (in both arms)
/// counted once. That 3-node union drives `&Telemetry` (all three carry it)
/// → 3 driver steps, 3 results, never the 8-element `Telemetry` set.
///
/// **When it matters:** a disjunction ("critical **or** standby") drives off
/// the combined small set in one pass, with no double-visiting of overlap.
pub(super) fn or_union_drives(tick: Res<TickCount>, q: Query<&Telemetry, CriticalOrStandby>) {
    if tick.0 != 0 {
        return;
    }
    let count = q.iter().count();
    info!(
        count,
        expected = 3,
        verdict = verdict(count, 3),
        "sandbox/ecs/driver-selection: Or<(With<Critical>, With<Standby>)> — deduplicated union drives (3)"
    );
}

/// **`Without` rejects per entity — a positive part still drives.**
///
/// `Without<Critical>` can't enumerate "lacks `Critical`", so it offers **no**
/// candidate set (a #65 non-goal — there's no smaller list of "everyone
/// except"). The positive data element `&Telemetry` (8) therefore drives, and
/// `Without<Critical>` rejects the 2 critical nodes per entity → 6 results.
/// The exclusion narrows the *result*, not the driver: cost stays ∝ the data
/// element it's paired with, exactly as for an unfiltered `Query<&Telemetry>`.
///
/// **When it matters:** the complement of a tag ("every node *not* flagged
/// critical"). It rides on whatever positive part drives; with nothing
/// positive at all (`Query<Entity, Without<Critical>>`) it would fall back to
/// the live set, which is why this pairs it with `&Telemetry`.
pub(super) fn without_rejects_per_entity(
    tick: Res<TickCount>,
    q: Query<&Telemetry, Without<Critical>>,
) {
    if tick.0 != 0 {
        return;
    }
    let count = q.iter().count();
    info!(
        count,
        expected = 6,
        verdict = verdict(count, 6),
        "sandbox/ecs/driver-selection: Query<&Telemetry, Without<Critical>> — Telemetry drives, Without rejects (6)"
    );
}

/// **Entity-as-data under `Without` — id + a negative filter.**
///
/// `Query<(Entity, &Telemetry), Without<Critical>>`: `Entity` and
/// `Without<Critical>` both offer no candidate, so `&Telemetry` (8) drives,
/// the id rides along on it, and `Without` rejects the 2 critical nodes →
/// 6 ids of non-critical nodes. The driver is the data element, never the
/// live set; the negative filter only trims which of those ids survive.
///
/// **When it matters:** you want the *ids* of "everything except the tagged
/// ones" together with some data — to despawn them, alert on them, etc. —
/// and you already have a data component to drive off.
pub(super) fn entity_without(
    tick: Res<TickCount>,
    q: Query<(Entity, &Telemetry), Without<Critical>>,
) {
    if tick.0 != 0 {
        return;
    }
    let count = q.iter().count();
    info!(
        count,
        expected = 6,
        verdict = verdict(count, 6),
        "sandbox/ecs/driver-selection: Query<(Entity, &Telemetry), Without<Critical>> — id + Without, Telemetry drives (6)"
    );
}
