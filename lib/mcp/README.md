# spark-mcp

The Spark engine's debug-only MCP plugin. Exposes the running game to
AI agents over a local HTTP port so an MCP client (Claude Desktop, the
`claude` CLI, Cursor) can read the world, control time, and search
logs — *while the game is running*.

> **What's MCP?** [Model Context Protocol](https://modelcontextprotocol.io) —
> a small JSON-RPC dialect over HTTP. Servers expose **tools** the
> agent can call. `spark-mcp` is the server; Claude is one client.

> **Where do MCP requests go?** To the local TCP port `127.0.0.1:9123`
> in **debug builds only**. The crate root is `#![cfg(debug_assertions)]`,
> so `cargo build --release` does not include any of this code — no
> port opens, no thread spawns, no symbols ship.

P1 (this version) implements just the **MCP handshake**:
`initialize` → `notifications/initialized` → `ping` → `tools/list`.
`tools/list` returns an empty array; `tools/call` returns method-not-found
for any name. The point is to land the three-thread plumbing (winit
main, `tiny_http` acceptor, mpsc inbox + reply channels) before any
tools start using it. Real tools land in P2 (`spark.world.*`,
`spark.schema`, `spark.resource.*`) and P3 (`spark.control`,
`spark.logs.*`, `spark.frame_stats`).

## Plug it into the `Application`

`SparkMcpPlugin` slots in **after** `LogPlugin` (so its `spark-mcp ready`
log line shows up) and **before** any plugin that takes over the main
thread (today: `WindowPlugin`'s runner). The `cfg(debug_assertions)`
gate keeps the call out of release builds:

```rust,no_run
use spark_core::{Application, EngineError};

fn main() -> Result<(), EngineError> {
    // `let mut app` instead of one chained expression — see below.
    let mut app = Application::new();
    // app.add_plugin(LogPlugin);  — logging first so the "ready" line is captured

    #[cfg(debug_assertions)]
    app.add_plugin(spark_mcp::SparkMcpPlugin::default());

    // app.add_plugin(WindowPlugin::default());  — runner goes last
    app.run()
}
```

The `let mut app = …; app.add_plugin(…)` shape (instead of one chained
expression) is what makes the `#[cfg]` line work — you can't decorate
a method call inside a builder chain. Future cfg-gated plugins should
follow the same pattern.

## Using it from the game (`src/`)

The plugin is the only public surface a game binary touches.
Everything else (`Inbox`, `Reply`, `McpError`) is re-exported for the
benefit of *other engine crates* that may need to construct MCP wire
shapes — e.g. a future `spark-render` PR that fills out a
`ScreenshotReply`. Game code does not import anything from `spark-mcp`
directly; it just adds the plugin and lets agents drive.

## Using it from an engine crate (`lib/*`)

Engine crates that need to construct an MCP-shaped reply or extend the
tool surface depend on `spark-mcp` like any other sibling:

```toml
[dependencies]
spark-mcp = { path = "../mcp" }
```

Engine crates **never** call `tracing::*!` through `spark-log` — they
depend on `tracing` directly per the project's logging rule. The same
split applies to the MCP plugin: it is application-level scaffolding,
not something an internal crate like `spark-render` should know exists.
The only reason an engine crate ever takes a `spark-mcp` dep is to
register a new tool or a new reply type.

## Configuration

The plugin binds `127.0.0.1:9123` by default. Override with the public
`addr: SocketAddr` field if the port is taken:

```rust,no_run
use spark_mcp::SparkMcpPlugin;

let plugin = SparkMcpPlugin {
    addr: "127.0.0.1:9999".parse().unwrap(),
};
```

There is no `with_addr(...)` builder method on purpose — `WindowConfig`
style direct field assignment is the project convention for plain data
configs.

There are no env vars and no Cargo features. `RUST_LOG=spark_mcp=debug`
is the only knob, and it controls the plugin's own log lines through
the standard `spark-log` filter — see *Configuration with `RUST_LOG`*
in [`spark-log`](../log/README.md).

## Common patterns

### Smoke-test the server with `curl`

Run the game in one terminal, then from another:

```bash
curl -s http://127.0.0.1:9123/health
# → ok

curl -s -X POST http://127.0.0.1:9123/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{}}}'
# → {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25","capabilities":{...},"serverInfo":{"name":"spark-mcp","version":"0.1.0"}}}

curl -s -X POST http://127.0.0.1:9123/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'
# → {"jsonrpc":"2.0","id":2,"result":{"tools":[]}}
```

`tools/list` is empty in P1 by design — there are no tools yet. The
shape is locked down so a P2 PR can add tools without changing any
plumbing.

### Wire up Claude as an MCP client

For the `claude` CLI:

```bash
claude mcp add spark-mcp http://127.0.0.1:9123/mcp
```

For Claude Desktop, add an entry to `claude_desktop_config.json` under
`mcpServers`, pointing at the same URL. The client will run
`initialize` and `tools/list` automatically; until P2 lands you'll see
`spark-mcp` connected with zero tools.

### Inspect the empty `tools/call` round-trip

The P1 dispatcher always returns method-not-found, but the full round
trip — HTTP thread → mpsc inbox → main thread on `Stage::First` →
per-request reply channel → HTTP thread → JSON-RPC envelope — is
exercised:

```bash
curl -s -X POST http://127.0.0.1:9123/mcp \
  -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"spark.world.query","arguments":{}}}'
# → {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"method not found: spark.world.query"}],"isError":true,"_rpc_code":-32601}}
```

The `_rpc_code` field is a Spark extension — MCP's spec only carries
`isError`, but agents need the standard JSON-RPC code to branch on
(retry on `−32001` timeout, give up on `−32601` not-found, …). The
leading underscore marks it as an extension field.

## Errors / pitfalls

- **Port already in use.** The HTTP thread logs `spark-mcp server
  stopped` with the OS error and exits silently. The game keeps
  running; agents just can't connect. Pick a different port via
  `SparkMcpPlugin { addr: ... }` or kill the other process holding
  9123.
- **Release builds don't expose the server.** `#![cfg(debug_assertions)]`
  is the security model — there is no `--features remote-debug` and no
  TLS / auth in scope. If you run `cargo run --release -p spark`,
  `curl http://127.0.0.1:9123/health` returns connection refused. That
  is correct.
- **`tools/call` can time out.** If the main thread is wedged (e.g. a
  game system is in a long loop), the tool call returns JSON-RPC
  `-32001` after `TOOL_CALL_TIMEOUT` (10 s in release, 100 ms under
  `cfg(test)`). Agents should treat this as a transient signal: the
  game is busy, not broken.
- **No loopback rebind.** The address is whatever you pass — there is
  no automatic fallback to `0.0.0.0` or `[::1]`. Loopback IPv4 is the
  intentional default; cross-machine access is explicitly out of scope.
