//! The [`Time`] resource and [`TimePlugin`] — Spark's single clock.
//!
//! # Logic
//!
//! One [`Time`] resource owns every clock the engine needs:
//!
//! - a **real** wall clock (`real_delta` / `real_elapsed`) — raw frame time,
//! - a **virtual** clock (`delta` / `elapsed`) — the real clock scaled by
//!   [`scale`](Time::scale) and frozen while [`paused`](Time::is_paused), which
//!   is what gameplay reads,
//! - the **fixed-timestep** accumulator and the per-frame step count that the
//!   window runner consumes to dispatch [`Stage::FixedUpdate`].
//!
//! Each frame the window runner ticks [`Stage::PreUpdate`], where
//! [`advance_time`] samples the wall clock and calls the pure [`Time::tick`].
//! `tick` clamps the delta, advances both clocks, and banks the fixed
//! accumulator, leaving [`fixed_steps_this_frame`](Time::fixed_steps_this_frame)
//! for the runner to read. The runner then calls [`Stage::FixedUpdate`] that
//! many times; [`advance_fixed_step`] bumps [`fixed_step`](Time::fixed_step)
//! once per sim step.
//!
//! # Why it works
//!
//! Folding the accumulator into `Time` makes it the *single* clock: the wall
//! clock is sampled once per frame (one `Instant::now()`), the 60 Hz constants
//! live in exactly one place, and `spark-window` carries no clock state — it
//! reads `fixed_steps_this_frame()` and drives the stage. Storing every
//! duration as a [`Duration`] (integer nanos) keeps the real clock's
//! accumulation and the fixed accumulator exact — no floating-point drift to
//! correct; the scaled virtual clock carries the small rounding inherent in
//! `mul_f32`. Seconds are produced only on read.
//!
//! # How NOT to use
//!
//! - In [`Stage::FixedUpdate`], read [`fixed_delta_secs`](Time::fixed_delta_secs)
//!   — never [`delta_secs`](Time::delta_secs). The variable delta is wall-clock
//!   and would make the simulation frame-rate-dependent, breaking determinism.
//! - Register [`TimePlugin`] *before* any plugin whose runner reads `Time`
//!   (today, `WindowPlugin`) — the resource must exist before the first tick.

use std::time::{Duration, Instant};

use spark_core::{Application, Plugin, Stage};
use spark_ecs::{ResMut, Resource};

/// Length of one fixed-timestep step: 1/60 s ≈ 16.667 ms. The single
/// definition of the simulation rate — `spark-window` reads it via `Time`.
///
/// `1_000_000_000 / 60` truncates to `16_666_666` ns — 0.666 ns short per step,
/// so ~40 ns/s is banked back into the accumulator. Negligible for any real
/// session (≈3.5 ms after a full day), and the carry keeps the rate honest.
const FIXED_DELTA: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// Upper bound on the real time a single frame may contribute. Without it, a
/// long stall (a breakpoint, a dragged title bar) would bank seconds and fire
/// a catch-up burst of `FixedUpdate` steps — the "spiral of death". Clamping a
/// frame to 250 ms caps catch-up at ~15 steps and lets the sim fall behind
/// gracefully instead.
const MAX_DELTA: Duration = Duration::from_millis(250);

/// Upper bound on [`Time::scale`]. Keeps `real.mul_f32(scale)` in [`Time::tick`]
/// from overflowing `Duration` — which would *panic* — on a wild input. Even at
/// the 250 ms frame clamp, 1e6× is ~69 hours of virtual time banked in a single
/// frame: astronomically beyond any speed control, yet far below `Duration`'s
/// ~584-year ceiling. `NaN`/negative inputs clamp to `0` (see [`Time::set_scale`]).
const MAX_SCALE: f32 = 1_000_000.0;

/// The engine's single clock: real + virtual wall clocks plus the
/// fixed-timestep accumulator and counters.
///
/// Inserted by [`TimePlugin`] and advanced once per frame by [`advance_time`]
/// in [`Stage::PreUpdate`]. Gameplay reads the *virtual* clock
/// ([`delta`](Self::delta) / [`elapsed`](Self::elapsed)) — scaled by
/// [`scale`](Self::scale), frozen while paused — so pause and speed controls
/// work without every system checking a flag.
///
/// All durations are stored as [`Duration`] for exact, drift-free
/// accumulation; the `*_secs` accessors convert on read.
///
/// # Examples
///
/// ```
/// use spark_common::Time;
///
/// let mut time = Time::default();
/// assert_eq!(time.scale(), 1.0);     // realtime by default
/// assert!(!time.is_paused());
/// time.set_scale(2.0);               // 2× speed
/// time.pause();                      // virtual clock frozen
/// assert!(time.is_paused());
/// ```
#[derive(Resource, Debug)]
pub struct Time {
    /// Virtual (scaled, pausable) delta for this frame.
    delta: Duration,
    /// Virtual time accumulated since startup.
    elapsed: Duration,
    /// Real (unscaled, clamped) delta for this frame.
    real_delta: Duration,
    /// Real time accumulated since startup.
    real_elapsed: Duration,
    /// Constant fixed-timestep length ([`FIXED_DELTA`]).
    fixed_delta: Duration,
    /// Real time banked but not yet spent in whole [`fixed_delta`](Self::fixed_delta) steps.
    fixed_accumulator: Duration,
    /// Whole fixed steps the last [`tick`](Self::tick) banked — read by the runner.
    fixed_steps_this_frame: u32,
    /// Render frames elapsed (one per [`advance_time`]).
    frame: u64,
    /// Simulation steps elapsed (one per [`advance_fixed_step`]).
    fixed_step: u64,
    /// Virtual-clock speed: `1.0` realtime, `2.0` double, clamped `>= 0`.
    scale: f32,
    /// When `true`, the virtual clock is frozen (effective scale `0`).
    paused: bool,
    /// Instant of the previous frame; `None` until the first tick.
    last_instant: Option<Instant>,
}

impl Default for Time {
    /// Realtime, unpaused, fixed step pre-set to [`FIXED_DELTA`]; every other
    /// field zero. Hand-written because a derived `Default` would zero `scale`
    /// (a paused-equivalent start) and `fixed_delta` (a 0 s, broken sim step).
    fn default() -> Self {
        Self {
            delta: Duration::ZERO,
            elapsed: Duration::ZERO,
            real_delta: Duration::ZERO,
            real_elapsed: Duration::ZERO,
            fixed_delta: FIXED_DELTA,
            fixed_accumulator: Duration::ZERO,
            fixed_steps_this_frame: 0,
            frame: 0,
            fixed_step: 0,
            scale: 1.0,
            paused: false,
            last_instant: None,
        }
    }
}

impl Time {
    /// Advances every clock by one frame from a measured raw wall-clock delta.
    ///
    /// The pure core of the clock — no `Instant`, no globals — so it is unit-
    /// tested directly with synthetic durations. It clamps `raw` to
    /// [`MAX_DELTA`], advances the real clock, derives the virtual clock from
    /// the effective scale (`0` while paused), banks the clamped delta into the fixed
    /// accumulator and drains it in whole [`fixed_delta`](Self::fixed_delta)
    /// chunks (carrying the remainder), and bumps the render-frame counter.
    ///
    /// Fixed-step accounting uses the **real** (clamped) delta, not the scaled
    /// one: `scale`/`paused` affect the gameplay clock only, so the simulation
    /// keeps ticking at a constant 60 Hz. (M7+ may feed the scaled delta here
    /// to make speed affect the sim rate deterministically.)
    fn tick(&mut self, raw: Duration) {
        let real = raw.min(MAX_DELTA);
        self.real_delta = real;
        self.real_elapsed += real;

        let eff = if self.paused { 0.0 } else { self.scale };
        let scaled = real.mul_f32(eff);
        self.delta = scaled;
        self.elapsed += scaled;

        self.frame += 1;

        self.fixed_accumulator += real;
        let mut steps = 0;
        while self.fixed_accumulator >= self.fixed_delta {
            self.fixed_accumulator -= self.fixed_delta;
            steps += 1;
        }
        self.fixed_steps_this_frame = steps;
    }

    /// This frame's virtual (scaled, pausable) delta.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().delta(), std::time::Duration::ZERO);
    /// ```
    #[must_use]
    pub fn delta(&self) -> Duration {
        self.delta
    }

    /// This frame's virtual delta in seconds — the value gameplay multiplies in
    /// (`pos += vel * time.delta_secs()`); honors pause and speed.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().delta_secs(), 0.0);
    /// ```
    #[must_use]
    pub fn delta_secs(&self) -> f32 {
        self.delta.as_secs_f32()
    }

    /// Virtual time accumulated since startup.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().elapsed(), std::time::Duration::ZERO);
    /// ```
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Virtual elapsed time in seconds (`f32`). For sessions over a few hours,
    /// prefer [`elapsed_secs_f64`](Self::elapsed_secs_f64) — `f32` loses
    /// sub-millisecond precision as elapsed grows.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().elapsed_secs(), 0.0);
    /// ```
    #[must_use]
    pub fn elapsed_secs(&self) -> f32 {
        self.elapsed.as_secs_f32()
    }

    /// Virtual elapsed time in seconds (`f64`) — use over `f32` for the in-game
    /// clock, which `f32` loses precision on after a few hours of runtime.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().elapsed_secs_f64(), 0.0);
    /// ```
    #[must_use]
    pub fn elapsed_secs_f64(&self) -> f64 {
        self.elapsed.as_secs_f64()
    }

    /// This frame's real (unscaled, clamped) delta — ignores pause and speed.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().real_delta(), std::time::Duration::ZERO);
    /// ```
    #[must_use]
    pub fn real_delta(&self) -> Duration {
        self.real_delta
    }

    /// This frame's real delta in seconds — for profilers and UI animation that
    /// must keep moving while the game is paused.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().real_delta_secs(), 0.0);
    /// ```
    #[must_use]
    pub fn real_delta_secs(&self) -> f32 {
        self.real_delta.as_secs_f32()
    }

    /// Real time accumulated since startup.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().real_elapsed(), std::time::Duration::ZERO);
    /// ```
    #[must_use]
    pub fn real_elapsed(&self) -> Duration {
        self.real_elapsed
    }

    /// Real elapsed time in seconds (`f32`). Like [`elapsed_secs`](Self::elapsed_secs),
    /// prefer the `f64` variant for long sessions.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().real_elapsed_secs(), 0.0);
    /// ```
    #[must_use]
    pub fn real_elapsed_secs(&self) -> f32 {
        self.real_elapsed.as_secs_f32()
    }

    /// Real elapsed time in seconds (`f64` — long sessions need the precision).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().real_elapsed_secs_f64(), 0.0);
    /// ```
    #[must_use]
    pub fn real_elapsed_secs_f64(&self) -> f64 {
        self.real_elapsed.as_secs_f64()
    }

    /// The constant fixed-timestep length (1/60 s).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().fixed_delta(), std::time::Duration::from_nanos(16_666_666));
    /// ```
    #[must_use]
    pub fn fixed_delta(&self) -> Duration {
        self.fixed_delta
    }

    /// The fixed-timestep length in seconds — the delta sim systems use in
    /// [`Stage::FixedUpdate`] (constant, so the sim stays deterministic).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// let dt = Time::default().fixed_delta_secs();
    /// assert!((dt - 1.0 / 60.0).abs() < 1e-6);
    /// ```
    #[must_use]
    pub fn fixed_delta_secs(&self) -> f32 {
        self.fixed_delta.as_secs_f32()
    }

    /// The fixed-timestep length in seconds (`f64`). Unlike the variable deltas,
    /// the fixed step is what a deterministic sim accumulates over many steps, so
    /// the `f64` form is offered to keep that accumulation precise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// let dt = Time::default().fixed_delta_secs_f64();
    /// assert!((dt - 1.0 / 60.0).abs() < 1e-9);
    /// ```
    #[must_use]
    pub fn fixed_delta_secs_f64(&self) -> f64 {
        self.fixed_delta.as_secs_f64()
    }

    /// Real time banked but not yet spent in whole fixed steps (debug/inspection).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().fixed_accumulator(), std::time::Duration::ZERO);
    /// ```
    #[must_use]
    pub fn fixed_accumulator(&self) -> Duration {
        self.fixed_accumulator
    }

    /// Whole fixed steps this frame banked — the count the window runner reads
    /// to dispatch [`Stage::FixedUpdate`]. A peek, not a consume.
    ///
    /// Set once per frame by [`tick`](Self::tick); it does **not** count down as
    /// the runner dispatches steps. Read from `Update`/`PostUpdate` it means "how
    /// many sim steps ran this frame", not "steps remaining" inside a burst.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().fixed_steps_this_frame(), 0);
    /// ```
    #[must_use]
    pub fn fixed_steps_this_frame(&self) -> u32 {
        self.fixed_steps_this_frame
    }

    /// Render frames elapsed since startup (one per [`Stage::PreUpdate`]). Index
    /// UI and animation by this; index save/replay by [`fixed_step`](Self::fixed_step).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().frame(), 0);
    /// ```
    #[must_use]
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Simulation steps elapsed since startup (one per [`Stage::FixedUpdate`]).
    /// Deterministic — index save/replay by this.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().fixed_step(), 0);
    /// ```
    #[must_use]
    pub fn fixed_step(&self) -> u64 {
        self.fixed_step
    }

    /// The virtual-clock speed multiplier (`1.0` realtime).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert_eq!(Time::default().scale(), 1.0);
    /// ```
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// Sets the virtual-clock speed multiplier, clamped to `[0, MAX_SCALE]`.
    /// Negative inputs and `NaN` become `0`; absurdly large inputs cap at
    /// `MAX_SCALE` (1e6) so [`tick`](Self::tick)'s `mul_f32` can never overflow
    /// `Duration` and panic. Prefer [`pause`](Self::pause) over `set_scale(0.0)`
    /// — it is reversible and preserves the chosen speed.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// let mut time = Time::default();
    /// time.set_scale(2.0);
    /// assert_eq!(time.scale(), 2.0);
    /// time.set_scale(-1.0);          // clamped up to 0
    /// assert_eq!(time.scale(), 0.0);
    /// ```
    pub fn set_scale(&mut self, scale: f32) {
        // Fold NaN to 0 first — `f32::clamp` would *propagate* it, and a NaN
        // scale makes `tick`'s `mul_f32` panic. Then bound to `[0, MAX_SCALE]`
        // so `+inf` and overflow-inducing values can't reach the multiply.
        let scale = if scale.is_nan() { 0.0 } else { scale };
        self.scale = scale.clamp(0.0, MAX_SCALE);
    }

    /// Whether the virtual clock is frozen.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// assert!(!Time::default().is_paused());
    /// ```
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Freezes the virtual clock (real clock keeps advancing).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// let mut time = Time::default();
    /// time.pause();
    /// assert!(time.is_paused());
    /// ```
    pub fn pause(&mut self) {
        self.paused = true;
    }

    /// Resumes the virtual clock at the current [`scale`](Self::scale).
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// let mut time = Time::default();
    /// time.pause();
    /// time.unpause();
    /// assert!(!time.is_paused());
    /// ```
    pub fn unpause(&mut self) {
        self.paused = false;
    }

    /// Sets the paused state directly — convenient for a bound toggle.
    ///
    /// # Examples
    ///
    /// ```
    /// # use spark_common::Time;
    /// let mut time = Time::default();
    /// time.set_paused(true);
    /// assert!(time.is_paused());
    /// ```
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }
}

/// Samples the wall clock and advances [`Time`] for this frame, leaving
/// [`fixed_steps_this_frame`](Time::fixed_steps_this_frame) for the runner. Runs
/// in [`Stage::PreUpdate`]; its position among other `PreUpdate` systems follows
/// registration order, so [`TimePlugin`] should be registered first (see there).
///
/// Splitting the clock read from [`Time::tick`] keeps the math pure and testable;
/// the first-frame delta is `0` because `last_instant` starts `None`.
#[allow(
    clippy::needless_pass_by_value,
    reason = "ResMut<Time> is a SystemParam — IntoSystem hands it in by value; \
              &mut ResMut<Time> is not itself a SystemParam."
)]
fn advance_time(mut time: ResMut<Time>) {
    let now = Instant::now();
    let raw = time
        .last_instant
        .map_or(Duration::ZERO, |prev| now.saturating_duration_since(prev));
    time.last_instant = Some(now);
    time.tick(raw);
}

/// First system in [`Stage::FixedUpdate`]: one invocation is one simulation
/// step, so it bumps [`Time::fixed_step`]. The window runner calls the stage
/// [`fixed_steps_this_frame`](Time::fixed_steps_this_frame) times per frame.
#[allow(
    clippy::needless_pass_by_value,
    reason = "ResMut<Time> is a SystemParam — IntoSystem hands it in by value; \
              &mut ResMut<Time> is not itself a SystemParam."
)]
fn advance_fixed_step(mut time: ResMut<Time>) {
    time.fixed_step += 1;
}

/// Registers the [`Time`] resource and its two advance systems.
///
/// Inserts [`Time::default`], puts [`advance_time`] first in
/// [`Stage::PreUpdate`] and [`advance_fixed_step`] first in
/// [`Stage::FixedUpdate`]. "First" follows from registration order, so register
/// `TimePlugin` before plugins whose systems depend on a fresh clock.
///
/// # Registration order
///
/// Register `TimePlugin` **before** [`WindowPlugin`](https://docs.rs/spark-window).
/// The window runner reads [`Time::fixed_steps_this_frame`] every frame; if
/// `Time` is absent when the runner first dispatches [`Stage::PreUpdate`], it
/// panics on the missing resource.
///
/// # Examples
///
/// ```
/// use spark_common::{Time, TimePlugin};
/// use spark_core::{Application, Stage};
///
/// let mut app = Application::new();
/// app.add_plugin(TimePlugin);
/// // One PreUpdate pump advances the clock by a (here zero) frame.
/// app.run_stage(Stage::PreUpdate);
/// assert_eq!(app.world().resource::<Time>().frame(), 1);
/// ```
#[derive(Debug, Default)]
pub struct TimePlugin;

impl Plugin for TimePlugin {
    fn build(&self, app: &mut Application) {
        app.add_resource(Time::default());
        app.add_system(Stage::PreUpdate, advance_time);
        app.add_system(Stage::FixedUpdate, advance_fixed_step);
    }
}

#[cfg(test)]
mod tests {
    use super::{FIXED_DELTA, MAX_DELTA, Time};
    use std::time::Duration;

    /// 1/60 s as a `Duration`, the natural step-boundary input.
    const STEP: Duration = FIXED_DELTA;

    #[test]
    fn single_tick_scale_one_advances_both_clocks() {
        let mut t = Time::default();
        t.tick(STEP);
        assert_eq!(t.delta(), STEP);
        assert_eq!(t.elapsed(), STEP);
        assert_eq!(t.real_delta(), STEP);
        assert_eq!(t.real_elapsed(), STEP);
        assert_eq!(t.frame(), 1);
    }

    #[test]
    fn scale_two_doubles_virtual_not_real() {
        let mut t = Time::default();
        t.set_scale(2.0);
        t.tick(STEP);
        // Virtual delta is real × 2 (within f32 scaling precision); real is untouched.
        assert!((t.delta_secs() - 2.0 * t.real_delta_secs()).abs() < 1e-6);
        assert_eq!(t.real_delta(), STEP);
    }

    #[test]
    fn paused_freezes_virtual_clock_only() {
        let mut t = Time::default();
        t.pause();
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
        assert_eq!(t.real_delta(), STEP);
        assert_eq!(t.real_elapsed(), STEP);
    }

    #[test]
    fn unpause_restores_scaled_advance() {
        let mut t = Time::default();
        t.pause();
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
        t.unpause();
        t.tick(STEP);
        assert_eq!(t.delta(), STEP);
    }

    #[test]
    fn negative_scale_clamps_to_zero() {
        let mut t = Time::default();
        t.set_scale(-1.0);
        assert!(t.scale().abs() < 1e-6); // clamped to 0
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
    }

    #[test]
    fn pathological_frame_is_clamped_to_max_delta() {
        let mut t = Time::default();
        t.tick(Duration::from_secs(1));
        assert_eq!(t.real_delta(), MAX_DELTA);
        assert_eq!(t.fixed_steps_this_frame(), 15);
    }

    #[test]
    fn first_frame_zero_delta_still_counts() {
        // What `advance_time` produces on the first frame: last_instant is None,
        // so raw is ZERO.
        let mut t = Time::default();
        t.tick(Duration::ZERO);
        assert_eq!(t.delta(), Duration::ZERO);
        assert_eq!(t.real_delta(), Duration::ZERO);
        assert_eq!(t.frame(), 1);
    }

    #[test]
    fn exact_step_drains_one_and_leaves_nothing() {
        let mut t = Time::default();
        t.tick(STEP);
        assert_eq!(t.fixed_steps_this_frame(), 1);
        assert_eq!(t.fixed_accumulator(), Duration::ZERO);
    }

    #[test]
    fn fast_frame_banks_without_stepping() {
        let mut t = Time::default();
        let dt = Duration::from_millis(5);
        t.tick(dt);
        assert_eq!(t.fixed_steps_this_frame(), 0);
        assert_eq!(t.fixed_accumulator(), dt);
    }

    #[test]
    fn four_fast_frames_step_once_on_the_fourth() {
        let mut t = Time::default();
        let dt = Duration::from_millis(5);
        let counts: Vec<u32> = (0..4)
            .map(|_| {
                t.tick(dt);
                t.fixed_steps_this_frame()
            })
            .collect();
        assert_eq!(counts, [0, 0, 0, 1]);
        // 20 ms banked − one 1/60 s step spent == the carried remainder.
        assert_eq!(t.fixed_accumulator() + STEP, Duration::from_millis(20));
    }

    #[test]
    fn slow_frames_catch_up() {
        let mut t = Time::default();
        let dt = Duration::from_millis(33);
        t.tick(dt);
        let first = t.fixed_steps_this_frame();
        t.tick(dt);
        let second = t.fixed_steps_this_frame();
        assert_eq!([first, second], [1, 2]); // 33ms→1 (rem ~16.3ms), +33ms→2
        assert!(t.fixed_accumulator() < STEP);
    }

    #[test]
    fn zero_delta_banks_and_steps_nothing() {
        let mut t = Time::default();
        t.tick(Duration::ZERO);
        assert_eq!(t.fixed_steps_this_frame(), 0);
        assert_eq!(t.fixed_accumulator(), Duration::ZERO);
    }

    #[test]
    fn inclusive_boundary_steps_now() {
        // Accumulator one nanosecond shy of a step; a 1 ns tick reaches the
        // inclusive `>=` boundary and spends it this frame.
        let mut t = Time::default();
        t.tick(Duration::from_nanos(16_666_665)); // FIXED_DELTA − 1 ns
        assert_eq!(t.fixed_steps_this_frame(), 0);
        t.tick(Duration::from_nanos(1));
        assert_eq!(t.fixed_steps_this_frame(), 1);
    }

    #[test]
    fn frame_counts_in_tick_fixed_step_does_not() {
        // `tick` only moves `frame`; `fixed_step` is bumped by the
        // `advance_fixed_step` system, never by `tick`.
        let mut t = Time::default();
        for _ in 0..5 {
            t.tick(STEP);
        }
        assert_eq!(t.frame(), 5);
        assert_eq!(t.fixed_step(), 0);
    }

    #[test]
    fn paused_still_drains_fixed_accumulator() {
        // M1 contract: pausing freezes the gameplay clock but the sim keeps
        // ticking — real time still banks fixed steps. M7+ may revisit.
        let mut t = Time::default();
        t.pause();
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
        assert_eq!(t.fixed_steps_this_frame(), 1);
    }

    #[test]
    fn clamp_and_scale_compose() {
        // The safety property MAX_DELTA promises: a 1 s frame is clamped to
        // 250 ms in the *real* domain, then scaled — so a 2× clock advances
        // 500 ms of virtual time, never 2 s. (This is also what keeps the
        // post-clamp `mul_f32` input bounded.)
        let mut t = Time::default();
        t.set_scale(2.0);
        t.tick(Duration::from_secs(1));
        assert_eq!(t.real_delta(), MAX_DELTA);
        let expected = MAX_DELTA.mul_f32(2.0);
        assert!((t.delta_secs() - expected.as_secs_f32()).abs() < 1e-6);
    }

    #[test]
    fn set_paused_round_trips() {
        // `set_paused(false)` is the half `pause()`/`unpause()` don't cover.
        let mut t = Time::default();
        t.set_paused(true);
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
        t.set_paused(false);
        t.tick(STEP);
        assert_eq!(t.delta(), STEP); // virtual clock resumes
        assert_eq!(t.elapsed(), STEP); // only the unpaused tick accumulated
    }

    #[test]
    fn elapsed_accumulates_across_ticks() {
        // Guards against an `elapsed`/`real_elapsed` assignment-instead-of-add bug
        // that a single-tick test would miss.
        let mut t = Time::default();
        t.set_scale(2.0);
        for _ in 0..4 {
            t.tick(STEP);
        }
        assert_eq!(t.real_elapsed(), STEP * 4); // real is exact
        assert_eq!(t.frame(), 4);
        // Virtual elapsed is 2× real (within f32 scaling precision).
        assert!((t.elapsed_secs_f64() - 2.0 * t.real_elapsed_secs_f64()).abs() < 1e-4);
    }

    #[test]
    fn scale_persists_across_pause_unpause() {
        // A bug where `unpause` reset scale to 1.0 would pass every other test.
        let mut t = Time::default();
        t.set_scale(3.0);
        t.pause();
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
        t.unpause();
        t.tick(STEP);
        assert!((t.delta_secs() - 3.0 * STEP.as_secs_f32()).abs() < 1e-6);
        assert!((t.scale() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn extreme_scale_clamps_and_does_not_panic() {
        // `mul_f32` panics on overflow/non-finite; `set_scale` must cap so a wild
        // input can't bring the engine down on the next tick.
        let mut t = Time::default();
        t.set_scale(f32::INFINITY);
        assert!((t.scale() - super::MAX_SCALE).abs() < 1.0);
        t.tick(STEP); // would panic without the cap
        assert_eq!(t.real_delta(), STEP); // real clock untouched

        t.set_scale(f32::NAN);
        assert!(t.scale().abs() < 1e-6); // NaN → 0
        t.tick(STEP);
        assert_eq!(t.delta(), Duration::ZERO);
    }
}
