//! The [`Application`] type — the engine's composition root. Knows
//! nothing about windowing or logging; every capability is supplied
//! by a [`Plugin`].

use spark_ecs::World;

use crate::error::EngineError;
use crate::plugin::Plugin;

type StartupSystem = Box<dyn FnOnce() -> Result<(), EngineError>>;
type Runner = Box<dyn FnOnce(Application) -> Result<(), EngineError>>;

/// Ordered list of plugins' registered work, plus the optional runner
/// that takes the main thread after startup.
///
/// Three phases per [`run`](Self::run): plugins are *built* (each one
/// pushes startup closures and/or installs a runner), startup closures
/// are *drained* in registration order with `?` short-circuiting on
/// the first error, then the runner — if any — receives an
/// [`Application`] and blocks. With no runner, `run` returns right
/// after startup, useful for headless plugin tests.
///
/// [`set_runner`](Self::set_runner) is last-write-wins; only one
/// plugin should install a runner in a normal program.
///
/// # Examples
///
/// ```
/// use spark_core::{Application, Plugin};
///
/// struct CounterPlugin;
/// impl Plugin for CounterPlugin {
///     fn build(&self, app: &mut Application) {
///         app.add_startup_system(|| Ok(()));
///     }
/// }
///
/// Application::new().add_plugin(CounterPlugin).run().unwrap();
/// ```
#[derive(Default)]
pub struct Application {
    world: World,
    startup: Vec<StartupSystem>,
    runner: Option<Runner>,
}

impl Application {
    /// Creates an empty `Application` — no plugins, no startup
    /// closures, no runner.
    ///
    /// # Examples
    ///
    /// ```
    /// spark_core::Application::new().run().unwrap();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a plugin by invoking its [`Plugin::build`] immediately.
    /// Returns `&mut Self` so plugins chain.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, Plugin};
    ///
    /// struct NoOp;
    /// impl Plugin for NoOp {
    ///     fn build(&self, _: &mut Application) {}
    /// }
    ///
    /// Application::new().add_plugin(NoOp).add_plugin(NoOp).run().unwrap();
    /// ```
    // Spec mandates `plugin: P` by value with `Plugin::build` taking
    // `&self`; the allow records the deliberate ergonomic trade.
    #[allow(clippy::needless_pass_by_value)]
    pub fn add_plugin<P: Plugin>(&mut self, plugin: P) -> &mut Self {
        plugin.build(self);
        self
    }

    /// Inserts a resource into the engine's [`World`]. Chainable; a
    /// second insert of the same type overwrites the previous value.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::Application;
    ///
    /// struct GameTime { dt: f32 }
    ///
    /// Application::new()
    ///     .add_resource(GameTime { dt: 0.016 })
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn add_resource<T: 'static>(&mut self, value: T) -> &mut Self {
        self.world.add_resource(value);
        self
    }

    /// Registers a closure to run during startup. Closures fire in
    /// registration order; the first `Err` short-circuits
    /// [`run`](Self::run).
    ///
    /// # Examples
    ///
    /// ```
    /// spark_core::Application::new()
    ///     .add_startup_system(|| Ok(()))
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn add_startup_system<F>(&mut self, f: F) -> &mut Self
    where
        F: FnOnce() -> Result<(), EngineError> + 'static,
    {
        self.startup.push(Box::new(f));
        self
    }

    /// Installs the runner — the closure that takes the main thread
    /// after startup. Last call wins; one plugin (today
    /// `WindowPlugin`) installs it. The `FnOnce(Application)`
    /// signature is stable across M4, where the runner will use the
    /// supplied `Application` to drive per-frame schedules.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_core::{Application, EngineError};
    ///
    /// Application::new()
    ///     .set_runner(|_app: Application| -> Result<(), EngineError> { Ok(()) })
    ///     .run()
    ///     .unwrap();
    /// ```
    pub fn set_runner<R>(&mut self, runner: R) -> &mut Self
    where
        R: FnOnce(Application) -> Result<(), EngineError> + 'static,
    {
        self.runner = Some(Box::new(runner));
        self
    }

    /// Runs the startup phase, then the runner phase.
    ///
    /// Drains every closure registered with
    /// [`add_startup_system`](Self::add_startup_system) in registration
    /// order, then — if a runner was installed — hands an [`Application`]
    /// to it (typically the winit event loop, which blocks until the
    /// window closes). With no runner, returns `Ok(())` right after
    /// startup. Takes `&mut self` so it can finish off a builder chain:
    /// `Application::new().add_plugin(_).run()`.
    ///
    /// # Errors
    ///
    /// Returns the first error any startup closure produced, or the
    /// runner's return value if startup succeeded. Closures convert
    /// their typed errors to [`EngineError`] via `?` before they reach
    /// this function.
    ///
    /// # Examples
    ///
    /// ```
    /// spark_core::Application::new().run().unwrap();
    /// ```
    pub fn run(&mut self) -> Result<(), EngineError> {
        let startup = std::mem::take(&mut self.startup);
        for system in startup {
            system()?;
        }

        if let Some(runner) = self.runner.take() {
            runner(std::mem::take(self))?;
        }

        Ok(())
    }
}
