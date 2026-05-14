//! Spark binary entry point.
//!
//! Installs the `tracing` subscriber, logs the engine version, and hands
//! off to [`spark_window::run`]. Becomes the textbook
//! `App::new().add_plugin(...).run()` shape once `spark-ecs` lands the
//! formal `App` / `Plugin` traits in M4.

fn main() -> Result<(), spark_window::WindowError> {
    spark_window::init_tracing();
    tracing::info!("Spark v{}", spark_core::VERSION);
    spark_window::run(
        spark_window::WindowConfig::default()
            .with_title("Spark")
            .with_size(1280, 720),
    )
}
