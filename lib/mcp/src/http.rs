//! `tiny_http` acceptor + MCP JSON-RPC routing.
//!
//! Single blocking thread. `POST /mcp` handles the MCP handshake
//! (`initialize` / `notifications/initialized` / `ping` / `tools/list`)
//! plus `tools/call`, which round-trips through the main thread via
//! the inbox. `GET /health` is a liveness probe. `GET /logs` is a P3
//! placeholder that returns 404 until the SSE log stream lands.

use std::net::SocketAddr;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use serde_json::{Value, json};
use tiny_http::{Header, Method, Response, Server};

use crate::methods::{Request, tool_descriptions};
use crate::types::{rpc_err, rpc_ok};

/// How long `tools/call` waits on the main thread to reply before
/// surfacing a JSON-RPC `−32001 timeout`.
///
/// 10 s in release; 100 ms under `cfg(test)` so the timeout path can be
/// covered by a unit test without making the suite drag.
#[cfg(not(test))]
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(test)]
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_millis(100);

/// MCP protocol version this server speaks. Pinned to the
/// 2025-11-25 revision; bump in lockstep with the spec.
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Run the acceptor loop on `addr`. Blocks the calling thread; returns
/// when the underlying server stops accepting (process shutdown).
///
/// # Errors
///
/// Returns the underlying I/O error if `tiny_http::Server::http` fails
/// to bind (port already in use, permission denied).
// `inbox_tx` is moved into the long-lived server loop, not consumed
// per call — the by-value handoff mirrors the thread-spawn pattern in
// `lib.rs` and matches `Sender`'s usual builder shape.
#[allow(clippy::needless_pass_by_value)]
pub fn serve(addr: SocketAddr, inbox_tx: Sender<Request>) -> std::io::Result<()> {
    let server = Server::http(addr).map_err(|e| {
        tracing::warn!(%addr, error = %e, "spark-mcp failed to bind");
        std::io::Error::other(e)
    })?;
    tracing::info!(%addr, "spark-mcp HTTP thread listening");
    for req in server.incoming_requests() {
        match (req.method(), req.url()) {
            (Method::Post, "/mcp") => handle_rpc(req, &inbox_tx),
            (Method::Get, "/health") => {
                let _ = req.respond(Response::from_string("ok"));
            }
            // `(Method::Get, "/logs")` lands here too: explicit 404
            // until P3's SSE stream replaces it. Folding it into the
            // catch-all avoids a clippy `identical-match-arms` flag
            // while keeping the route documented for the reader.
            _ => {
                let _ = req.respond(Response::empty(404));
            }
        }
    }
    Ok(())
}

/// Parse one JSON-RPC request, dispatch it, and write the response.
fn handle_rpc(mut req: tiny_http::Request, inbox: &Sender<Request>) {
    let mut body = Vec::new();
    if req.as_reader().read_to_end(&mut body).is_err() {
        let _ = req.respond(Response::from_string("read failed").with_status_code(400));
        return;
    }

    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, "spark-mcp JSON-RPC parse failed");
            // Parse error: id is unknown, so use Null per JSON-RPC spec.
            let _ = req.respond(json_response(rpc_err(Value::Null, -32700, &e.to_string())));
            return;
        }
    };

    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    let method = parsed.get("method").and_then(Value::as_str).unwrap_or("");
    let params = parsed.get("params").cloned().unwrap_or_else(|| json!({}));
    tracing::debug!(method, id = %id, "spark-mcp POST /mcp");

    match method {
        "initialize" => {
            let client_version = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(MCP_PROTOCOL_VERSION);
            let _ = req.respond(json_response(rpc_ok(
                id,
                initialize_result(negotiate_version(client_version)),
            )));
        }
        // Notifications carry no `id` and expect no body — HTTP 204.
        "notifications/initialized" => {
            let _ = req.respond(Response::empty(204));
        }
        "ping" => {
            let _ = req.respond(json_response(rpc_ok(id, json!({}))));
        }
        "tools/list" => {
            let _ = req.respond(json_response(rpc_ok(
                id,
                json!({ "tools": tool_descriptions() }),
            )));
        }
        "tools/call" => {
            let (reply_tx, reply_rx) = mpsc::channel::<Value>();
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));

            // If the main thread has already torn down the channel
            // (shutdown), surface that as a timeout — same observable
            // behaviour for the client.
            if inbox
                .send(Request::ToolCall {
                    name: name.clone(),
                    args,
                    reply: reply_tx,
                })
                .is_err()
            {
                tracing::error!(tool = %name, "spark-mcp inbox closed; main thread is gone");
                let _ = req.respond(json_response(rpc_err(id, -32001, "game loop shut down")));
                return;
            }

            let envelope = if let Ok(result) = reply_rx.recv_timeout(TOOL_CALL_TIMEOUT) {
                rpc_ok(id, result)
            } else {
                // Almost always means the winit event loop is on
                // `ControlFlow::Wait` and no OS event has woken the
                // main thread within the timeout window — see #50
                // (`Wait → Poll`) and the README's known-limitation
                // section. Surface it explicitly so a future debugger
                // doesn't have to guess.
                tracing::warn!(
                    tool = %name,
                    timeout_ms = TOOL_CALL_TIMEOUT.as_millis(),
                    "spark-mcp tools/call timed out — main thread did not service the inbox (event loop on Wait? see #50)",
                );
                rpc_err(id, -32001, "timeout waiting for game loop")
            };
            let _ = req.respond(json_response(envelope));
        }
        other => {
            tracing::debug!(method = other, "spark-mcp unknown JSON-RPC method");
            let _ = req.respond(json_response(rpc_err(
                id,
                -32601,
                &format!("method not found: {other}"),
            )));
        }
    }
}

/// Pick the version to advertise back to the client.
///
/// MCP §3.1 has the server *negotiate*: echo the client's version if
/// the handshake shape is compatible, otherwise offer the server's
/// version. The 2024-11-05 / 2025-03-26 / 2025-11-25 revisions share
/// the `initialize` / `tools/list` / `tools/call` shape we implement,
/// so we echo whatever the client sent — version-strict clients
/// (which abort on a non-matching echo) keep working. A missing or
/// empty `protocolVersion` field falls back to the server's pinned
/// [`MCP_PROTOCOL_VERSION`].
fn negotiate_version(client_version: &str) -> &str {
    if client_version.is_empty() {
        MCP_PROTOCOL_VERSION
    } else {
        client_version
    }
}

/// MCP `initialize` result. `protocol_version` is the negotiated
/// version per [`negotiate_version`] — the client may or may not be on
/// the same revision as the server.
fn initialize_result(protocol_version: &str) -> Value {
    json!({
        "protocolVersion": protocol_version,
        "capabilities": {
            "tools":   { "listChanged": false },
            "logging": {},
        },
        "serverInfo": {
            "name":    "spark-mcp",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// Wrap a serializable value in an `application/json` HTTP response.
///
/// `serde_json::to_vec` on a value we just built can only fail if a
/// custom `Serialize` impl panics — all of ours are derived or plain
/// `json!` literals, so the unwrap path is statically unreachable.
// Taking `Value` by value lets callers chain `json_response(rpc_ok(...))`
// without sprinkling `&` everywhere; the body is moved into the
// response either way.
#[allow(clippy::needless_pass_by_value)]
fn json_response(v: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(&v).expect("json envelope must serialize");
    let header: Header = "Content-Type: application/json"
        .parse()
        .expect("static header literal must parse");
    Response::from_data(body).with_header(header)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;

    /// `tools/call` blocks on a reply channel; if the main thread never
    /// answers, the wait must surface as a timeout after
    /// [`TOOL_CALL_TIMEOUT`]. We exercise the bare receiver here —
    /// constructing a real `tiny_http::Request` is not feasible in a
    /// unit test, but the timeout logic *is* the channel `recv_timeout`,
    /// so this directly covers the production code path.
    #[test]
    fn tool_call_recv_times_out_after_constant() {
        let (_tx, rx) = mpsc::channel::<Value>(); // sender alive, never sends
        let start = Instant::now();
        let err = rx
            .recv_timeout(TOOL_CALL_TIMEOUT)
            .expect_err("must time out");
        assert!(matches!(err, mpsc::RecvTimeoutError::Timeout));
        assert!(
            start.elapsed() >= TOOL_CALL_TIMEOUT,
            "should have waited at least the full timeout",
        );
    }

    /// Sanity that the `cfg(test)` override is short enough not to drag
    /// the suite. A ceiling, not an exact-value check — the constant is
    /// allowed to move within reason without breaking this test.
    #[test]
    fn test_timeout_override_is_short_enough() {
        assert!(
            TOOL_CALL_TIMEOUT <= Duration::from_millis(500),
            "test override must keep the suite under a second per timeout-path test",
        );
    }

    #[test]
    fn initialize_result_echoes_negotiated_version() {
        let v = initialize_result("2024-11-05");
        assert_eq!(v["protocolVersion"], "2024-11-05");
        assert_eq!(v["capabilities"]["tools"]["listChanged"], false);
        assert_eq!(v["serverInfo"]["name"], "spark-mcp");
        assert_eq!(v["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn negotiate_version_echoes_client_when_non_empty() {
        assert_eq!(negotiate_version("2024-11-05"), "2024-11-05");
        assert_eq!(negotiate_version("2025-11-25"), "2025-11-25");
        assert_eq!(negotiate_version(""), MCP_PROTOCOL_VERSION);
    }
}
