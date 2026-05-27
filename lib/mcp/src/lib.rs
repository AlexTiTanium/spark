#![cfg(debug_assertions)]
#![doc = include_str!("../README.md")]

mod bus;
mod control;
mod http;
mod methods;
mod types;

use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;

use spark_core::Stage;
use spark_core::{Application, EngineError, Plugin};
use spark_ecs::WorkloadLabel;

pub use bus::Bus;
pub use control::McpControl;
pub use http::TOOL_CALL_TIMEOUT;
pub use methods::{INBOX_BUDGET, Inbox, tool_descriptions};
pub use types::{McpError, Reply, rpc_err, rpc_ok};

/// Workload labels for the plugin's three frame slots.
///
/// `Inbox` runs in [`Stage::First`] and drains pending `tools/call`
/// requests. `Control` runs in [`Stage::PreUpdate`] and applies
/// agent-driven pause/scale (no-op in P1). `Outbox` runs in
/// [`Stage::Last`] and publishes ECS events to the future `/events`
/// SSE stream (no-op in P1).
#[derive(WorkloadLabel)]
pub enum McpWorkload {
    /// Tool-call drain on [`Stage::First`].
    Inbox,
    /// Pause/scale apply on [`Stage::PreUpdate`].
    Control,
    /// Event publish on [`Stage::Last`].
    Outbox,
}

/// The MCP plugin. Spawns a `tiny_http` acceptor on
/// [`addr`](Self::addr), registers the three workloads, and inserts
/// the [`Inbox`] + [`McpControl`] resources.
///
/// # Examples
///
/// ```
/// use spark_core::Application;
/// use spark_mcp::SparkMcpPlugin;
///
/// // Use a free OS-chosen port so doctests on a developer's machine
/// // never collide with a running game.
/// let addr = "127.0.0.1:0".parse().unwrap();
/// let plugin = SparkMcpPlugin { addr };
/// assert_eq!(plugin.addr, addr);
/// ```
pub struct SparkMcpPlugin {
    /// Local socket the HTTP thread binds. Loopback is the security
    /// model — see the README's *Errors / pitfalls*.
    pub addr: SocketAddr,
}

impl Default for SparkMcpPlugin {
    /// Binds `127.0.0.1:9123` — the design's reserved port for Spark
    /// MCP. The literal is hard-coded; the `expect` is on an inline
    /// constant that cannot fail.
    fn default() -> Self {
        Self {
            addr: "127.0.0.1:9123"
                .parse()
                .expect("hard-coded loopback literal must parse"),
        }
    }
}

impl Plugin for SparkMcpPlugin {
    /// Registers the plugin with `app`.
    ///
    /// Inserts the [`Inbox`] / [`McpControl`] resources, attaches the
    /// three workloads, and pushes a **startup closure** that spawns
    /// the `spark-mcp-http` thread. Spawning at startup (not at
    /// registration time) keeps `Plugin::build` a pure registrar per
    /// the project convention, and means the OS thread does not exist
    /// until `Application::run` is called — useful for tests that
    /// build an `Application` without running it.
    ///
    /// # Errors
    ///
    /// The startup closure surfaces an [`EngineError`] if the OS
    /// refuses to spawn the HTTP thread. A bind failure is *not*
    /// surfaced here — it is logged from inside the thread, because
    /// the bind happens after the OS thread is already alive.
    fn build(&self, app: &mut Application) {
        let (req_tx, req_rx) = mpsc::channel::<methods::Request>();

        app.add_resource(methods::Inbox::new(req_rx))
            .add_resource(control::McpControl::default());

        app.add_workload(McpWorkload::Inbox, Stage::First, |w| {
            w.add_system(methods::drain_inbox);
        });
        app.add_workload(McpWorkload::Control, Stage::PreUpdate, |w| {
            w.add_system(control::apply_control);
        });
        app.add_workload(McpWorkload::Outbox, Stage::Last, |w| {
            w.add_system(methods::publish_events);
        });

        let addr = self.addr;
        app.add_startup_system(move || -> Result<(), EngineError> {
            thread::Builder::new()
                .name("spark-mcp-http".into())
                .spawn(move || {
                    if let Err(e) = http::serve(addr, req_tx) {
                        tracing::warn!(error = %e, "spark-mcp server stopped");
                    }
                })?;
            tracing::info!(addr = %addr, "spark-mcp ready");
            Ok(())
        });
    }
}
