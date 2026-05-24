# spark-input

The Spark engine's input layer. Turns the keyboard and mouse events the
window forwards into two resources you can read from any system —
`KeyboardState` and `MouseState`.

> **State vs events — the one idea to grok.** An *event* is a thing that
> happened ("the W key went down"). *State* is how things are right now ("W
> is held"). `spark-input` consumes the low-level events and keeps the
> running state for you, so your game code asks simple questions —
> `is_pressed`, `just_pressed`, where's the cursor — instead of tracking key
> presses frame by frame itself.

> **Where do the events come from?** `spark-window` translates the OS's
> `winit` events into Spark's own `KeyboardInput` / `MouseButtonInput` /
> `CursorMoved` / `MouseWheel` / `FocusLost` events and forwards them into
> the world. `spark-input` reads them. The two crates never reference each
> other — `spark-window` depends on `spark-input` (to name the event types it
> emits), never the reverse, and `spark-input` has no `winit` dependency at
> all.

## Plug it into the `Application`

Add `InputPlugin`. To actually *produce* input, the window has to be running
too, so register `WindowPlugin` as well (it forwards the events):

```rust
use spark_core::Application;
use spark_input::InputPlugin;

Application::new()
    .add_plugin(InputPlugin)
    // .add_plugin(spark_window::WindowPlugin::default()) // forwards OS input
    .run()
    .unwrap();
```

`InputPlugin` inserts `KeyboardState` and `MouseState` and registers its
collection systems on `Stage::Input` — the first stage each frame — so by the
time `Update` runs, the state already reflects this frame's input.

## Using it from the game (`src/`)

Read the state in a system with `Res<KeyboardState>` / `Res<MouseState>`. Keys
are physical positions (`KeyCode::KeyW` is the same key on any layout):

```rust
use spark_core::{Application, Stage};
use spark_ecs::Res;
use spark_input::{InputPlugin, KeyboardState, MouseState, KeyCode, MouseButton};

fn control_player(keys: Res<KeyboardState>, mouse: Res<MouseState>) {
    // Held — true every frame the key is down. Use for continuous movement.
    if keys.is_pressed(KeyCode::KeyW) {
        // walk forward
    }

    // Edge — true only on the frame the key went down. Use for one-shot actions.
    if keys.just_pressed(KeyCode::Space) {
        // jump
    }

    // Mouse: held buttons, absolute cursor, per-frame scroll.
    if mouse.is_pressed(MouseButton::Left) {
        let (x, y) = mouse.position();
        let _ = (x, y);
    }
    let (_sx, scroll_y) = mouse.scroll();
    let _ = scroll_y; // zoom, etc.
}

let mut app = Application::new();
app.add_plugin(InputPlugin)
   .add_system(Stage::Update, control_player);
```

The headline accessors:

| Method | Answers |
|-|-|
| `keys.is_pressed(KeyCode)` | Is this key held *right now*? |
| `keys.just_pressed(KeyCode)` | Did it go down *this frame*? |
| `keys.just_released(KeyCode)` | Did it go up *this frame*? |
| `mouse.is_pressed / just_pressed / just_released(MouseButton)` | Same, for buttons. |
| `mouse.position()` | Cursor in window pixels, top-left origin. |
| `mouse.scroll()` | This frame's scroll delta in pixels (resets each frame). |

## Using it from an engine crate (`lib/*`)

`spark-input` sits *below* `spark-window` in the dependency graph: the window
depends on this crate to name the events it emits. So a crate that produces
input (a future gamepad backend, say) does the same thing the window does —
it depends on `spark-input` and sends the event types into the world:

```toml
[dependencies]
spark-input = { path = "../input" }
spark-ecs = { path = "../ecs" }   # for Events<T> / the send path
```

A crate that *consumes* input just reads the resources via `Res<KeyboardState>`
/ `Res<MouseState>`, exactly like game code.

## How input flows each frame

```text
OS ─winit─▶ spark-window ──KeyboardInput / MouseButtonInput / CursorMoved
                          │  / MouseWheel / FocusLost  (forwarded as events)
                          ▼
Stage::Input:  swap_events::<T>   (rotate last frame's events in)
               collect_keyboard / collect_mouse_buttons / collect_mouse_motion
                          ▼
later stages:  Res<KeyboardState>.is_pressed(..)   // zero latency
```

The swap runs before the collectors (guaranteed by `InputPlugin`), so a key
pressed just before a frame is readable *that* frame.

## Errors / pitfalls

- **`just_pressed` is one frame only.** It's `true` on the frame the press
  arrived and `false` after. Poll it every frame; don't expect it to latch.
- **`scroll()` resets each frame.** It's a delta, not a running total —
  `(0, 0)` on frames with no wheel movement.
- **`position()` survives focus loss; held keys/buttons don't.** When the
  window loses focus (Alt-Tab), every held key and button is released (and
  reported via `just_released`) so nothing gets stuck — but the cursor keeps
  its last known position.
- **Some keys are dropped.** `KeyCode` is a curated subset; keys outside it
  never produce an event. To add one, extend `KeyCode` and the window's
  `map_key` (see the `KeyCode` docs).
- **No window, no input.** `InputPlugin` alone wires the state up but produces
  nothing; it's `WindowPlugin` that forwards real events. (In tests you can
  feed synthetic events directly — see this crate's `collect` tests.)
