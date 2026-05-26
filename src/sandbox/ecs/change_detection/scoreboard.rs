//! The demo's self-check harness — shared by every scenario module.
//!
//! Change detection is frame-relative ("changed *since this system last
//! ran*"), so proving it works means asserting an exact match count **per
//! frame**. This module provides the machinery every scenario reports
//! through:
//!
//! - [`Frame`] — a 1-based per-frame counter, bumped in `PreUpdate` so an
//!   observer in `Update` can branch its expectation on "is this the first
//!   frame" (the frame where everything seeded before the sim started reads
//!   as freshly changed).
//! - [`Scoreboard`] — the current frame's [`Check`]s, one per observer.
//!   Cleared each frame, so it never grows.
//! - [`record`] — the single call an observer makes to register its
//!   `actual` vs `expected` count.
//! - [`report_scoreboard`] — logs the frame's results (`PASS` / `FAIL`),
//!   then goes quiet once everything has settled, speaking up again only on
//!   a regression.
//!
//! It carries no game meaning — it is the test rig the energy-sim scenarios
//! run on.

use spark_core::{Application, Stage};
use spark_ecs::{Res, ResMut, Resource};
use spark_log::{error, info};

/// How many leading frames log the full per-observer breakdown. By this
/// frame every scenario has reached its steady state, so
/// [`report_scoreboard`] falls silent — and only speaks again on a
/// divergence.
pub(super) const REPORT_FRAMES: u32 = 8;

/// 1-based frame counter for the change-detection demo. Bumped once per
/// frame in `PreUpdate` (see [`open_frame`]) so an `Update` observer can
/// tell its first run from later ones.
#[derive(Resource)]
pub(super) struct Frame(pub(super) u32);

/// One observer's outcome for the current frame: the matches it counted
/// (`actual`) versus the count its scenario predicts (`expected`).
pub(super) struct Check {
    /// Short game-flavoured label, e.g. `"grid-solver"` — the scenario the
    /// line belongs to.
    scenario: &'static str,
    /// The exact `Query<…>` the scenario exercises, for the log line.
    shape: &'static str,
    /// Matches counted this frame.
    actual: usize,
    /// Matches the scenario's per-frame rule predicts.
    expected: usize,
}

impl Check {
    /// Whether the observer counted exactly what its scenario predicted.
    const fn passed(&self) -> bool {
        self.actual == self.expected
    }
}

/// The current frame's checks, one per observer. [`open_frame`] clears it
/// at the top of each frame, observers fill it during `Update`, and
/// [`report_scoreboard`] reads it in `PostUpdate` — so it holds exactly one
/// frame's worth.
#[derive(Resource, Default)]
pub(super) struct Scoreboard(Vec<Check>);

/// Registers one observer's outcome for this frame — the single write point
/// into the [`Scoreboard`], so every scenario reports through one shape.
pub(super) fn record(
    board: &mut Scoreboard,
    scenario: &'static str,
    shape: &'static str,
    actual: usize,
    expected: usize,
) {
    board.0.push(Check {
        scenario,
        shape,
        actual,
        expected,
    });
}

/// Opens a frame: advances the counter and clears the previous frame's
/// checks. Runs in `PreUpdate`, before any sim system, so `Update` sees a
/// fresh, correctly-numbered frame.
pub(super) fn open_frame(mut frame: ResMut<Frame>, mut board: ResMut<Scoreboard>) {
    // `wrapping_add` so a session left running for ~2.3 years at 60 fps
    // can't panic on overflow; the only cost is that the first-frame-special
    // scenarios replay their transient once at the wrap — harmless for a
    // demo, the same class as the documented component-clock wraparound.
    frame.0 = frame.0.wrapping_add(1);
    board.0.clear();
}

/// Logs the frame's scoreboard. For the first [`REPORT_FRAMES`] frames it
/// prints every observer's `actual` vs `expected` plus an `N/M PASS` tally;
/// afterwards it is silent unless a count diverges, which it then reports at
/// `error!` so a regression is impossible to miss.
pub(super) fn report_scoreboard(frame: Res<Frame>, board: Res<Scoreboard>) {
    let total = board.0.len();
    let failed = board.0.iter().filter(|c| !c.passed()).count();

    if frame.0 <= REPORT_FRAMES {
        for c in &board.0 {
            info!(
                scenario = c.scenario,
                shape = c.shape,
                frame = frame.0,
                actual = c.actual,
                expected = c.expected,
                verdict = if c.passed() { "PASS" } else { "FAIL" },
                "sandbox/ecs/change-detection"
            );
        }
        info!(
            frame = frame.0,
            passed = total - failed,
            failed,
            total,
            "sandbox/ecs/change-detection: frame scoreboard"
        );
    } else if failed > 0 {
        for c in board.0.iter().filter(|c| !c.passed()) {
            error!(
                scenario = c.scenario,
                shape = c.shape,
                frame = frame.0,
                actual = c.actual,
                expected = c.expected,
                "sandbox/ecs/change-detection: REGRESSION — count diverged from expectation"
            );
        }
    }
}

/// Inserts the shared resources and registers the frame-open / report
/// systems. Called once by [`super::ChangeDetectionPlugin`] before the
/// scenario modules register their own systems.
pub(super) fn register(app: &mut Application) {
    app.add_resource(Frame(0));
    app.add_resource(Scoreboard::default());
    app.add_system(Stage::PreUpdate, open_frame);
    app.add_system(Stage::PostUpdate, report_scoreboard);
}
