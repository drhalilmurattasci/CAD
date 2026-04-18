# Phase 16 — Enhanced Input System

Phase 13 gave the **editor** a rebindable action registry: `Ctrl+P` toggles Play, `F` frames the selection, and users can remap any menu shortcut. That system lives in `rustforge-editor`. It never runs when the game does. Phase 16 is about the other side: a runtime input stack that ships inside `rustforge-core`, runs on every platform the engine targets, and replaces the ad-hoc "read raw `KeyEvent` in a player script" pattern with something an 80-hour campaign or a competitive netgame can rely on.

The distinction matters because the two systems have different constraints. The editor's keybinder only sees keyboard chords, is allowed to allocate, runs at 60 Hz in a forgiving context, and never needs replay. Game input handles gamepads, mice, pens, and touches; fires at the engine tick rate; must be deterministic for Phase 14 networking; and has to survive a script hot-reload (Phase 7) without the player losing their bindings mid-play. The shape of the API, the data model, and the serialization format are all different.

This phase also takes on a specific architectural invariant: **gameplay code never reads a raw device event**. Scripts, ECS systems, and C-API consumers see `Actions` and `ActionValue`s only. The path from "USB keyboard sends scancode 30" to "the player's script sees `jump.triggered == true`" goes through one pipeline, it's the same pipeline that records a replay, and it's the same pipeline that accessibility features hook into. If anyone in gameplay code grabs raw events, the net layer can't snapshot them, the replay can't reproduce them, and the hold-to-press helper can't transform them.

## Goals

By end of Phase 16:

1. **Action / context model** — named actions grouped into stackable contexts (UI, Gameplay, Vehicle, Menu), with priority-ordered consumption.
2. **Device abstraction** for keyboard, mouse, gamepad (`gilrs`), touch, and pen/stylus, behind a single event trait.
3. **Modifiers & triggers** — press, release, hold, tap, double-tap, chord; composable into combos without hand-rolled timers in gameplay code.
4. **Composite actions** — WASD → `Vec2`, trigger pairs → axis, button combos → virtual button.
5. **`.rinput` asset format** — RON bindings shipped with the game, reloadable, editable in a dedicated panel.
6. **Runtime rebinding UI** with per-player save slots and conflict detection.
7. **Recording & replay** — deterministic capture of the action stream (not device events) for Phase 14 netcode and automated tests.
8. **Accessibility** — hold-to-press, sticky modifiers, single-button mode, input repeat tuning.
9. **IME / text input** hook for chat, name-entry, and script prompts.
10. **Input debug overlay** extending Phase 10 diagnostics — live action state, raw events, binding trace.
11. **Script hot-reload safety** (Phase 7) — rebinding survives a `.wasm` module swap without a player losing their remap.
12. `rustforge-core` with the input subsystem still builds and runs without the `editor` feature.

## 1. The runtime-input invariant

> **Gameplay code reads `Action`s. Only the input subsystem reads devices.**

This is the one rule that makes every other feature in Phase 16 possible. If a script does `if keyboard.just_pressed(Key::Space) { jump() }`, five things break:

- Rebinding stops working for that script.
- The action doesn't appear in replay capture (there is no action).
- Phase 14's net snapshot can't send it across the wire — it's a local-only event.
- Hold-to-press accessibility can't transform it.
- Phase 10 diagnostics can't trace "why did the jump not fire?" through a pipeline the jump didn't go through.

Enforce it with a lint: gameplay crates are not allowed to `use rustforge::input::raw::*`. The `raw` module is `pub(crate)` at the engine level; scripts and game systems see `ActionState`, `Action<T>`, and `InputContext` only. Document this explicitly — it's the one thing new contributors will try to violate first.

## 2. Action and context model

```rust
#[derive(Reflect, Serialize, Deserialize, Hash, Eq, PartialEq, Clone)]
pub struct ActionId(pub Cow<'static, str>);    // "gameplay.jump"

pub enum ActionKind {
    Button,             // bool: pressed / released / held
    Axis1D,             // f32: -1.0..=1.0
    Axis2D,             // Vec2: joystick, WASD, touch drag
    Axis3D,             // Vec3: IMU, 6DoF — rare but cheap to support
}

pub struct ActionMeta {
    pub id:    ActionId,
    pub kind:  ActionKind,
    pub label: String,           // localized through Phase 13 t!()
    pub scope: InputContextId,   // which context owns it
}
```

Contexts are the gameplay equivalent of a "focus group":

```rust
pub struct InputContext {
    pub id:       InputContextId,    // "gameplay", "vehicle", "menu", "ui.modal"
    pub priority: i16,               // higher = consumed first
    pub consume:  ConsumePolicy,     // PassThrough | Consume
    pub actions:  Vec<ActionId>,
}
```

The stack (`Vec<InputContext>`) is ordered by `priority`. When a modal dialog opens, the UI pushes `ui.modal` at priority 1000 with `Consume`; gameplay at priority 10 stops seeing any raw events that the UI context's bindings matched. When the dialog closes, pop it and gameplay resumes. Vehicle entry pushes `vehicle` at 20 and keeps `gameplay` active at 10 for handbrake-while-on-foot-disabled interactions; vehicle's `Consume` policy hides `gameplay.jump` while driving but lets `gameplay.emote` bleed through.

### 2.1 Context stacking ASCII

```
                  device events
                        │
                        ▼
              ┌───────────────────┐
              │  Raw Event Queue  │   (keyboard/mouse/pad/touch/pen)
              └────────┬──────────┘
                       │
         ┌─────────────▼──────────────┐
         │      Context Stack         │
         │                            │
         │  ui.modal     prio 1000 C  │ ── highest
         │  ui.hud       prio 100  P  │
         │  vehicle      prio  20  C  │
         │  gameplay     prio  10  P  │
         │  debug        prio   0  P  │ ── lowest
         └─────────────┬──────────────┘
                       │  consume / pass
                       ▼
              ┌───────────────────┐
              │  Action Resolver  │   triggers, modifiers, composites
              └────────┬──────────┘
                       │
            ┌──────────▼──────────┐
            │   ActionState map   │   read by scripts + ECS + net
            └─────────────────────┘
```

Priorities are explicit numbers, not z-order. A plugin (Phase 11) that wants to intercept input declares a priority; developers can see conflicts by listing the stack in the debug overlay (§9).

## 3. Bindings and composites

A binding is the link from a physical input to an action. The model:

```rust
pub enum Source {
    Key(Key),
    MouseButton(MouseButton),
    MouseMotion,                     // dx, dy
    MouseWheel,
    GamepadButton { pad: PadSlot, button: GamepadButton },
    GamepadAxis   { pad: PadSlot, axis:   GamepadAxis },
    TouchTap,
    TouchDrag,
    PenPressure,
    Composite(Box<Composite>),
}

pub enum Composite {
    Vec2FromKeys { up: Source, down: Source, left: Source, right: Source },
    Axis1DFromKeys { pos: Source, neg: Source },
    TriggerAxis { minus: Source, plus: Source },  // L2/R2 → one axis
    Chord(Vec<Source>),                           // all must be held
}
```

WASD movement is the canonical composite: four keys collapse into a `Vec2` and the gameplay system sees `Action<Vec2>`. If a player rebinds W to T, only the WASD composite changes — the gameplay code is untouched. The same composite machinery handles a gamepad left stick producing the same `Vec2`, so a single action can have two bindings (keyboard composite + stick) and the code reads whichever is active.

Composites compose. A diagonal dash could be `Chord(Shift, Composite(Vec2FromKeys(WASD)))` producing `Action<Vec2>` only while Shift is held.

## 4. Modifiers and triggers

Raw events become action state through a trigger pipeline:

```rust
pub enum Trigger {
    Press,              // 1 frame of "just pressed"
    Release,            // 1 frame of "just released"
    Held,               // every frame while down
    Tap { max_hold: Duration },            // short press, on release
    Hold { min_hold: Duration },           // fires at min_hold, still held
    DoubleTap { within: Duration },        // two presses inside window
    Combo(Vec<Trigger>),                   // ordered sequence
}
```

Trigger evaluation lives inside the resolver and is **stateful per action per context**. This is the one place in gameplay-input where we keep allocations off the hot path: trigger state is a `SmallVec<[TriggerFsm; 2]>` stored next to the action, reset when the binding changes.

Modifiers (`Shift`, `Ctrl`, `Alt`, `Meta`, gamepad L1/R1 etc.) are just additional `Source`s that a `Chord` consumes. There is no separate modifier type — it's all sources and chords. This unifies "Ctrl+Click" (keyboard modifier + mouse click) with "L2+Square" (gamepad shoulder + face button) without special casing.

### 4.1 Timing — tied to the fixed tick

Trigger windows (`Tap::max_hold`, `Hold::min_hold`, `DoubleTap::within`) are expressed in real time but **sampled at the engine fixed step** (Phase 7 §6). A 0.2 s double-tap window is exactly 12 frames at 60 Hz; it is not "12 frames" at 30 Hz. Determinism for replay and net (Phase 14) depends on this.

## 5. Device abstraction

```rust
pub trait InputDevice: Send + Sync {
    fn poll(&mut self, out: &mut RawEventQueue, now: Instant);
    fn name(&self) -> &str;
    fn slot(&self) -> DeviceSlot;   // P1, P2, P3, P4, Pen, Keyboard(0), ...
}
```

Concrete implementations live in `rustforge-core/src/input/devices/`:

```
devices/
├── keyboard.rs       # from winit
├── mouse.rs          # from winit
├── gamepad.rs        # via gilrs (Xinput, DirectInput, evdev, IOKit)
├── touch.rs          # winit touch + synthetic gestures
└── pen.rs            # winit pen events + pressure
```

`gilrs` handles hot-plug, battery level, rumble (rumble is an output — we forward `Rumble` commands, but authoring the haptic curves is out of scope, see §12). Device identity is a `(DeviceKind, usize)` pair, stable across a run; on hot-plug events we emit an `InputDeviceEvent::Connected`/`Disconnected` so the UI can say "Controller 2 disconnected" without the game glitching.

### 5.1 Touch & pen

Touch becomes `TouchTap`, `TouchDrag`, and `TouchPinch` at the composite layer. Multi-touch is explicit (touch ids), not simulated. Pen is treated as a mouse with an extra `pressure: f32` source. Swipe detection is a composite over `TouchDrag` with a direction threshold. Gestures beyond swipes (rotate, complex multi-finger) are out of scope (§12).

## 6. `.rinput` asset-driven bindings

Bindings are data, not code. A game ships a `.rinput` RON file:

```ron
(
    version: 1,
    contexts: [
        (
            id: "gameplay",
            priority: 10,
            consume: PassThrough,
            actions: [
                (
                    id: "gameplay.move",
                    kind: Axis2D,
                    bindings: [
                        Composite(Vec2FromKeys(up: Key(W), down: Key(S), left: Key(A), right: Key(D))),
                        GamepadAxis(pad: P1, axis: LeftStick),
                    ],
                ),
                (
                    id: "gameplay.jump",
                    kind: Button,
                    triggers: [Press],
                    bindings: [ Key(Space), GamepadButton(pad: P1, button: South) ],
                ),
            ],
        ),
    ],
)
```

The asset loads through the Phase 5 asset pipeline; reimport triggers a rebuild of the resolver. Games split bindings across files (`gameplay.rinput`, `ui.rinput`, `vehicle.rinput`) and load each per context. Version field feeds a migration shim when the schema bumps, same pattern as Phase 13 §1.2.

## 7. Runtime rebinding

Separate from Phase 13's editor keybinder. Players rebind from an in-game menu the game itself ships — the engine provides the widget, not the menu.

```rust
pub struct RebindRequest {
    pub action: ActionId,
    pub binding_index: usize,        // action may have multiple bindings
    pub allow_kinds: SourceKindMask, // e.g. "keyboard + mouse" or "gamepad only"
}

pub enum RebindResult {
    Captured(Source),
    Cancelled,
    Conflict { with: ActionId, within: InputContextId },
}
```

- **Capture window:** push a special `rebind` context at priority `i16::MAX` that consumes everything. First non-modifier source received becomes the new binding.
- **Conflict policy:** same context → hard conflict, user must resolve. Different contexts that can't be active simultaneously → allowed, warn only. Two contexts that are both active (e.g. `ui.hud` + `gameplay`) → conflict.
- **Reset-to-default** reloads the shipped `.rinput` values for that action.

### 7.1 Per-player save

Multi-player couch games need per-player remaps. Per-player save lives next to the engine's save-game directory:

```
<save>/input/
├── player_0.rinput-override       # overrides applied on top of defaults
├── player_1.rinput-override
└── shared.rinput-override         # bindings not tied to a player slot
```

Override files only store diffs from the shipped `.rinput`. On load: apply defaults, then apply the player's override on top. This way a shipped rebalance that changes a default binding propagates to players who didn't customize that action, without clobbering ones they did.

## 8. Recording & replay

Phase 14 (netcode) depends on this. Phase 7's determinism test will graduate to using it.

```rust
pub struct InputRecording {
    pub version: u32,
    pub start_tick: u64,
    pub entries: Vec<InputFrame>,   // one per tick
}

pub struct InputFrame {
    pub tick: u64,
    pub actions: SmallVec<[(ActionId, ActionValue); 8]>,
}
```

What to record: **the resolved action stream**, not raw device events. Two reasons:

- Replay survives rebinding. A player records a run with WASD, shares the `.replay`, another player with arrow keys plays it back identically because both pipelines produced the same `gameplay.move` `Vec2`.
- Replay survives device hot-plug differences. You don't need P1's controller in port 1 on playback.

Cost: a controller-heavy game emits ~30 action samples/frame; at 60 Hz that's ~1800 samples/s, a few dozen KB/s compressed. Trivial.

### 8.1 Headless replay

Dropped into a test harness, `InputRecording` drives the input subsystem without any device present: the poll step is replaced by "read the next frame's entries." This is the hook Phase 7's determinism test and Phase 14's client-side prediction both use.

### 8.2 When replay breaks

Replay is only valid against the same `.rinput` schema version and the same set of declared actions. Loading a replay with missing actions: fail fast with a clear error, never silently drop frames.

## 9. Input debug overlay

Phase 10 has a diagnostics overlay. Input gets its own pane inside it:

```
┌─ Input Debug ─────────────────────────────────────────┐
│ Contexts (top = highest priority)                     │
│   ui.modal       prio 1000   Consume     ACTIVE       │
│   gameplay       prio   10   PassThrough ACTIVE       │
│                                                        │
│ Devices                                                │
│   Keyboard(0)   connected                              │
│   Gamepad(P1)   Xbox Wireless, 78% battery             │
│                                                        │
│ Actions                                                │
│   gameplay.move        Vec2(0.42, -0.18)  [stick L]    │
│   gameplay.jump        idle                            │
│   gameplay.reload      triggered (Tap, 142 ms)         │
│                                                        │
│ Last raw event                                         │
│   GamepadAxis(P1, LeftStick) = (0.42, -0.18) @ t+0ms   │
│                                                        │
│ Trace: gameplay.move                                   │
│   GamepadAxis(P1, LeftStick) → Composite(stick L)      │
│   → bound to gameplay.move in context 'gameplay'       │
│   → no earlier context consumed → delivered            │
└────────────────────────────────────────────────────────┘
```

"Trace" is the single most valuable field. When a designer says "jump doesn't work in the vehicle," the trace shows `gameplay.jump` is bound but the `vehicle` context is consuming it — one glance answers the question. This is worth the overlay all on its own.

## 10. Accessibility

Four toggles, each prefs-backed (Phase 13):

1. **Hold-to-press.** Any `Trigger::Hold` becomes `Trigger::Press` after a short confirm window. A player who can't hold a button presses once, presses again to stop. Useful for charge attacks.
2. **Sticky modifiers.** Press Shift, release; next source fires as if Shift were held. Consumed after one use. Same pattern as OS sticky keys but game-scoped so it doesn't fight the OS version.
3. **Single-button mode.** Collapses a chord (`L2+Square`) into a radial menu triggered by the chord root. The engine ships the radial as a reusable UI widget; games opt in per-action.
4. **Input repeat tuning.** `repeat_delay` and `repeat_rate` for `Trigger::Held`. Defaults match OS typematic; overridable per player in save slot.

All four operate at the trigger layer, not the device layer — so replay reproduces them correctly regardless of the recorder's accessibility state. (Record the *resolved* action, remember.)

Dead-zone and curve controls on analog sticks also live here: per-player, saved with the override, applied before composites. A player with a drifting stick sets a 15% dead zone once and never thinks about it again.

## 11. IME and text input

Game chat, player names, script prompts in the play build. IME (Input Method Editor) support is not optional for any CJK, Vietnamese, or complex-script market.

```rust
pub enum TextInputEvent {
    Preedit { text: String, cursor: usize },   // composition in progress
    Commit  { text: String },                  // committed string
}

pub struct TextInputContext {
    pub active: bool,                // enabled when a text field has focus
    pub rect:   Rect,                // where to anchor IME candidate window
    pub multiline: bool,
}
```

When `TextInputContext::active` is true, the input subsystem stops resolving text-entry keys as gameplay bindings and routes them to the IME through winit's text-input hooks. On commit, the text field receives the final string. This is the correct escape hatch — without it, a Japanese player's "input hiragana" composition would collide with WASD.

IME anchoring rect matters: the OS candidate popup should appear near the text cursor, not center-screen.

## 12. Script integration and hot-reload safety

Phase 7 hot-reloads WASM scripts. When a script reloads, its ECS state is migrated or reset — but its **input bindings must not reset**. A player halfway through a game should not suddenly lose their "I rebound aim to L2" preference because the developer tweaked a script and triggered a reimport.

Bindings are owned by the input subsystem, not by the script. A script declares which actions it reads via an attribute:

```rust
#[script]
impl PlayerController {
    #[input(context = "gameplay", read = ["gameplay.move", "gameplay.jump", "gameplay.reload"])]
    fn update(&mut self, inputs: &InputState) { ... }
}
```

On reload:

1. The scripting host (Phase 7 §7) re-registers the script's declared action list with the resolver.
2. If the new version declares new actions, they load with shipped defaults.
3. If it drops actions, their bindings stay in the override file (dormant) and come back automatically if the script re-declares them later.

This is the analog of the "transient, reflected, survives reload" rule Phase 7 established for components, applied to input. Bindings survive by virtue of being data, not code.

## 13. Build order within Phase 16

1. **Action / context / resolver scaffolding** — no devices yet; pump `RawEventQueue` from a unit test.
2. **Keyboard + mouse via winit** — first real devices. Gameplay script reads `gameplay.move` from WASD.
3. **`.rinput` asset format + loader** — replace hardcoded test bindings.
4. **Gamepad via `gilrs`** — hot-plug, P1..P4 slots.
5. **Touch + pen** — as time allows; tablet and mobile targets need them.
6. **Triggers** — Press, Release, Held, Tap, Hold, DoubleTap, Combo.
7. **Composites** — Vec2FromKeys, Axis1DFromKeys, TriggerAxis, Chord.
8. **Rebinding UI widget** — capture, conflict detection, reset-to-default.
9. **Per-player override save** — diff against shipped `.rinput`, load on game start.
10. **Recording + headless replay** — graduate Phase 7's determinism test to use it.
11. **Accessibility toggles** — hold-to-press, sticky modifiers, single-button mode, repeat tuning.
12. **IME text-input route** — winit hooks, preedit/commit events, anchor rect.
13. **Script hot-reload binding preservation** (Phase 7 integration).
14. **Input debug overlay** — extends Phase 10 diagnostics.
15. **Replay-determinism CI test** — record a 60-second run, replay, assert end state matches.

## 14. Scope boundaries — what's NOT in Phase 16

- ❌ **VR/AR / 6DoF controllers.** Deferred to Phase 28 (XR). `Axis3D` exists in the data model so the pipeline doesn't need restructuring, but no XR device implementation ships here.
- ❌ **Haptic authoring.** Rumble forwarding on/off is supported; a timeline editor for designing haptic patterns is a separate phase.
- ❌ **Brain-computer interface.** Not now, probably not ever in a general-purpose engine.
- ❌ **Gestures beyond swipes.** Rotate, pinch-to-zoom with rotation, multi-finger choreographies — future. Pinch is in (it's too common to defer).
- ❌ **Eye / head / hand tracking.** XR-phase work.
- ❌ **Voice commands.** Separate capability, separate phase. Audio input pipeline doesn't exist yet.
- ❌ **Networked input prediction / rollback.** Phase 14 owns that; Phase 16 produces the deterministic action stream Phase 14 consumes.
- ❌ **Cloud-sync of player bindings across devices.** Save files are local; cross-device sync is a platform-integration project.
- ❌ **Adaptive triggers (DualSense trigger effects).** Platform-specific output; Phase 16 covers input only.
- ❌ **Two-chord sequence triggers at the gameplay level** (press A then B within 500 ms). Combo is the single-sequence form; nested chord languages are overkill for gameplay.

## 15. Risks & gotchas

- **Determinism drift from wall-clock sampling.** Trigger windows measured in `Instant::now()` break replay. Always sample against the fixed-tick clock, never real time. Regression-test this once and put the assertion in CI forever.
- **Context-stack leaks.** A panel pushes a context and its teardown path forgets to pop on an error. The game enters a state where nothing responds. Use RAII: `ContextGuard` pops on drop; pushing by value is forbidden outside the guard constructor.
- **Gamepad hot-plug races.** `gilrs` emits connect events on a separate thread; the resolver must deduplicate if a pad re-announces itself. Always key on the `(backend_id, uuid)` pair, not just slot index.
- **IME vs gameplay collision.** A text field opens, focus moves, the player is still holding W — the held state must survive the context push, not reset. Test: enter text box while moving; close box; movement resumes without requiring re-press.
- **Composite ambiguity.** `Chord(Shift, A)` and bare `A` both fire on "press A while holding Shift" unless chord consumption is defined. Rule: if a more-specific chord matches, suppress the less-specific base. Document and test explicitly.
- **Rebinding a modifier.** Player binds Shift to "fire." Now no chord involving Shift fires. Refuse to bind pure-modifier keys unless the conflict check is explicitly acknowledged.
- **Replay schema drift.** A developer adds an action mid-project; old recordings don't declare it. Version the recording; when loading a version-N recording into a version-(N+k) build, emit default values for added actions and log.
- **Per-player override stale after rebalance.** Ship v1 with `jump=Space`; v1.1 rebalances to `jump=LShift`. Players with no override get the new default; players with *any* override on `jump` keep their custom binding (correct). Players with no override on `jump` but who rebound `move` are tricky — make sure override files list only actually-changed actions, not a full snapshot.
- **Dead-zone and curve on keyboard-only.** A player playing with only a keyboard sees no "dead zone" setting in the menu because it only applies to sticks. The menu must hide per-device settings when no matching device is connected, not show them greyed-out and confusing.
- **Single-button mode radial blocking gameplay.** Radial is modal — pushes a context. If the player triggers it mid-combat, gameplay is frozen while they choose. That's the intended behavior, but signpost it in the UX: dim the world, don't hide it.
- **Sticky modifier interaction with chord triggers.** Sticky Shift fires chord once then unstickies; if the chord trigger is `Hold`, it fires after `min_hold` and immediately ends because the next frame Shift is un-held. Treat sticky modifiers as held-for-N-frames (N=1 by default; configurable).
- **Touch event debounce.** Cheap touch hardware double-fires. A 10–20 ms debounce at the device layer prevents spurious double-taps from becoming `DoubleTap` triggers.
- **Input starvation under frame drops.** At 10 fps, press-and-release inside one frame is invisible. Device polling must accumulate and replay events in order; never drop raw events because the tick is slow. The queue is bounded — if it overflows, log a loud diagnostic and drop oldest, not newest.

## 16. Exit criteria

Phase 16 is done when all of these are true:

- [ ] Action and context model landed; gameplay scripts read `Action`s, never raw device events.
- [ ] Context stack supports push/pop with RAII, priority ordering, and per-context `Consume` / `PassThrough`.
- [ ] Keyboard, mouse, gamepad (via `gilrs`), touch, and pen are all wired; hot-plug events surface.
- [ ] Triggers (Press, Release, Held, Tap, Hold, DoubleTap, Combo) all land with fixed-tick-deterministic timing.
- [ ] Composites (Vec2FromKeys, Axis1DFromKeys, TriggerAxis, Chord) produce typed `ActionValue`s.
- [ ] `.rinput` RON assets load through the Phase 5 pipeline and live-reimport on edit.
- [ ] Runtime rebinding UI widget captures bindings, detects conflicts, and resets-to-default.
- [ ] Per-player override files persist rebinds as diffs; shipped-default rebalances propagate to un-touched actions.
- [ ] Recording captures the resolved action stream per tick; replay reproduces final game state bit-for-bit.
- [ ] Headless replay harness drives the input subsystem with no devices attached.
- [ ] Hold-to-press, sticky modifiers, single-button mode, and repeat-tuning are prefs-controlled and reflected in replays correctly.
- [ ] IME routing active while text-input contexts are focused; preedit and commit events reach text fields.
- [ ] Script hot-reload (Phase 7) preserves player binding overrides across `.wasm` module swaps.
- [ ] Input debug overlay extends Phase 10 diagnostics with contexts, devices, actions, raw event, and per-action trace.
- [ ] Replay-determinism CI test passes on every PR.
- [ ] `rustforge-core` with input built-in still compiles and runs without the `editor` feature.
