# MCP_DESIGN — Spark MCP Plugin

**Status:** Final design, ready for prototype.
**Companion docs:** [`PLAN.md`](./PLAN.md), [`ECS_DESIGN.md`](./ECS_DESIGN.md), [`RENDER_API.md`](./RENDER_API.md), [`UI_DESIGN.md`](./UI_DESIGN.md).

## 0. TL;DR

`spark-mcp` is a debug-only ECS plugin that exposes the running game to AI agents over a local HTTP port. Agents connect as MCP clients to read and mutate ECS state, control the game loop, search logs, and take screenshots — every response tagged with the exact frame it ran on.

- One process, one plugin, no bridges, no second binary.
- `#![cfg(debug_assertions)]` — the crate is physically absent from release builds.
- Single non-stdlib dependency: `tiny_http`.
- 12 tools in MVP. Events and input integration land when ECS Stage 14 and `spark-input` exist.
- Sibling to the editor plugin (`lib/editor/`) — both consume the same reflection layer the ECS already plans to expose.

## 1. Goals and non-goals

**Goals.**
- Live programmatic inspection and mutation of ECS state from an agent.
- Frame-deterministic responses (every reply carries `frame` and `at_ms`).
- Game-loop control (pause, resume, step N ticks, time scale).
- On-disk screenshots tied to frame numbers for diff-style debugging.
- Searchable structured log access independent of stdout.
- Zero release-build overhead.

**Non-goals.**
- Production telemetry, multi-user access, network egress.
- Authentication, TLS, rate limiting. This is a dev tool on loopback in debug builds; absence is the security model.
- Replacing the editor plugin. Editor is the human surface; MCP is the agent surface. Both feed off one reflection layer in ECS.
- Hot-reloading systems or scripts (out of scope for this design; a future scripting layer can sit on top).

## 2. Design principles

- **One plugin, one process.** Industry double-process MCP bridges (Bevy `bevy_brp_mcp`, Unity, Unreal) exist because their plugin authors don't own the engine. We do. The bridge disappears.
- **Source code is the schema.** Components reflect their JSON layout via `serde`. Each type registers its source path; the agent reads the file when it needs structure. This is more accurate than a generated schema and costs zero infrastructure.
- **Standard library first.** No `tokio`, no `async`, no `axum`, no `hyper`. Three OS threads and `std::sync::mpsc` cover everything.
- **Engine changes earn their place independently.** Every modification we ask of `core`, `common`, `ecs`, or `render` is also useful without MCP — pause flag for menus, screenshot capture for trailers, source paths for the editor.
- **MVP is small and final.** 12 tools cover ECS read/write, control, screenshots, logs, frame stats. Events and input wait their turn. No speculative tools.

## 3. Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│ Game process (debug build only)                                  │
│                                                                  │
│   winit main thread ─── App::run() ── Scheduler.frame()         │
│      ├── Stage::First       drain_inbox(world)                │
│      ├── Stage::PreUpdate   apply_control(world)              │
│      ├── Stage::FixedUpdate (skipped if Time.paused)          │
│      ├── Stage::Update      (skipped if Time.paused)          │
│      ├── Stage::Render                                        │
│      └── Stage::Last        publish_events(world)             │
│                                                                  │
│      ↑ std::sync::mpsc<Request>          ↓ Bus<LogLine>          │
│      │                                    │                       │
│   ┌──┴────────────────────────────────────┴───────────────────┐  │
│   │ HTTP thread (tiny_http)                                   │  │
│   │   POST /mcp  → JSON-RPC routing                            │  │
│   │   GET /logs  → SSE log stream                              │  │
│   │   GET /health                                              │  │
│   └────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              ▲
                              │ MCP Streamable HTTP
                              │ http://127.0.0.1:9123/mcp
                              ▼
                       Claude Desktop · Cursor · claude-cli
```

Three threads:
1. Main thread runs the ECS schedule (driven by winit).
2. One HTTP listener thread (tiny_http acceptor).
3. One short-lived thread per active SSE stream.

Two channels:
1. `std::sync::mpsc<Request>` — HTTP thread → main thread.
2. `Bus<LogLine>` — main thread → all subscribed SSE streams.

Three ECS systems registered by the plugin:
1. `drain_inbox` in `Stage::First` — processes pending tool calls under exclusive `&mut World`.
2. `apply_control` in `Stage::PreUpdate` — applies pause/step/scale onto `Time` for the rest of the frame.
3. `publish_events` in `Stage::Last` — placeholder until events land (§ 10).

## 4. Cross-doc review

Before this design merges, the following changes propagate to companion docs.

### 4.1 PLAN.md

- Add `lib/common/` to the repo layout as a sibling of `lib/core/`.
- Move `Time`, `InputState`, `ChangeLog`, and other engine-global Resources that are not part of the boot harness from `spark-core` into `spark-common`. `spark-core` keeps `App`, `Plugin`, `Schedule`, the boot harness, the root error type, and tracing init.
- Update the module dependency graph:
  - `common → ecs`
  - `core → ecs` (no dependency on `common`)
  - `window → core` (and `common` when it needs `Time`)
  - `render → core + common`
  - `mcp → core + ecs + common`

### 4.2 ECS_DESIGN.md

- **Stage 7 (`#[derive(Component)]`):** extend the planned `ComponentRegistry` entry with two fields beyond the documented "name, TypeId, Debug formatter, optional serde hooks":
  - `source_path: &'static str`, generated by `concat!(file!(), ":", line!())` in the derive output.
  - JSON hooks (`to_json`, `from_json`, `remove`) populated when `T: Serialize + DeserializeOwned` is in scope at the derive site; absent otherwise. Components without `serde` derive remain valid Spark components but are invisible to MCP and to the editor's JSON inspector.
- **Stage 14 (Events):** the same pattern repeats for `#[derive(Event)]` to give MCP its `EventRegistry` for free. No work needed before Stage 14.
- No new system params, no new lifetimes. MCP uses what Stage 7 and Stage 14 produce.

### 4.3 RENDER_API.md

- Add one config resource:
  ```rust
  #[derive(Resource, Default)]
  pub struct ScreenshotRequest {
      pub pending: Option<std::sync::mpsc::Sender<ScreenshotResult>>,
  }

  pub struct ScreenshotResult {
      pub png: Vec<u8>,
      pub width: u32,
      pub height: u32,
  }
  ```
- The render plugin's post-present system inspects this resource; if `pending` is `Some`, it reads back the swap-chain texture, encodes PNG, and posts the result on the channel. ~50 lines of `wgpu` code that pays for itself in trailer captures and test artifacts.

### 4.4 UI_DESIGN.md

- No changes required. The editor plugin and MCP are both consumers of the reflection layer that ECS already exposes (`ComponentRegistry`, `FrameTrace`, `ChangeLog`, `CommandLog`). They never call each other; they never overlap. UI_DESIGN.md line 44 already calls these reflection APIs out as the editor's input — MCP joins the same trough.

### 4.5 No conflicts

`GAME_DESIGN.md`, `RENDER_CONCEPT.md`, `RENDER_ROADMAP.md`, and `Spark_Renderer_Feature_Catalog_RU.md` are unaffected.

## 5. Engine contract

The complete set of additions to the rest of the engine that this design assumes. All are useful outside MCP.

| Crate | Addition | Lines | Independent value |
|---|---|---:|---|
| `spark-common` (new) | Hosts `Time`, `InputState`, `ChangeLog`, etc. Moved from `core`. | — (refactor) | Stops `core` from accumulating engine-wide state |
| `spark-common` | `Time::paused: bool`, `Time::scale: f32` | ~5 | Menu pause, slow-motion, replay |
| `spark-core` (scheduler) | Skip `FixedUpdate` and `Update` when `Time.paused`; multiply `delta` by `Time.scale` | ~5 | Same as above |
| `spark-ecs` (derive) | `ComponentRegistry` entry gets `source_path` + JSON hooks | ~30 (derive) | Editor uses identical hooks for its inspector |
| `spark-render` | `ScreenshotRequest` resource + post-present readback system | ~50 | Marketing screenshots, regression tests |
| `spark-core` (tracing) | `tracing_subscriber` set up via `Registry::default().with(layer)` so extra layers can attach | ~5 | Standard Rust practice |

Total: ~100 lines of engine work spread across crates that need it anyway.

## 6. Plugin internals

### 6.1 Cargo.toml

```toml
[package]
name = "spark-mcp"
edition.workspace = true
rust-version.workspace = true
license.workspace = true

[lints]
workspace = true

[dependencies]
spark-core   = { path = "../core" }
spark-ecs    = { path = "../ecs" }
spark-common = { path = "../common" }

serde      = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
tracing    = { workspace = true }
thiserror  = { workspace = true }

tiny_http  = "0.12"
```

One non-stdlib dependency: `tiny_http` (pure-Rust, sync, ~30 KB, single transitive `ascii`).

### 6.2 File layout

```
lib/mcp/
├── Cargo.toml
└── src/
    ├── lib.rs              # SparkMcpPlugin                       ~80 lines
    ├── bus.rs              # Bus<T> built on std::sync::mpsc      ~30
    ├── http.rs             # tiny_http server + MCP routing      ~180
    ├── methods.rs          # dispatcher + run! macro              ~80
    ├── handlers.rs         # tool implementations, 5-15 lines each ~250
    ├── types.rs            # Args, Reply structs, McpError, Reply ~200
    ├── control.rs          # McpControl + apply_control system    ~40
    ├── screenshot.rs       # disk path management                 ~30
    └── logs.rs             # LogBuffer + tracing Layer + search   ~120
```

Total: 9 files, ~1010 lines. Linear, browsable, no macros that obscure intent beyond the `run!` dispatch helper.

### 6.3 Plugin entry — `lib.rs`

```rust
//! Spark MCP — single-process debug protocol for AI agents.
//! Compiled out of release builds.

#![cfg(debug_assertions)]

mod bus;
mod control;
mod handlers;
mod http;
mod logs;
mod methods;
mod screenshot;
mod types;

use std::net::SocketAddr;
use std::sync::mpsc;
use std::thread;

use spark_core::{App, Plugin};
use spark_ecs::{Schedule, WorkloadLabel};

#[derive(WorkloadLabel)]
pub enum McpWorkload { Inbox, Control, Outbox }

pub struct SparkMcpPlugin {
    pub addr: SocketAddr,
}

impl Default for SparkMcpPlugin {
    fn default() -> Self {
        Self { addr: "127.0.0.1:9123".parse().unwrap() }
    }
}

impl Plugin for SparkMcpPlugin {
    fn build(&self, app: &mut App) {
        // Reset on-disk screenshot dir so sessions do not mingle.
        screenshot::reset_dir();

        let (req_tx, req_rx) = mpsc::channel::<methods::Request>();
        let log_buf  = logs::LogBuffer::default();
        let log_bus  = bus::Bus::new();

        logs::try_attach_layer(&log_buf, &log_bus);

        app.init_resource::<control::McpControl>()
           .insert_resource(methods::Inbox::new(req_rx))
           .insert_resource(log_buf.clone())
           .add_workload(McpWorkload::Inbox, Stage::First, |w| {
               w.add(methods::drain_inbox);
           })
           .add_workload(McpWorkload::Control, Stage::PreUpdate, |w| {
               w.add(control::apply_control);
           })
           .add_workload(McpWorkload::Outbox, Stage::Last, |w| {
               w.add(methods::publish_events);
           });

        let addr = self.addr;
        thread::Builder::new()
            .name("spark-mcp-http".into())
            .spawn(move || {
                if let Err(e) = http::serve(addr, req_tx, log_bus, log_buf) {
                    tracing::warn!(error = %e, "spark-mcp server stopped");
                }
            })
            .expect("spawn spark-mcp thread");

        tracing::info!(addr = %self.addr, "spark-mcp ready");
    }
}
```

In the game binary:

```rust
#[cfg(debug_assertions)]
app.add_plugin(spark_mcp::SparkMcpPlugin::default());
```

In release builds the crate is not even compiled.

### 6.4 HTTP server — `http.rs`

`tiny_http` blocking acceptor on the main HTTP thread. Each request handled in-line for `POST /mcp` (cheap, sub-millisecond). Long-lived SSE responses each spawn a writer thread.

```rust
use std::io::Read;
use std::net::SocketAddr;
use std::sync::mpsc::Sender;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Method, Response, Server};

use crate::bus::Bus;
use crate::logs::{LogBuffer, LogLine};
use crate::methods::{self, Request};

pub fn serve(
    addr: SocketAddr,
    inbox_tx: Sender<Request>,
    log_bus: Bus<LogLine>,
    _log_buf: LogBuffer,
) -> std::io::Result<()> {
    let server = Server::http(addr).map_err(std::io::Error::other)?;
    for req in server.incoming_requests() {
        match (req.method(), req.url()) {
            (&Method::Post, "/mcp")    => handle_rpc(req, &inbox_tx),
            (&Method::Get,  "/logs")   => spawn_log_sse(req, log_bus.subscribe()),
            (&Method::Get,  "/health") => { let _ = req.respond(Response::from_string("ok")); }
            _ => { let _ = req.respond(Response::empty(404)); }
        }
    }
    Ok(())
}

fn handle_rpc(mut req: tiny_http::Request, inbox: &Sender<Request>) {
    let mut body = Vec::new();
    if req.as_reader().read_to_end(&mut body).is_err() {
        let _ = req.respond(Response::from_string("read failed").with_status_code(400));
        return;
    }
    let parsed: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            let _ = req.respond(json_resp(rpc_err(Value::Null, -32700, &e.to_string())));
            return;
        }
    };
    let id = parsed.get("id").cloned().unwrap_or(Value::Null);
    let method = parsed.get("method").and_then(Value::as_str).unwrap_or("");
    let params = parsed.get("params").cloned().unwrap_or(json!({}));

    let resp = match method {
        "initialize" => json!({
            "jsonrpc": "2.0", "id": id,
            "result": {
                "protocolVersion": "2025-11-25",
                "capabilities":    { "tools": { "listChanged": false }, "logging": {} },
                "serverInfo":      { "name": "spark-mcp", "version": env!("CARGO_PKG_VERSION") },
            }
        }),
        "notifications/initialized" => { let _ = req.respond(Response::empty(204)); return; }
        "ping"        => json!({ "jsonrpc": "2.0", "id": id, "result": {} }),
        "tools/list"  => json!({ "jsonrpc": "2.0", "id": id,
                                 "result": { "tools": methods::tool_descriptions() } }),
        "tools/call"  => {
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            let name = params.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let _ = inbox.send(Request::ToolCall { name, args, reply: reply_tx });
            match reply_rx.recv_timeout(Duration::from_secs(10)) {
                Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
                Err(_)     => rpc_err(id, -32001, "timeout waiting for game loop"),
            }
        }
        _ => rpc_err(id, -32601, &format!("method not found: {method}")),
    };
    let _ = req.respond(json_resp(resp));
}

fn json_resp(v: Value) -> Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_vec(&v).unwrap();
    Response::from_data(body)
        .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap())
}

fn rpc_err(id: Value, code: i32, msg: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } })
}

fn spawn_log_sse(req: tiny_http::Request, rx: std::sync::mpsc::Receiver<LogLine>) {
    std::thread::spawn(move || {
        use std::io::Write;
        let (mut writer, _) = match req.into_writer() { Ok(p) => p, Err(_) => return };
        let _ = writer.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
              Cache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n");
        while let Ok(line) = rx.recv() {
            let s = format!("data: {}\n\n", serde_json::to_string(&line).unwrap());
            if writer.write_all(s.as_bytes()).is_err() { break; }
        }
    });
}
```

### 6.5 Dispatcher — `methods.rs`

```rust
//! MCP method dispatch — turns name + args into typed handler calls.
//!
//! # Spark ECS API assumptions (PSEUDO until M4 lands)
//!
//! * `world.entity_iter() -> impl Iterator<Item = Entity>`
//! * `world.has::<T>(e)`, `get::<T>(e)`, `insert(e, t)`, `remove::<T>(e)`, `despawn(e)`
//! * `world.resource::<T>()`, `world.resource_mut::<T>()`
//!
//! Exact method names follow the ECS implementation; replace at call sites
//! when ECS Stage 7-14 lands. The dispatch shape is independent of these names.

use std::sync::mpsc::{Receiver, Sender};
use serde_json::Value;
use spark_ecs::World;
use spark_core::Resource;

use crate::handlers;
use crate::types::*;

#[derive(Resource)]
pub struct Inbox { rx: Receiver<Request> }
impl Inbox { pub fn new(rx: Receiver<Request>) -> Self { Self { rx } } }

pub enum Request {
    ToolCall { name: String, args: Value, reply: Sender<Value> },
}

/// `Stage::First` system. Drains MCP requests under exclusive `&mut World`.
/// Spark ECS schedules `&mut World` systems in their own batch (M4 parallel executor).
pub fn drain_inbox(world: &mut World) {
    let mut budget = 256usize;
    while budget > 0 {
        let req = {
            let inbox = world.resource::<Inbox>();
            match inbox.rx.try_recv() { Ok(r) => r, Err(_) => return }
        };
        budget -= 1;
        let Request::ToolCall { name, args, reply } = req;
        let _ = reply.send(wrap(dispatch(&name, args, world)));
    }
}

/// `Stage::Last` system. Placeholder until events land in ECS Stage 14;
/// see § 10 Future work.
pub fn publish_events(_world: &mut World) {}

fn dispatch(name: &str, args: Value, world: &mut World) -> Result<Value, McpError> {
    macro_rules! run {
        ($handler:expr) => {{
            let parsed = serde_json::from_value(args).map_err(McpError::bad_args)?;
            let data   = $handler(world, parsed)?;
            serde_json::to_value(Reply::new(world, data)).map_err(McpError::internal)?
        }};
        (no_args $handler:expr) => {{
            let data = $handler(world)?;
            serde_json::to_value(Reply::new(world, data)).map_err(McpError::internal)?
        }};
    }

    Ok(match name {
        "spark.world.query"   => run!(handlers::world_query),
        "spark.world.get"     => run!(handlers::world_get),
        "spark.world.spawn"   => run!(handlers::world_spawn),
        "spark.world.insert"  => run!(handlers::world_insert),
        "spark.world.remove"  => run!(handlers::world_remove),
        "spark.world.despawn" => run!(handlers::world_despawn),
        "spark.resource.get"  => run!(handlers::resource_get),
        "spark.resource.set"  => run!(handlers::resource_set),
        "spark.schema"        => run!(no_args handlers::schema),
        "spark.control"       => run!(handlers::control),
        "spark.screenshot"    => run!(no_args handlers::screenshot),
        "spark.logs.tail"     => run!(handlers::logs_tail),
        "spark.logs.search"   => run!(handlers::logs_search),
        "spark.frame_stats"   => run!(no_args handlers::frame_stats),
        other => return Err(McpError::method_not_found(other)),
    })
}

fn wrap(r: Result<Value, McpError>) -> Value {
    match r {
        Ok(v)  => serde_json::json!({
            "content":   [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap() }],
            "isError":   false,
        }),
        Err(e) => serde_json::json!({
            "content":   [{ "type": "text", "text": e.to_string() }],
            "isError":   true,
            "_rpc_code": e.rpc_code(),
        }),
    }
}

/// Tool descriptions for `tools/list`. Each `description` is written as a
/// short prompt: it tells the agent when to use the tool, when not to,
/// and what to call before it.
pub fn tool_descriptions() -> Value {
    use serde_json::json;
    json!([
        tool("spark.world.query",
             "List entities matching a component filter. Use this BEFORE spark.world.get \
              when you do not know which entities exist. Pass `fetch` to inline component \
              values; leave it empty to get just entity ids. If you do not know component \
              type names, call `spark.schema` first.",
             json!({
                 "type": "object",
                 "properties": {
                     "with":    { "type": "array", "items": { "type": "string" } },
                     "without": { "type": "array", "items": { "type": "string" } },
                     "fetch":   { "type": "array", "items": { "type": "string" } },
                     "limit":   { "type": "integer", "default": 100 }
                 }
             })),
        // ... remaining 11 tools, same shape.
    ])
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    serde_json::json!({ "name": name, "description": description, "inputSchema": schema })
}
```

### 6.6 Types — `types.rs`

```rust
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use spark_ecs::World;

/// Wrapper that prefixes every reply with the frame and wall-clock millis it ran on.
/// Lets agents cross-reference responses against their own logs deterministically.
#[derive(Serialize)]
pub struct Reply<T: Serialize> {
    pub frame: u64,
    pub at_ms: u64,
    pub data: T,
}

impl<T: Serialize> Reply<T> {
    pub fn new(world: &World, data: T) -> Self {
        let time = world.resource::<spark_common::Time>();
        Self {
            frame: time.frame,
            at_ms: now_ms(),
            data,
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// --- Args -------------------------------------------------------------------

#[derive(Deserialize)]
pub struct QueryArgs {
    #[serde(default)] pub with:    Vec<String>,
    #[serde(default)] pub without: Vec<String>,
    #[serde(default)] pub fetch:   Vec<String>,
    #[serde(default = "default_limit")] pub limit: usize,
}
fn default_limit() -> usize { 100 }

#[derive(Deserialize)] pub struct GetArgs     { pub entity: u64, #[serde(default)] pub components: Vec<String> }
#[derive(Deserialize)] pub struct SpawnArgs   { pub components: serde_json::Map<String, Value> }
#[derive(Deserialize)] pub struct InsertArgs  { pub entity: u64, pub components: serde_json::Map<String, Value> }
#[derive(Deserialize)] pub struct RemoveArgs  { pub entity: u64, pub components: Vec<String> }
#[derive(Deserialize)] pub struct DespawnArgs { pub entity: u64 }

#[derive(Deserialize)] pub struct ResourceGetArgs { pub name: String }
#[derive(Deserialize)] pub struct ResourceSetArgs { pub name: String, pub value: Value }

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum ControlArgs {
    Pause,
    Resume,
    Step  { n: u32 },
    Scale { scale: f32 },
}

#[derive(Deserialize)]
pub struct LogsTailArgs   { #[serde(default = "default_log_limit")] pub limit: usize }
#[derive(Deserialize)]
pub struct LogsSearchArgs {
    pub query: String,
    #[serde(default = "default_level")] pub level_min: String,
    #[serde(default = "default_log_limit")] pub limit: usize,
}
fn default_log_limit() -> usize { 50 }
fn default_level()     -> String { "trace".into() }

// --- Replies ----------------------------------------------------------------

#[derive(Serialize)] pub struct OkReply       {}
#[derive(Serialize)] pub struct EntityReply   { pub entity: u64 }

#[derive(Serialize)]
pub struct EntityView {
    pub entity: u64,
    pub components: BTreeMap<String, Value>,
}

#[derive(Serialize)]
pub struct QueryReply {
    pub entities: Vec<EntityView>,
    pub total: usize,
    pub truncated: bool,
}

#[derive(Serialize)]
pub struct ControlReply {
    pub paused: bool,
    pub scale: f32,
    pub pending_steps: u32,
}

#[derive(Serialize)]
pub struct ScreenshotReply {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Serialize)] pub struct LogsReply       { pub lines: Vec<crate::logs::LogLine> }
#[derive(Serialize)] pub struct FrameStatsReply { pub frame: u64, pub delta_ms: f64, pub fps: f64 }

#[derive(Serialize)] pub struct TypeMeta { pub source_path: &'static str }

#[derive(Serialize)]
pub struct SchemaReply {
    pub components: BTreeMap<String, TypeMeta>,
    pub events:     BTreeMap<String, TypeMeta>,
    pub resources:  BTreeMap<String, TypeMeta>,
}

// --- Errors -----------------------------------------------------------------

#[derive(thiserror::Error, Debug)]
pub enum McpError {
    #[error("invalid arguments: {0}")] BadArgs(String),
    #[error("entity {0} not found")]   EntityNotFound(u64),
    #[error("unknown component: {0}")] UnknownComponent(String),
    #[error("unknown resource: {0}")]  UnknownResource(String),
    #[error("method not found: {0}")]  MethodNotFound(String),
    #[error("timeout: {0}")]           Timeout(&'static str),
    #[error("io: {0}")]                Io(#[from] std::io::Error),
    #[error("internal: {0}")]          Internal(String),
}

impl McpError {
    pub fn bad_args(e: impl std::fmt::Display) -> Self { Self::BadArgs(e.to_string()) }
    pub fn internal(e: impl std::fmt::Display) -> Self { Self::Internal(e.to_string()) }
    pub fn timeout(what: &'static str)          -> Self { Self::Timeout(what) }
    pub fn method_not_found(name: impl Into<String>) -> Self { Self::MethodNotFound(name.into()) }

    /// JSON-RPC error code.
    pub fn rpc_code(&self) -> i32 {
        match self {
            Self::BadArgs(_)            => -32602,
            Self::MethodNotFound(_)     => -32601,
            Self::Timeout(_)            => -32001,
            Self::EntityNotFound(_)
            | Self::UnknownComponent(_)
            | Self::UnknownResource(_)  => -32002,
            Self::Io(_) | Self::Internal(_) => -32603,
        }
    }
}
```

### 6.7 Handler example — `handlers.rs`

Every handler is a pure function from `(World, Args) → Result<Reply, McpError>`. No `json!` literals, no string-keyed lookups; everything typed.

```rust
use spark_ecs::{Entity, World};
use crate::types::*;

pub fn world_query(world: &mut World, args: QueryArgs) -> Result<QueryReply, McpError> {
    let registry = world.resource::<spark_ecs::ComponentRegistry>();
    let mut entities = Vec::with_capacity(args.limit);
    let mut total = 0;
    let mut truncated = false;

    for entity in world.entity_iter() {                                  // PSEUDO
        if !args.with.iter().all(|n| registry.has_named(world, entity, n))   { continue; }
        if  args.without.iter().any(|n| registry.has_named(world, entity, n)) { continue; }
        total += 1;

        if entities.len() >= args.limit { truncated = true; continue; }

        let components = args.fetch.iter()
            .filter_map(|n| registry.to_json_named(world, entity, n).map(|v| (n.clone(), v)))
            .collect();

        entities.push(EntityView { entity: entity.to_bits(), components });
    }
    Ok(QueryReply { entities, total, truncated })
}
```

The remaining 11 handlers follow the same shape, each 5-15 lines, none with hidden complexity.

### 6.8 Control resource — `control.rs`

```rust
use spark_core::{Res, ResMut, Resource};
use spark_common::Time;

#[derive(Resource, Default)]
pub struct McpControl {
    /// What the agent last asked for. Mirrored onto `Time.paused` each frame.
    pub want_paused: bool,
    /// Frames the agent has scheduled while paused. Decrements each frame
    /// until zero, then `want_paused` re-applies.
    pub pending_steps: u32,
}

/// `Stage::PreUpdate` system. Single source of truth for the paused flag.
pub fn apply_control(
    mut ctl: ResMut<McpControl>,
    mut time: ResMut<Time>,
) {
    if ctl.pending_steps > 0 {
        time.paused = false;
        ctl.pending_steps -= 1;
        if ctl.pending_steps == 0 {
            time.paused = ctl.want_paused;
        }
    } else {
        time.paused = ctl.want_paused;
    }
}
```

### 6.9 Screenshots — `screenshot.rs`

```rust
use std::path::PathBuf;
use std::time::Duration;
use spark_ecs::World;
use spark_render::{ScreenshotRequest, ScreenshotResult};                    // PSEUDO crate path
use spark_common::Time;
use crate::types::{McpError, ScreenshotReply};

pub fn dir() -> PathBuf { PathBuf::from("target/spark-mcp/screenshots") }

pub fn reset_dir() {
    let _ = std::fs::remove_dir_all(dir());
    let _ = std::fs::create_dir_all(dir());
}

pub fn take(world: &mut World) -> Result<ScreenshotReply, McpError> {
    let (tx, rx) = std::sync::mpsc::channel::<ScreenshotResult>();
    world.resource_mut::<ScreenshotRequest>().pending = Some(tx);

    let result = rx.recv_timeout(Duration::from_secs(2))
        .map_err(|_| McpError::timeout("screenshot"))?;

    let frame = world.resource::<Time>().frame;
    let path  = dir().join(format!("{frame:08}.png"));
    std::fs::write(&path, &result.png)?;

    Ok(ScreenshotReply {
        path:   path.to_string_lossy().into_owned(),
        width:  result.width,
        height: result.height,
    })
}
```

### 6.10 Logs — `logs.rs`

```rust
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use serde::Serialize;
use spark_core::Resource;
use tracing::Level;

use crate::bus::Bus;

#[derive(Clone, Serialize)]
pub struct LogLine {
    pub frame:  u64,
    pub at_ms:  u64,
    pub level:  &'static str,
    pub target: String,
    pub msg:    String,
}

#[derive(Resource, Default, Clone)]
pub struct LogBuffer(pub Arc<Mutex<VecDeque<LogLine>>>);

impl LogBuffer {
    const CAPACITY: usize = 4096;

    pub fn tail(&self, limit: usize) -> Vec<LogLine> {
        let buf = self.0.lock().unwrap();
        buf.iter().rev().take(limit).cloned().collect()
    }

    pub fn search(&self, query: &str, level_min: Level, limit: usize) -> Vec<LogLine> {
        let q = query.to_lowercase();
        let buf = self.0.lock().unwrap();
        buf.iter().rev()
            .filter(|l| level_at_least(l.level, level_min))
            .filter(|l| l.msg.to_lowercase().contains(&q)
                     || l.target.to_lowercase().contains(&q))
            .take(limit).cloned().collect()
    }

    fn push(&self, line: LogLine) {
        let mut q = self.0.lock().unwrap();
        if q.len() >= Self::CAPACITY { q.pop_front(); }
        q.push_back(line);
    }
}

pub fn parse_level(s: &str) -> Result<Level, String> {
    match s.to_ascii_lowercase().as_str() {
        "trace" => Ok(Level::TRACE),
        "debug" => Ok(Level::DEBUG),
        "info"  => Ok(Level::INFO),
        "warn"  => Ok(Level::WARN),
        "error" => Ok(Level::ERROR),
        other   => Err(format!("unknown level: {other}")),
    }
}

fn level_at_least(line: &str, min: Level) -> bool {
    parse_level(line).map(|l| l <= min).unwrap_or(true)
}

/// Tries to attach the layer to the global tracing subscriber. Silently no-ops
/// if the subscriber was set up without an extensible registry. Spark-core
/// uses `Registry::default().with(...)` so this attaches cleanly in debug builds.
pub fn try_attach_layer(buf: &LogBuffer, bus: &Bus<LogLine>) {
    // PSEUDO: attach a tracing_subscriber::Layer that pushes each event into
    // `buf` and `bus`. Implementation is ~50 lines using
    // tracing_subscriber::Layer::on_event + a small visitor that flattens
    // structured fields into the `msg` string.
    let _ = (buf, bus);
}
```

## 7. Tool surface

| Tool | Purpose | Args | Reply |
|---|---|---|---|
| `spark.world.query` | List entities by component filter | `QueryArgs` | `QueryReply` |
| `spark.world.get` | Get one entity's components | `GetArgs` | `EntityView` |
| `spark.world.spawn` | Spawn an entity with components | `SpawnArgs` | `EntityReply` |
| `spark.world.insert` | Add or replace components on an entity | `InsertArgs` | `OkReply` |
| `spark.world.remove` | Remove components from an entity | `RemoveArgs` | `OkReply` |
| `spark.world.despawn` | Destroy an entity | `DespawnArgs` | `OkReply` |
| `spark.resource.get` | Read a Resource by type name | `ResourceGetArgs` | `Value` |
| `spark.resource.set` | Write a Resource by type name | `ResourceSetArgs` | `OkReply` |
| `spark.schema` | List registered types and source paths | — | `SchemaReply` |
| `spark.control` | Pause, resume, step, or set time scale | `ControlArgs` | `ControlReply` |
| `spark.screenshot` | Capture frame to disk, return path | — | `ScreenshotReply` |
| `spark.logs.tail` | Last N log lines | `LogsTailArgs` | `LogsReply` |
| `spark.logs.search` | Substring search across log buffer | `LogsSearchArgs` | `LogsReply` |
| `spark.frame_stats` | Current FPS and frame timing | — | `FrameStatsReply` |

Every reply is wrapped in `Reply<T> { frame, at_ms, data }`.

### SSE streams

| Endpoint | Purpose |
|---|---|
| `GET /logs?level_min=warn` | Stream `LogLine` events as `text/event-stream`. |
| `GET /health` | Liveness probe. |

`GET /events` arrives in Phase 2 when ECS events land.

## 8. Concurrency model

- HTTP thread parses JSON-RPC, builds a `Request`, and sends it through the `mpsc::Sender<Request>`. It then blocks on a per-request `oneshot` until the main thread responds, up to a 10-second timeout.
- The main thread drains the inbox in `Stage::First` under exclusive `&mut World`. Spark ECS's M4 parallel executor schedules `&mut World` systems in their own batch, so this is safe and explicit.
- A per-frame budget of 256 requests caps how much the main thread will service before yielding to the rest of the schedule. Excess requests wait one or more frames.
- Log events publish into a `Bus<LogLine>` (an `Arc<Mutex<Vec<Sender>>>`) from any thread — the tracing layer pushes lines as they occur.
- Each active SSE stream runs in its own short-lived OS thread that owns the `Receiver` half of one channel.

There is no async runtime. There is no shared state outside the mpsc and the broadcast bus. There are no locks held across `.await` because there are no awaits.

## 9. Security model

There is no security model. The plugin is `#![cfg(debug_assertions)]`. In release builds the entire crate is omitted. In debug builds it binds `127.0.0.1` only, no authentication, no CORS, no token. If a malicious local process can connect to the loopback port, the developer has a bigger problem than MCP.

If a future need arises (sharing a debug build remotely, e.g. with a teammate over a tunnel), a `SparkMcpPlugin::with_token(secret)` opt-in can be added in ~20 lines. Not in MVP.

## 10. Future work

### 10.1 Events (after ECS Stage 14)

When `Events<T>`, `EventReader<T>`, and `EventWriter<T>` exist, three additions:

- `EventRegistry` populated by `#[derive(Event)]`, mirroring `ComponentRegistry`.
- Tools `spark.event.list` (registered event types + source paths) and `spark.event.emit { type, payload }` (writes through `EventWriter<T>`).
- `GET /events?types=Foo,Bar` SSE stream. The `publish_events` system in `Stage::Last` drains `EventReader<T>` for each subscribed type and forwards onto a `Bus<NamedEvent>`. Filtering happens server-side based on subscription query params.

No design work needed in advance. The shapes mirror the component plumbing exactly.

### 10.2 Input injection (after `spark-input` lands)

Two paths depending on how input is built:

- If input flows through events (`KeyPressed`, `MouseMoved` events), MCP gets input injection for free via `spark.event.emit`.
- If input is purely state-based (`InputState` Resource only), an `InputOverride` Resource can be layered on top: MCP writes the override, the input system applies it on top of (or instead of) winit-derived state for one frame.

The decision is deferred until `spark-input` exists.

### 10.3 Scenario tool (after MVP + events)

`spark.scenario { actions: [...], assert: ... }` — runs a sequence of tool calls plus a JSON predicate against the resulting world. Provides agentic automated tests at game-loop speed without writing Rust integration tests. Builds entirely on existing tools.

### 10.4 Resource registry

`spark.schema` returns an empty `resources` map in MVP. When a `ResourceRegistry` lands in ECS (analogous to `ComponentRegistry`), the map fills in automatically.

### 10.5 Command and change history

When `CommandLog` and `ChangeLog` resources land (ECS Phase 2), expose `spark.history` and `spark.changes` tools. Lets an agent see what the previous agent (or this agent's earlier turn) did. Free observability.

## 11. Open questions

- **Inventory crate for auto-registration.** The `#[derive(Component)]` macro could register via the `inventory` crate (link-time collection) or via an explicit `app.register_component::<T>()` call per plugin. Default: explicit. Migrate to `inventory` if call sites multiply.
- **Workload labels for plugins.** Currently MCP defines `McpWorkload` and adds three workloads under it. If many plugins do this the editor's workload visualization may get crowded. Open question for editor UX, not for MCP.
- **`Time::scale` semantics under `FixedUpdate`.** `Time.scale` multiplying `delta` is well-defined for variable-rate `Update`. For `FixedUpdate` accumulator, the scale either changes how fast wall-clock time accumulates or it changes how many fixed steps run per frame. The latter is more useful for agent scenarios; documented as such, implemented in `apply_control`.

## 12. The agent skill

A SKILL.md describing how to use this MCP from an agent's perspective is reproduced below. Drop it at `.claude/skills/spark-mcp/SKILL.md` in the project root once the plugin lands; Claude Code auto-discovers it when working in the Spark repo.

The skill covers: when to use MCP versus reading source code or running `cargo`; standard workflows (discover schema, query state, mutate, step, observe); common pitfalls (frame budget, paused render, JSON shapes); and how to map an agent's higher-level intent to a sequence of tool calls.

Outer fence is four backticks because the skill itself contains three-backtick code blocks. Copy everything between the outer fences verbatim into `SKILL.md`.

````markdown
---
name: spark-mcp
description: Use this skill when working on the Spark game engine (https://github.com/AlexTiTanium/spark) and the running game is reachable via the Spark MCP plugin. Trigger any time the user wants to inspect or modify live ECS state, debug a misbehaving system, pause/step the game, capture screenshots tied to specific frames, search runtime logs, or set up automated scenarios. Also trigger when the user mentions any of the `spark.*` tool names (e.g. `spark.world.query`, `spark.control`, `spark.schema`) or talks about debugging the live game versus editing code. Do NOT use this skill for static code review, build problems, or anything that does not require the game to be running.
---

# Spark MCP — Agent Skill

This skill teaches you how to drive a running Spark game through the `spark.*` MCP tools. The plugin runs inside the game process in debug builds and binds to `http://127.0.0.1:9123/mcp`. If `spark.*` tools are not in your tool list, the game is not running or the plugin is not enabled — fall back to reading source code.

## When to use which tool

**Read source first, MCP second.** The Spark repo is your schema. Before calling `spark.world.query` you usually need to know the component type names (e.g. `spark::transform::Position`, not `Position`). Get those from one of three places:

1. The output of `spark.schema` — gives you every registered component name plus a `source_path` you can open. Use this first when you do not know what is loaded.
2. Searching the repo for `#[derive(Component)]` if MCP is unreachable.
3. The user — when they reference a component by short name, ask which fully-qualified path they mean.

**Use MCP when state matters now.** If the question is "what is the game doing right now?" — query. If the question is "what does this code do?" — read source. Never use MCP to answer questions that have static answers; it is slower than reading the file and it consumes a frame.

## Standard workflows

### Discover what is in the world

```
spark.schema                              # what types exist; what file each is in
spark.world.query { with: ["spark::power::Plant"], limit: 5 }
                                          # any plants? what entities?
spark.world.get { entity: 4294967297 }    # all components on that entity
```

### Mutate a specific entity

```
spark.world.query { with: ["spark::power::Plant"], fetch: ["spark::power::Plant"], limit: 50 }
# Find the entity you want.
spark.world.insert {
    entity: <id>,
    components: { "spark::power::Plant": { "kind": "WaterWheel", "output_mw": 5.0 } }
}
```

If `spark.world.insert` returns an `isError: true` reply, the JSON shape was wrong. The error message is from `serde` and points at the exact field. Open the source file (`source_path` from `spark.schema`) and re-read the struct.

### Reproduce a bug deterministically

```
spark.control { action: "pause" }
spark.world.spawn { components: { "spark::transform::Position": [10.0, 5.0],
                                  "spark::power::Plant":        { "kind": "Coal", "output_mw": 8.0 } } }
spark.control { action: "step", n: 10 }   # advances exactly 10 fixed-update ticks
spark.world.query { with: ["spark::power::Plant"], fetch: ["spark::power::Plant"] }
                                          # see what happened
spark.screenshot                          # png saved to target/spark-mcp/screenshots/<frame>.png
```

Pause + step + query is your main debugging loop. Each response carries a `frame` field; you can correlate screenshots, logs, and queries by frame number with zero ambiguity.

### Investigate a warning in the logs

```
spark.logs.search { query: "no fuel", level_min: "warn", limit: 20 }
# returns LogLine entries with frame, level, target, msg.
spark.world.query { with: ["spark::power::Plant", "spark::power::Fuel"] }
# look at the world state that produced the warning.
```

For continuous monitoring, ask the user to keep an SSE connection open to `GET http://127.0.0.1:9123/logs`. You cannot subscribe to SSE directly from an MCP tool — but the user can pipe it to a file you read.

### Verify a code change

After editing source and the user runs `cargo run`:

```
spark.frame_stats                         # confirm game is alive, FPS OK
spark.world.query { with: ["<the component you changed>"], fetch: [...] }
                                          # confirm new field is present
spark.control { action: "step", n: 30 }
spark.screenshot                          # visual regression
```

## Pitfalls

- **Component type names are fully qualified.** Use `spark::transform::Position`, never `Position`. `spark.schema` gives you the exact strings.
- **Entity ids are `u64`** (Spark packs index + generation into one number). `entity.to_bits()` if you have an `Entity`. Never invent ids — get them from `spark.world.query` or a previous `spark.world.spawn` response.
- **The game keeps rendering while paused.** Screenshots work in paused mode. Only `FixedUpdate` and `Update` schedules are skipped — `Render` always runs.
- **Bulk mutations are slow.** The plugin caps at 256 tool calls per frame. If you need to spawn 1000 entities, send 1000 small `spark.world.spawn` calls and accept the wait, or pause + step pattern to make it deterministic.
- **`spark.resource.set` may be overwritten.** Some Resources (e.g. `InputState`) are refilled every frame by a system. Setting them only sticks if no other system writes them on the same or later frames. Read the source to check.
- **Mutations are not transactional.** If you call `spark.world.insert` with three components and the second one fails JSON deserialization, the first one is already applied. The reply carries the failure of the second. Read the error message, fix, retry.

## Frame-deterministic debugging

Every reply has this prefix:

```json
{ "frame": 1234, "at_ms": 1716397842123, "data": {...} }
```

Use `frame` to correlate:
- A screenshot taken at `frame: 1230` shows the world *before* a warning logged at `frame: 1232`.
- A `spark.world.query` reply at `frame: 1232` is the world state *at* the warning.
- A `spark.world.query` reply at `frame: 1234` is the world state two ticks later.

Always mention frame numbers in your reasoning when comparing observations. Wall-clock time is unreliable for fast events; frame numbers are exact.

## How to map user intent to tool calls

**"Why is the city losing power?"**
1. `spark.world.query { with: ["spark::city::City"], fetch: ["spark::city::City"] }` → see demand
2. `spark.world.query { with: ["spark::power::Plant"], fetch: ["spark::power::Plant"] }` → see supply
3. `spark.resource.get { name: "spark::power::PowerNetwork" }` → see ratio
4. `spark.logs.search { query: "blackout" }` → check for events
5. Summarize the chain to the user, citing frame numbers.

**"Test that the worker AI picks up jobs."**
1. `spark.control { action: "pause" }`
2. `spark.world.spawn` a worker and a job at known positions.
3. `spark.world.get` to record initial state.
4. `spark.control { action: "step", n: 60 }` (one second of sim).
5. `spark.world.get` again. Confirm worker has the job assigned.
6. `spark.screenshot` for the user.

**"Show me what's on screen right now."**
1. `spark.screenshot` — done. The reply has `path`; open it.

**"Speed up time so I can see the colony grow over an hour of game time."**
1. `spark.control { action: "scale", scale: 100.0 }`
2. Periodically `spark.frame_stats` to confirm it's running.
3. `spark.screenshot` at intervals.
4. `spark.control { action: "scale", scale: 1.0 }` when done.

## When not to use this skill

- The user asks about a code structure that exists in source — read the file.
- The build is failing — run `cargo`, fix the code.
- The user wants to add a new feature — write code first; debug with MCP after.
- The game is not running — say so, ask the user to start it.

## Quick reference

```
spark.world.query   { with, without, fetch, limit }
spark.world.get     { entity, components? }
spark.world.spawn   { components: { type: value, ... } }
spark.world.insert  { entity, components }
spark.world.remove  { entity, components: [type, ...] }
spark.world.despawn { entity }

spark.resource.get  { name }
spark.resource.set  { name, value }

spark.schema                              # list registered types + source paths
spark.control       { action: pause|resume|step|scale, n?, scale? }
spark.screenshot                          # writes target/spark-mcp/screenshots/<frame>.png
spark.logs.tail     { limit }
spark.logs.search   { query, level_min?, limit? }
spark.frame_stats
```

Reply shape (always):

```json
{ "frame": <u64>, "at_ms": <u64>, "data": <method-specific> }
```

On error:

```json
{ "isError": true, "_rpc_code": <i32>, "content": [{ "type": "text", "text": "<message>" }] }
```
````

## 13. Implementation roadmap

| Phase | Scope | Depends on | Estimate |
|---|---|---|---|
| **P1 — Skeleton** | `lib/mcp/` crate, plugin entry, HTTP server, `initialize` + `ping` + `tools/list` round-trip with Claude Desktop | ECS M4, `Time::paused` | ~2 days |
| **P2 — World tools** | `spark.world.*` + `spark.schema` + `spark.resource.*` | `ComponentRegistry` JSON hooks (ECS Stage 7) | ~3 days |
| **P3 — Control + logs** | `spark.control`, `spark.logs.*`, `spark.frame_stats` | `Time::paused`, `Time::scale`, tracing Layer attached | ~2 days |
| **P4 — Screenshots** | `spark.screenshot` | `ScreenshotRequest` in render | ~1 day after render readback lands |
| **P5 — Events** | `spark.event.*`, `EventRegistry`, `/events` SSE | ECS Stage 14 | ~2 days after Stage 14 |
| **P6 — Input** | `spark.input.*` | `spark-input` crate | TBD |
| **P7 — Scenarios** | `spark.scenario` | P5 | ~2 days |

P1 through P4 are MVP. The whole MVP fits in roughly 8 days of work after ECS M4 lands.
