//! The [`Plugin`] trait — every engine subsystem implements it.

use crate::application::Application;

/// A self-contained engine subsystem.
///
/// [`build`](Self::build) runs exactly once, inside
/// [`Application::add_plugin`]. The plugin is a *registrar*: it uses
/// `&mut Application` to push startup closures
/// ([`Application::add_startup_system`]) or install the runner
/// ([`Application::set_runner`]). Don't do real work inside `build` —
/// push it into a startup closure so it fires in the controlled
/// startup phase.
///
/// # Examples
///
/// ```
/// use spark_core::{Application, Plugin};
///
/// struct BannerPlugin;
/// impl Plugin for BannerPlugin {
///     fn build(&self, app: &mut Application) {
///         app.add_startup_system(|| Ok(()));
///     }
/// }
///
/// Application::new().add_plugin(BannerPlugin).run().unwrap();
/// ```
pub trait Plugin {
    /// Registers this plugin's lifecycle hooks with `app`. Called once,
    /// synchronously, from inside [`Application::add_plugin`].
    fn build(&self, app: &mut Application);
}
