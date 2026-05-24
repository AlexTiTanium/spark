//! Top-level sandbox composer.
//!
//! [`SandboxPlugin`] is what `main.rs` adds. Its job is to:
//!
//!  1. Insert the resources every sub-sandbox shares
//!     (today: just [`TickCount`]).
//!  2. Register each sub-sandbox plugin in turn via
//!     `app.add_plugin(...)` — a nested-plugin pattern that lets
//!     each subsystem demo live in its own folder with its own
//!     local components, while reusing the shared ones from
//!     `crate::sandbox::components`.
//!
//! To add a new sub-sandbox (e.g. a future render demo): create
//! `src/sandbox/<name>/` mirroring `ecs/`'s layout, expose
//! `<Name>SandboxPlugin`, and add one `.add_plugin(<Name>SandboxPlugin)`
//! line in [`SandboxPlugin::build`] below.

use spark_core::{Application, Plugin};

use super::ecs::EcsSandboxPlugin;
use super::input::InputSandboxPlugin;
use super::resources::TickCount;

/// The umbrella sandbox plugin — adds shared resources, then
/// composes every sub-sandbox.
///
/// This is the plugin `main.rs` registers. Sub-sandbox plugins
/// (`EcsSandboxPlugin`, future `RenderSandboxPlugin`, …) are nested
/// inside this one's `build`, so a binary that wants the full
/// playground just adds `SandboxPlugin` and gets everything.
pub struct SandboxPlugin;

impl Plugin for SandboxPlugin {
    fn build(&self, app: &mut Application) {
        // ----- Shared resources -----
        //
        // Add these *before* any sub-sandbox plugin runs, so
        // sub-sandbox systems can rely on them via
        // `Res<T>` / `ResMut<T>` from their very first tick.
        app.add_resource(TickCount(0));

        // ----- Sub-sandboxes -----
        //
        // Each sub-sandbox owns its own entities, components, and
        // systems. They share resources / components defined at the
        // `crate::sandbox` level (above) and add their own locally.
        app.add_plugin(EcsSandboxPlugin);
        app.add_plugin(InputSandboxPlugin);
    }
}
