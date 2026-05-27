//! Tool dispatch — turns a `(name, args)` pair from the inbox into a
//! handler call.
//!
//! In P1 the dispatcher always returns [`McpError::MethodNotFound`].
//! The point is to exercise the full HTTP → mpsc → main-thread → reply
//! round-trip on a real frame schedule before any tools land in P2.
//! [`tool_descriptions`] is the matching empty array for `tools/list`.

use std::sync::mpsc::{Receiver, Sender};

use serde_json::{Value, json};
use spark_common::Time;
use spark_ecs::{Res, Resource};

use crate::types::{McpError, Reply};

/// Hard cap on the number of `tools/call` requests `drain_inbox`
/// processes per frame. Keeps a burst of agent traffic from starving
/// the renderer — excess requests sit in the channel and get serviced
/// next frame.
pub const INBOX_BUDGET: usize = 256;

/// Resource that hands the HTTP thread's request stream to the
/// `drain_inbox` system.
///
/// One `Receiver` per plugin instance, created in `SparkMcpPlugin::build`
/// and passed in here. `Receiver` is `Send + !Sync`, but the world stores
/// resources in `RefCell`s and accesses them single-threaded, so the
/// missing `Sync` is fine.
#[derive(Resource)]
pub struct Inbox {
    rx: Receiver<Request>,
}

impl Inbox {
    /// Wraps an mpsc receiver in the resource shape.
    #[must_use]
    pub fn new(rx: Receiver<Request>) -> Self {
        Self { rx }
    }
}

/// Wire type carried on the HTTP → main-thread mpsc.
///
/// Only one variant today (`ToolCall`) — kept as an enum because P3
/// will add `Shutdown` (graceful drain on engine close) and P5 will add
/// `EventEmit` (synchronous event injection from `spark.event.emit`).
pub enum Request {
    /// MCP `tools/call`. `name` is the tool name, `args` is the JSON
    /// `arguments` object, `reply` is the per-request reply channel the
    /// HTTP thread blocks on.
    ToolCall {
        /// Tool name as named in the dispatch match — e.g. `spark.world.query`.
        name: String,
        /// `arguments` JSON from the `tools/call` request.
        args: Value,
        /// One-shot reply channel back to the HTTP thread.
        reply: Sender<Value>,
    },
}

/// `Stage::First` system. Drains pending [`Request`]s and sends each
/// reply back through its per-request channel.
///
/// Drains at most [`INBOX_BUDGET`] requests; the rest wait for the next
/// frame. Each reply is wrapped in the MCP tool-result envelope by
/// [`wrap`] — error results carry the JSON-RPC code as `_rpc_code` so
/// clients that branch on it stay deterministic.
///
/// When the dispatcher needs `&mut World` (P2 onward, once ECS supports
/// `&mut World` system params), the signature here changes to accept
/// the world and the dispatcher threads it through. For P1 the dispatch
/// only needs `Time` to stamp `Reply`s.
#[allow(
    clippy::needless_pass_by_value,
    reason = "Res<T> / ResMut<T> are SystemParam values — IntoSystem hands them in by value."
)]
pub fn drain_inbox(inbox: Res<Inbox>, time: Res<Time>) {
    let mut budget = INBOX_BUDGET;
    while budget > 0 {
        let Ok(req) = inbox.rx.try_recv() else { return };
        budget -= 1;
        let Request::ToolCall { name, args, reply } = req;
        let _ = reply.send(wrap(&time, dispatch(&name, args)));
    }
}

/// `Stage::Last` no-op stub.
///
/// P5 replaces this body with the publisher that drains the engine's
/// `EventRegistry` into the `Bus<EventLine>` that backs the future
/// `GET /events` SSE.
pub fn publish_events() {}

/// Dispatch a tool call to its handler.
///
/// P1: every tool name returns [`McpError::MethodNotFound`]. P2 adds
/// match arms for the `spark.world.*` / `spark.resource.*` / `spark.schema`
/// tools as `ComponentRegistry` lands; P3 adds the control / log /
/// frame-stat tools; P4 adds `spark.screenshot`. The dispatch *shape*
/// — `(name, args) → Result<Value, McpError>` — does not change.
fn dispatch(name: &str, _args: Value) -> Result<Value, McpError> {
    Err(McpError::method_not_found(name))
}

/// Wrap a handler result in the MCP tool-result envelope.
///
/// Success: `{ content: [{ type: "text", text: "<pretty json>" }], isError: false }`.
/// Failure: `{ content: [...], isError: true, _rpc_code: <jsonrpc code> }`.
///
/// The `_rpc_code` field is a Spark extension — MCP's spec only carries
/// `isError`, but agents need the standard JSON-RPC code to branch on
/// (e.g. retry on −32001 timeout, give up on −32601). The leading
/// underscore signals "extension field" to readers without changing
/// envelope shape for spec-strict clients.
fn wrap(time: &Time, r: Result<Value, McpError>) -> Value {
    match r {
        Ok(v) => {
            let reply = Reply::new(time, v);
            // `to_string_pretty` on a value we just built can only fail if a
            // custom `Serialize` impl panics — Reply<Value> has neither.
            let text = serde_json::to_string_pretty(&reply)
                .unwrap_or_else(|_| String::from("<serialization failed>"));
            json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false,
            })
        }
        Err(e) => json!({
            "content": [{ "type": "text", "text": e.to_string() }],
            "isError": true,
            "_rpc_code": e.rpc_code(),
        }),
    }
}

/// Tool descriptions returned from `tools/list`.
///
/// Empty in P1 — the dispatch table is empty. P2–P4 populate this with
/// one entry per shipped tool, each with `name`, `description`, and
/// `inputSchema` (JSON Schema).
#[must_use]
pub fn tool_descriptions() -> Value {
    json!([])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P1's defining behaviour: every dispatch returns method-not-found
    /// with the standard JSON-RPC code.
    #[test]
    fn dispatch_always_returns_method_not_found_in_p1() {
        let err = dispatch("spark.world.query", json!({})).expect_err("must be Err in P1");
        assert!(matches!(err, McpError::MethodNotFound(ref n) if n == "spark.world.query"));
        assert_eq!(err.rpc_code(), -32601);
    }

    #[test]
    fn wrap_error_carries_rpc_code() {
        let time = Time::default();
        let env = wrap(&time, Err(McpError::method_not_found("spark.x")));
        assert_eq!(env["isError"], true);
        assert_eq!(env["_rpc_code"], -32601);
        assert_eq!(env["content"][0]["type"], "text");
        assert_eq!(env["content"][0]["text"], "method not found: spark.x");
    }

    /// Success path: `data` is nested under a frame-tagged [`Reply`]
    /// so even an empty handler payload still carries the frame.
    #[test]
    fn wrap_ok_pretty_prints_reply_with_frame() {
        let time = Time::default();
        let env = wrap(&time, Ok(json!({"a": 1})));
        assert_eq!(env["isError"], false);
        let text = env["content"][0]["text"].as_str().unwrap();
        let parsed: Value = serde_json::from_str(text).unwrap();
        assert_eq!(parsed["frame"], 0);
        assert_eq!(parsed["data"]["a"], 1);
    }

    #[test]
    fn tool_descriptions_is_empty_array_in_p1() {
        assert_eq!(tool_descriptions(), json!([]));
    }

    #[test]
    fn inbox_budget_is_256() {
        assert_eq!(INBOX_BUDGET, 256);
    }
}
