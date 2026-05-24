//! Integration coverage for [`TimePlugin`]'s stage wiring: the systems it
//! registers advance the right counters when their stages run. The unit tests
//! in `src/time.rs` pin the pure `tick` math; this pins that the plugin routes
//! `advance_time` to `PreUpdate` and `advance_fixed_step` to `FixedUpdate`.

use spark_common::{Time, TimePlugin};
use spark_core::{Application, Stage};

/// `advance_time` on `PreUpdate` bumps the render-frame counter; each
/// `FixedUpdate` run bumps the sim-step counter via `advance_fixed_step`. This
/// is the exact contract the `spark-window` runner depends on.
#[test]
fn plugin_systems_advance_frame_and_fixed_step() {
    let mut app = Application::new();
    app.add_plugin(TimePlugin);

    // Two PreUpdate pumps == two render frames.
    app.run_stage(Stage::PreUpdate);
    app.run_stage(Stage::PreUpdate);
    assert_eq!(app.world().resource::<Time>().frame(), 2);

    // The window runner dispatches FixedUpdate N times per frame; each run is
    // one sim step. fixed_step advances independently of frame.
    app.run_stage(Stage::FixedUpdate);
    app.run_stage(Stage::FixedUpdate);
    app.run_stage(Stage::FixedUpdate);
    let time = app.world().resource::<Time>();
    assert_eq!(time.fixed_step(), 3);
    assert_eq!(time.frame(), 2); // unchanged by FixedUpdate
}
