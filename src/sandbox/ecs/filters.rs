//! Query-filter demo systems — every `Query<D, F>` combination,
//! documented **in place**: each system shows the storage it sees, the
//! entities it keeps, and when you'd reach for that filter.
//!
//! A filter (the second `Query` generic) narrows **which** entities
//! iterate without changing **what** each yields:
//! `Query<&Building, With<Powered>>` still yields `&Building`, just for
//! fewer entities. `F` defaults to `()` (match everything), so
//! `Query<&Building>` is exactly `Query<&Building, ()>`.
//!
//! [`spawn_filter_demo`] seeds the "power-grid" roster every system
//! reads; each system's own docs replay that roster annotated with its
//! own kept / skipped verdict. Run
//! `RUST_LOG=spark=info cargo run -p spark` to see the live matches —
//! each log line carries `expected=` next to the actual `count=`.
//!
//! Every report is first-tick gated and runs in `PreUpdate`, after the
//! `Startup` seed flushes — the one-shot pattern from
//! [`super::systems::report_initial`].

use spark_ecs::{And, Commands, Or, Query, Res, With, Without};
use spark_log::info;

use crate::sandbox::resources::TickCount;

use super::components::{Backup, Building, Capacity, Operational, Powered, UnderMaintenance};

// Naming a filter in a `type` alias keeps call sites readable and is the
// idiomatic way to reuse a filter across systems — it also keeps the
// `Query<…>` parameter under clippy's `type_complexity` threshold. Each
// alias is a plain composition of the shipped filter combinators.

/// Operational **and** not mid-repair.
type Healthy = And<(With<Operational>, Without<UnderMaintenance>)>;

/// Has grid power **or** an on-site backup source.
type AnyPower = Or<(With<Powered>, With<Backup>)>;

/// Operational **and** supplied from some source — `Healthy`'s sibling,
/// showing that an alias (`AnyPower`) nests inside another filter.
type RunningAndSupplied = And<(With<Operational>, AnyPower)>;

/// **`Commands`** — seeds the power-grid roster every filter system
/// reads. Runs in `Startup`; flushes before `PreUpdate` fires.
///
/// After the flush, storage holds (entity → its components):
///
/// ```text
/// substation : Building, Capacity(50),  Powered, Operational
/// plant-a    : Building, Capacity(120), Powered, Operational, UnderMaintenance
/// windfarm   : Building, Capacity(30),  Backup,  Operational
/// depot      : Building, Capacity(10)
/// hybrid     : Building, Capacity(80),  Powered, Backup
/// ```
///
/// The buildings carry no `Position` / `Health`, so they stay out of the
/// physics-demo queries in [`super::systems`].
pub(super) fn spawn_filter_demo(mut commands: Commands) {
    commands
        .spawn()
        .insert(Building { name: "substation" })
        .insert(Capacity(50))
        .insert(Powered)
        .insert(Operational);
    commands
        .spawn()
        .insert(Building { name: "plant-a" })
        .insert(Capacity(120))
        .insert(Powered)
        .insert(Operational)
        .insert(UnderMaintenance);
    commands
        .spawn()
        .insert(Building { name: "windfarm" })
        .insert(Capacity(30))
        .insert(Backup)
        .insert(Operational);
    commands
        .spawn()
        .insert(Building { name: "depot" })
        .insert(Capacity(10));
    commands
        .spawn()
        .insert(Building { name: "hybrid" })
        .insert(Capacity(80))
        .insert(Powered)
        .insert(Backup);
    info!("sandbox/ecs/filters: spawn_filter_demo queued 5 buildings (Commands)");
}

/// **`With<T>` — keep entities that *have* a component, without fetching it.**
///
/// Storage → this filter (`With<Powered>`):
///
/// ```text
/// substation : Powered, Operational                    → kept
/// plant-a    : Powered, Operational, UnderMaintenance  → kept
/// windfarm   : Backup, Operational                     → skipped  (no Powered)
/// depot      : (none)                                  → skipped  (no Powered)
/// hybrid     : Powered, Backup                          → kept
/// ```
///
/// The loop body runs for **substation, plant-a, hybrid** only.
///
/// **When to reach for it:** you want to act on every entity that *has*
/// `Powered` but never read the marker's data (it's zero-sized anyway).
/// `With<Powered>` is cheaper and clearer than joining `&Powered` into
/// the data shape just to ignore it — the marker never enters the item,
/// so the body still binds plain `&Building`.
pub(super) fn with_filter(tick: Res<TickCount>, q: Query<&Building, With<Powered>>) {
    if tick.0 != 0 {
        return;
    }
    let matched: Vec<&str> = q.iter().map(|b| b.name).collect();
    info!(
        ?matched,
        count = matched.len(),
        expected = 3,
        "sandbox/ecs/filters: Query<&Building, With<Powered>> — buildings drawing grid power"
    );
}

/// **`Without<T>` — keep entities that *lack* a component.**
///
/// Storage → this filter (`Without<Powered>`), the exact complement of
/// [`with_filter`]:
///
/// ```text
/// substation : Powered, Operational                    → skipped  (has Powered)
/// plant-a    : Powered, Operational, UnderMaintenance  → skipped  (has Powered)
/// windfarm   : Backup, Operational                     → kept
/// depot      : (none)                                  → kept
/// hybrid     : Powered, Backup                          → skipped  (has Powered)
/// ```
///
/// The loop body runs for **windfarm, depot** only.
///
/// **When to reach for it:** you want the *complement* of a tag — "every
/// building that is **not** on the grid", "every worker **without** a
/// job". It pairs naturally with a marker some other system sets and
/// clears (`Powered`, `CurrentJob`), letting two systems split the world
/// between them with no shared bookkeeping.
pub(super) fn without_filter(tick: Res<TickCount>, q: Query<&Building, Without<Powered>>) {
    if tick.0 != 0 {
        return;
    }
    let matched: Vec<&str> = q.iter().map(|b| b.name).collect();
    info!(
        ?matched,
        count = matched.len(),
        expected = 2,
        "sandbox/ecs/filters: Query<&Building, Without<Powered>> — buildings off the grid"
    );
}

/// **`And<(…)>` — every inner filter must match.**
///
/// Filter is the [`Healthy`] alias = `And<(With<Operational>,
/// Without<UnderMaintenance>)>`. `And` is spelled out (not a bare tuple)
/// so it stays symmetric with [`Or`] and unambiguous when they nest.
///
/// Storage → this filter (operational **and** not under maintenance):
///
/// ```text
/// substation : Powered, Operational                    → kept     (Op ✓, maint ✗)
/// plant-a    : Powered, Operational, UnderMaintenance  → skipped  (under maintenance)
/// windfarm   : Backup, Operational                     → kept     (Op ✓, maint ✗)
/// depot      : (none)                                  → skipped  (not operational)
/// hybrid     : Powered, Backup                          → skipped  (not operational)
/// ```
///
/// The loop body runs for **substation, windfarm** only.
///
/// **When to reach for it:** the condition is a *conjunction* — "has X
/// **and** lacks Y". Mixing `With` and `Without` inside one `And`
/// expresses "running, and not currently being repaired" in a single
/// query instead of an `if` guard inside the loop.
pub(super) fn and_filter(tick: Res<TickCount>, q: Query<&Building, Healthy>) {
    if tick.0 != 0 {
        return;
    }
    let matched: Vec<&str> = q.iter().map(|b| b.name).collect();
    info!(
        ?matched,
        count = matched.len(),
        expected = 2,
        "sandbox/ecs/filters: And<(With<Operational>, Without<UnderMaintenance>)> — healthy & running"
    );
}

/// **`Or<(…)>` — any inner filter matches.**
///
/// Filter is the [`AnyPower`] alias = `Or<(With<Powered>, With<Backup>)>`.
///
/// Storage → this filter (grid power **or** a backup source):
///
/// ```text
/// substation : Powered, Operational                    → kept     (Powered)
/// plant-a    : Powered, Operational, UnderMaintenance  → kept     (Powered)
/// windfarm   : Backup, Operational                     → kept     (Backup)
/// depot      : (none)                                  → skipped  (neither)
/// hybrid     : Powered, Backup                          → kept     (both)
/// ```
///
/// The loop body runs for everyone **except depot**.
///
/// **When to reach for it:** the condition is a *disjunction* — several
/// independent ways to qualify. "Supplied" means grid power **or** a
/// backup generator; `Or` collapses that into one query rather than two
/// passes whose results you'd have to de-duplicate.
pub(super) fn or_filter(tick: Res<TickCount>, q: Query<&Building, AnyPower>) {
    if tick.0 != 0 {
        return;
    }
    let matched: Vec<&str> = q.iter().map(|b| b.name).collect();
    info!(
        ?matched,
        count = matched.len(),
        expected = 4,
        "sandbox/ecs/filters: Or<(With<Powered>, With<Backup>)> — has any power source"
    );
}

/// **Nested `And<(…, Or<(…)>)>` — combinators compose.**
///
/// Filter is the [`RunningAndSupplied`] alias = `And<(With<Operational>,
/// AnyPower)>`, i.e. operational **and** (powered **or** backup). `Or`
/// nests inside `And` because every combinator is itself a `QueryFilter`.
///
/// Storage → this filter:
///
/// ```text
/// substation : Powered, Operational                    → kept     (Op ✓, Powered ✓)
/// plant-a    : Powered, Operational, UnderMaintenance  → kept     (Op ✓, Powered ✓)
/// windfarm   : Backup, Operational                     → kept     (Op ✓, Backup ✓)
/// depot      : (none)                                  → skipped  (not operational)
/// hybrid     : Powered, Backup                          → skipped  (powered, but not operational)
/// ```
///
/// The loop body runs for **substation, plant-a, windfarm** only.
///
/// **When to reach for it:** the real predicate has structure — "running
/// **and** drawing power from *some* source". Naming the inner `Or`
/// ([`AnyPower`]) keeps the outer query readable and lets you reuse the
/// sub-filter (here also used by [`or_filter`]).
pub(super) fn nested_filter(tick: Res<TickCount>, q: Query<&Building, RunningAndSupplied>) {
    if tick.0 != 0 {
        return;
    }
    let matched: Vec<&str> = q.iter().map(|b| b.name).collect();
    info!(
        ?matched,
        count = matched.len(),
        expected = 3,
        "sandbox/ecs/filters: And<(With<Operational>, Or<(With<Powered>, With<Backup>)>)> — running, supplied"
    );
}

/// **Filter over a multi-component shape — the filter never enters the item.**
///
/// Data shape is `(&Building, &Capacity)`; filter is `With<Powered>`. The
/// query yields `(&Building, &Capacity)` — the marker narrows the set but
/// is *not* part of what you read.
///
/// Storage → this filter (`With<Powered>`):
///
/// ```text
/// substation : Capacity(50),  Powered   → kept   → (substation, 50)
/// plant-a    : Capacity(120), Powered   → kept   → (plant-a, 120)
/// windfarm   : Capacity(30),  Backup    → skipped (no Powered)
/// depot      : Capacity(10)             → skipped (no Powered)
/// hybrid     : Capacity(80),  Powered   → kept   → (hybrid, 80)
/// ```
///
/// Sums to **250 MW** across the three powered buildings.
///
/// **When to reach for it:** you *do* need component data (`Capacity`),
/// but only for a subset. The filter keeps the data shape honest — the
/// body binds `(&Building, &Capacity)`, never the `Powered` marker — so
/// the signature reads as "capacity of powered buildings", which is
/// exactly the intent.
pub(super) fn filtered_join(tick: Res<TickCount>, q: Query<(&Building, &Capacity), With<Powered>>) {
    if tick.0 != 0 {
        return;
    }
    let powered: Vec<(&str, u32)> = q.iter().map(|(b, c)| (b.name, c.0)).collect();
    let total: u32 = powered.iter().map(|(_, mw)| mw).sum();
    info!(
        ?powered,
        total_mw = total,
        expected_total_mw = 250,
        "sandbox/ecs/filters: Query<(&Building, &Capacity), With<Powered>> — filtered join"
    );
}

/// **Filtered mutation — write to a subset, gated by a filter.**
///
/// Data shape is `(&Building, &mut Capacity)` (the mut-not-first arity-2
/// shape); filter is `With<Powered>`. Each kept building's capacity is
/// bumped by 10 MW.
///
/// Storage → this filter, then the write:
///
/// ```text
/// substation : Capacity(50),  Powered   → kept   → 50  + 10 = 60
/// plant-a    : Capacity(120), Powered   → kept   → 120 + 10 = 130
/// windfarm   : Capacity(30),  Backup    → skipped (untouched, stays 30)
/// depot      : Capacity(10)             → skipped (untouched, stays 10)
/// hybrid     : Capacity(80),  Powered   → kept   → 80  + 10 = 90
/// ```
///
/// **When to reach for it:** you want to mutate only the entities that
/// pass a condition. The filter does the gating, so the body is an
/// unconditional `capacity.0 += 10` rather than a `for` loop wrapping an
/// `if`. Note the access rule: `With<Powered>` reads `Powered` while the
/// data writes `Capacity` — different components, so no conflict.
/// `Query<&mut Capacity, With<Capacity>>` *would* panic at construction,
/// because `With` reports a read of the very component the data writes.
pub(super) fn bump_powered_capacity(
    tick: Res<TickCount>,
    mut q: Query<(&Building, &mut Capacity), With<Powered>>,
) {
    if tick.0 != 0 {
        return;
    }
    let mut bumped: Vec<(&str, u32)> = Vec::new();
    for (building, capacity) in q.iter_mut() {
        capacity.0 += 10;
        bumped.push((building.name, capacity.0));
    }
    info!(
        ?bumped,
        count = bumped.len(),
        expected = 3,
        "sandbox/ecs/filters: Query<(&Building, &mut Capacity), With<Powered>> — +10 MW to powered only"
    );
}
