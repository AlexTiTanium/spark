//! The world's **data** — every component the lessons use, in one place.
//!
//! In an ECS, a *component* is just a piece of data you bolt onto an
//! entity: a number, or sometimes nothing at all (a zero-sized "flag" that
//! says "this entity is a such-and-such"). Components hold no logic — the
//! *systems* (in the lesson files next door) do all the work. Keeping the
//! two apart is the whole idea behind an ECS, so this file is the cast of
//! "nouns" and the lesson files are the "verbs".
//!
//! They're grouped by the lesson that uses them. Two flavours appear:
//!
//! - **value components** like `LineLoad(u32)` — carry a number.
//! - **flag components** like `Surveyed` — carry nothing; their mere
//!   *presence* is the information. (`Changed` / `Added` still work on a
//!   flag: each component, even an empty one, carries its own change clock.)
//!
//! Everything is `pub(super)` so the sibling lesson modules can spawn and
//! read these, but nothing leaks outside the change-detection demo.

use spark_ecs::Component;

// ───────────────────────── Lesson 1 — reacting ──────────────────────────

/// Live power flowing through a transmission line, in MW. The `line-telemetry`
/// example re-measures it every tick, so it always counts as changed.
#[derive(Component)]
pub(super) struct LineLoad(pub(super) u32);

/// Flag: this map tile's terrain survey has finished. Attached once and
/// never touched again — the `survey-cache` example uses it to show
/// `Changed` going quiet.
#[derive(Component)]
pub(super) struct Surveyed;

/// A power plant's metered output, in MW. The `plant-output` /
/// `plant-commission` examples overwrite it every tick to contrast
/// `Changed` (keeps firing) with `Added` (fires once).
#[derive(Component)]
pub(super) struct Output(pub(super) u32);

/// A storage cell's charge, 0–100%. The `battery-bank` example tops up only
/// the cells below full, showing that change marking is *precise*.
#[derive(Component)]
pub(super) struct BatteryCharge(pub(super) u32);

// ─────────────────── Lesson 2 — reacting across components ───────────────

/// Fuel left in a plant's hopper, burned down each tick (`refuel-dispatch`).
#[derive(Component)]
pub(super) struct FuelLevel(pub(super) u32);
/// A standing order to refill the hopper — written for plants whose
/// [`FuelLevel`] moved (`refuel-dispatch`).
#[derive(Component)]
pub(super) struct RefuelOrder(pub(super) u32);

/// Voltage on a grid node, re-measured every tick (`grid-solver`).
#[derive(Component)]
pub(super) struct BusVoltage(pub(super) u32);
/// Flag: this node carries a transformer — one half of a real substation
/// (`grid-solver`).
#[derive(Component)]
pub(super) struct Transformer;
/// Flag: this node is wired to a feeder — the other half (`grid-solver`).
#[derive(Component)]
pub(super) struct Feeder;

/// Power passing through a metering station, updated each tick
/// (`energy-toll`).
#[derive(Component)]
pub(super) struct Throughput(pub(super) u32);
/// Running total of energy billed at a station — grows when its
/// [`Throughput`] moves (`energy-toll`).
#[derive(Component)]
pub(super) struct EnergySold(pub(super) u32);

/// Ticks until a plant is next serviced — counted down when its wear moves
/// (`service-schedule`).
#[derive(Component)]
pub(super) struct ServiceCountdown(pub(super) u32);
/// A plant's accumulated wear, rising each tick it runs (`service-schedule`).
#[derive(Component)]
pub(super) struct WearLevel(pub(super) u32);

/// A substation's cable temperature, recomputed when its load moves
/// (`substation-heat`).
#[derive(Component)]
pub(super) struct CableTemp(pub(super) u32);
/// A substation's coil temperature, recomputed alongside the cable
/// (`substation-heat`).
#[derive(Component)]
pub(super) struct CoilTemp(pub(super) u32);
/// The live load signal that drives the thermal recompute (`substation-heat`).
#[derive(Component)]
pub(super) struct LoadSignal(pub(super) u32);
/// Flag: this substation's [`LoadSignal`] is energised (re-sent each tick).
#[derive(Component)]
pub(super) struct Energised;

/// Power flowing through a transmission segment, updated each tick
/// (`segment-loss` / `new-segment`).
#[derive(Component)]
pub(super) struct SegmentLoad(pub(super) u32);
/// Flag: this segment terminates at an endpoint — and the subject of the
/// "newly laid" check (`new-segment`).
#[derive(Component)]
pub(super) struct Endpoint;

// ──────────────────── Lesson 3 — combining conditions ────────────────────

/// A city's power demand, recomputed each tick (`billing` / `offgrid-alert`).
#[derive(Component)]
pub(super) struct CityDemand(pub(super) u32);
/// Flag: this city is wired to the grid (`billing` / `offgrid-alert`).
#[derive(Component)]
pub(super) struct Connected;

/// Flag: a hybrid renewable site — the thing the `hybrid-output` query lists.
#[derive(Component)]
pub(super) struct HybridSite;
/// A site's solar output, updated where the sun is out (`hybrid-output`).
#[derive(Component)]
pub(super) struct SolarYield(pub(super) u32);
/// A site's wind output, updated where the wind blows (`hybrid-output`).
#[derive(Component)]
pub(super) struct WindYield(pub(super) u32);
/// Flag: this site's [`SolarYield`] is updated each tick (`hybrid-output`).
#[derive(Component)]
pub(super) struct SunExposed;
/// Flag: this site's [`WindYield`] is updated each tick (`hybrid-output`).
#[derive(Component)]
pub(super) struct WindExposed;

/// Flag: a nuclear plant — the thing the `safety-interlock` query lists.
#[derive(Component)]
pub(super) struct NuclearPlant;
/// Reactor fuel-rod position, stepped where the rods are cycling
/// (`safety-interlock`).
#[derive(Component)]
pub(super) struct FuelRods(pub(super) u32);
/// Coolant flow reading, stepped where the loop is circulating
/// (`safety-interlock`).
#[derive(Component)]
pub(super) struct Coolant(pub(super) u32);
/// Flag: this plant's [`FuelRods`] step each tick (`safety-interlock`).
#[derive(Component)]
pub(super) struct RodsCycling;
/// Flag: this plant's [`Coolant`] circulates each tick (`safety-interlock`).
#[derive(Component)]
pub(super) struct CoolantCirculating;

/// Flag: a generator — the thing the `ops-dashboard` query lists.
#[derive(Component)]
pub(super) struct Generator;
/// A generator's streamed telemetry, updated where it's reporting
/// (`ops-dashboard`).
#[derive(Component)]
pub(super) struct Telemetry(pub(super) u32);
/// Flag: this generator was commissioned this session — a one-time `Added`
/// signal (`ops-dashboard`).
#[derive(Component)]
pub(super) struct Commissioned;
/// Flag: this generator is online — gates the whole dashboard query
/// (`ops-dashboard`).
#[derive(Component)]
pub(super) struct Online;
/// Flag: this generator is streaming telemetry each tick (`ops-dashboard`).
#[derive(Component)]
pub(super) struct Reporting;

/// Water pressure in a hydro dam's penstock, stepped where it's open
/// (`full-flow` / `any-activity`).
#[derive(Component)]
pub(super) struct Penstock(pub(super) u32);
/// A dam's turbine RPM, stepped where it's spinning (`full-flow` /
/// `any-activity`).
#[derive(Component)]
pub(super) struct Turbine(pub(super) u32);
/// A dam's tailrace flow, stepped where it's draining (`full-flow` /
/// `any-activity`).
#[derive(Component)]
pub(super) struct Tailrace(pub(super) u32);
/// Flag: this dam's [`Penstock`] steps each tick.
#[derive(Component)]
pub(super) struct PenstockOpen;
/// Flag: this dam's [`Turbine`] steps each tick.
#[derive(Component)]
pub(super) struct TurbineSpinning;
/// Flag: this dam's [`Tailrace`] steps each tick.
#[derive(Component)]
pub(super) struct TailraceDraining;
