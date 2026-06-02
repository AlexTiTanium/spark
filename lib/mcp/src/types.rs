//! Shared types — frame-tagged reply wrapper, the typed error enum, and
//! the JSON-RPC envelope helpers used by both `http.rs` and `methods.rs`.
//!
//! Nothing here touches the network or the world; the file is a pure
//! data layer so the dispatcher and the HTTP layer can be unit-tested
//! independently.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use serde_json::{Value, json};
use spark_common::Time;

/// Reply envelope that prefixes every tool response with the engine
/// frame it ran on plus a wall-clock millisecond timestamp.
///
/// Lets an agent correlate JSON results, screenshots, and `/logs`
/// entries by frame number — the only deterministic clock the engine
/// exposes. `at_ms` is for cross-process correlation (screen recording,
/// agent transcript), not for ordering within Spark.
///
/// # Examples
///
/// ```
/// use spark_common::Time;
/// use spark_mcp::Reply;
///
/// let time = Time::default();
/// let reply = Reply::new(&time, 42_u32);
/// assert_eq!(reply.frame, 0);
/// assert_eq!(reply.data, 42);
/// ```
#[derive(Debug, Serialize)]
pub struct Reply<T: Serialize> {
    /// Engine render-frame counter ([`Time::frame`]). Monotonic.
    pub frame: u64,
    /// Wall-clock milliseconds since the UNIX epoch.
    pub at_ms: u64,
    /// Tool-specific payload.
    pub data: T,
}

impl<T: Serialize> Reply<T> {
    /// Builds a reply tagged with the current frame from a [`Time`]
    /// borrow.
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_common::Time;
    /// use spark_mcp::Reply;
    ///
    /// let reply = Reply::new(&Time::default(), ());
    /// assert_eq!(reply.frame, 0);
    /// ```
    pub fn new(time: &Time, data: T) -> Self {
        Self {
            frame: time.frame(),
            at_ms: now_ms(),
            data,
        }
    }
}

/// Current wall-clock time as milliseconds since the UNIX epoch.
/// Returns `0` if the system clock is set before 1970 — preferable to a
/// panic for a debug-only telemetry field that nothing else depends on.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Errors a tool handler may return.
///
/// Each variant maps to a JSON-RPC error code via [`rpc_code`](Self::rpc_code)
/// per the design's §6.6 table. P1 only ever returns [`MethodNotFound`](Self::MethodNotFound)
/// from the dispatcher; the other variants exist because future phases
/// (P2–P4) plug into the same error surface and we want the codes locked
/// down before tools start using them.
#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum McpError {
    /// JSON did not match the tool's argument schema.
    #[error("invalid arguments: {0}")]
    BadArgs(String),
    /// `tools/call` referenced an entity id that is not alive.
    #[error("entity {0} not found")]
    EntityNotFound(u64),
    /// `tools/call` referenced a component type name that is not
    /// registered with `ComponentRegistry` (P2).
    #[error("unknown component: {0}")]
    UnknownComponent(String),
    /// `tools/call` referenced a resource that is not registered (P2).
    #[error("unknown resource: {0}")]
    UnknownResource(String),
    /// `tools/call` referenced a tool the dispatcher does not know.
    /// **The only variant the P1 dispatcher actually returns.**
    #[error("method not found: {0}")]
    MethodNotFound(String),
    /// The main thread did not service the request inside
    /// [`crate::TOOL_CALL_TIMEOUT`].
    #[error("timeout: {0}")]
    Timeout(&'static str),
    /// I/O failure (screenshot disk write, P4).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Catch-all for serde / serialization failures that the dispatcher
    /// should never trigger but must surface if they do.
    #[error("internal: {0}")]
    Internal(String),
}

impl McpError {
    /// Builds a [`BadArgs`](Self::BadArgs) from any `Display` value —
    /// typically the `serde_json::Error` returned by
    /// `serde_json::from_value`.
    pub fn bad_args(e: impl std::fmt::Display) -> Self {
        Self::BadArgs(e.to_string())
    }

    /// Builds an [`Internal`](Self::Internal) from any `Display` value.
    pub fn internal(e: impl std::fmt::Display) -> Self {
        Self::Internal(e.to_string())
    }

    /// Builds a [`MethodNotFound`](Self::MethodNotFound) from any
    /// string-like value.
    pub fn method_not_found(name: impl Into<String>) -> Self {
        Self::MethodNotFound(name.into())
    }

    /// JSON-RPC error code for this error.
    ///
    /// The code set is fixed by the [JSON-RPC 2.0 spec][spec] (−32700,
    /// −32601, −32602, −32603 are reserved) plus the Spark-specific
    /// −32001 (timeout) and −32002 (lookup miss). Stable so MCP clients
    /// can branch on it.
    ///
    /// [spec]: https://www.jsonrpc.org/specification#error_object
    ///
    /// # Examples
    ///
    /// ```
    /// use spark_mcp::McpError;
    ///
    /// assert_eq!(McpError::method_not_found("spark.x").rpc_code(), -32601);
    /// assert_eq!(McpError::bad_args("oops").rpc_code(),            -32602);
    /// assert_eq!(McpError::Timeout("foo").rpc_code(),              -32001);
    /// ```
    #[must_use]
    pub fn rpc_code(&self) -> i32 {
        match self {
            Self::BadArgs(_) => -32602,
            Self::MethodNotFound(_) => -32601,
            Self::Timeout(_) => -32001,
            Self::EntityNotFound(_) | Self::UnknownComponent(_) | Self::UnknownResource(_) => {
                -32002
            }
            Self::Io(_) | Self::Internal(_) => -32603,
        }
    }
}

// --- JSON-RPC envelope helpers ----------------------------------------------

/// Wraps a `result` value in a JSON-RPC 2.0 success envelope.
///
/// # Examples
///
/// ```
/// use serde_json::{json, Value};
/// use spark_mcp::rpc_ok;
///
/// let env = rpc_ok(json!(1), json!({"tools": []}));
/// assert_eq!(env["jsonrpc"], "2.0");
/// assert_eq!(env["id"], 1);
/// assert_eq!(env["result"]["tools"], json!([]));
/// ```
// `json!` moves `id` / `result` into the new value, but clippy's
// data-flow analysis sees them only behind a macro and flags them as
// "passed by value but not consumed." Taking ownership lets callers
// chain `rpc_ok(id, result)` without `&`, matching the rest of the
// JSON-builder surface.
#[allow(clippy::needless_pass_by_value)]
#[must_use]
pub fn rpc_ok(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// Wraps an error in a JSON-RPC 2.0 error envelope.
///
/// `id` is `Value::Null` for parse errors (no parsable id available)
/// and the caller's id otherwise.
///
/// # Examples
///
/// ```
/// use serde_json::{json, Value};
/// use spark_mcp::rpc_err;
///
/// let env = rpc_err(json!(1), -32601, "method not found: spark.x");
/// assert_eq!(env["error"]["code"], -32601);
/// assert_eq!(env["error"]["message"], "method not found: spark.x");
/// ```
// See `rpc_ok` — `json!` moves `id` into the envelope, but clippy's
// analysis can't see through the macro.
#[allow(clippy::needless_pass_by_value)]
#[must_use]
pub fn rpc_err(id: Value, code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use spark_common::Time;

    #[test]
    fn reply_reads_frame_from_time() {
        let time = Time::default();
        let reply = Reply::new(&time, "hello");
        assert_eq!(reply.frame, 0);
        assert_eq!(reply.data, "hello");
    }

    /// Table-driven check of every variant's JSON-RPC code, per the
    /// design's §6.6 mapping. Locks the wire codes down so a future
    /// rename can't silently shift them.
    #[test]
    fn rpc_code_table() {
        let cases: &[(McpError, i32)] = &[
            (McpError::bad_args("x"), -32602),
            (McpError::method_not_found("x"), -32601),
            (McpError::Timeout("x"), -32001),
            (McpError::EntityNotFound(1), -32002),
            (McpError::UnknownComponent("X".into()), -32002),
            (McpError::UnknownResource("X".into()), -32002),
            (McpError::internal("x"), -32603),
            (McpError::Io(std::io::Error::other("boom")), -32603),
        ];
        for (err, code) in cases {
            assert_eq!(err.rpc_code(), *code, "variant {err:?} mapped wrong");
        }
    }

    #[test]
    fn rpc_ok_envelope_shape() {
        let env = rpc_ok(json!(7), json!({"a": 1}));
        assert_eq!(env["jsonrpc"], "2.0");
        assert_eq!(env["id"], 7);
        assert_eq!(env["result"]["a"], 1);
        assert!(env.get("error").is_none());
    }

    #[test]
    fn rpc_err_envelope_shape() {
        let env = rpc_err(Value::Null, -32700, "parse error");
        assert_eq!(env["jsonrpc"], "2.0");
        assert_eq!(env["id"], Value::Null);
        assert_eq!(env["error"]["code"], -32700);
        assert_eq!(env["error"]["message"], "parse error");
        assert!(env.get("result").is_none());
    }
}
