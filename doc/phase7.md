# Phase 7 — Play-in-Editor (PIE)

Until now, the editor has been a pure authoring tool: you edit a scene, save it, and a separate `cargo run` of your game loads it. That's a slow iteration loop. Phase 7 adds **Play-in-Editor**: hit Play, the engine runs your scene with scripts and physics active, right inside the editor. Hit Stop, the scene returns exactly to its pre-play state.

This is the feature that separates "scene editor" from "game engine editor".

## Goals

By end of Phase 7:

1. **Play / Pause / Step / Stop** controls in the toolbar, driving engine tick state.
2. **Full state snapshot** taken on entering play mode; exact restoration on exit.
3. **Script hot-reload** — edit a `.rs`/WASM script, recompile, running play session picks it up without restart.
4. **Play-mode semantics**: scene edits during play don't persist to disk; some edits (transform tweaks via gizmo) can be allowed as live-tuning but reverted on stop.
5. **Command stack integration**: play-mode freeze as designed in Phase 6.

## 1. The fundamental invariant

> **Stopping play mode must return the scene to a byte-identical state.**

This is the feature's entire value proposition. Users test gameplay freely because there's zero cost — nothing they did in play mode survives. If this invariant ever fails (even once), users stop trusting Play, save obsessively before pressing it, and PIE becomes worthless.

Every other design decision in Phase 7 serves this invariant.

## 2. Snapshot/restore architecture

Two viable strategies:

### Strategy A: Serialize/deserialize via Phase 4 scene format

On Play: `SceneFile::from_world(&world)` → hold in memory. On Stop: clear world, `SceneFile::into_world()` restores.

**Pros:** reuses existing code; guaranteed-correct round-trip (already tested).
**Cons:** serialization isn't instant for large scenes; non-reflected state is lost (but it was never part of the scene anyway).

### Strategy B: ECS-native clone

Walk every entity, clone every component via reflection vtable, store in a shadow `World`. Restore by swapping worlds.

**Pros:** faster, avoids format round-trip.
**Cons:** more code; must handle non-`Clone` components somehow; harder to test.

### Recommendation: **Strategy A**

You already have scene serialization. It's tested. Its performance is fine for realistic scenes (<1 second even at 10k entities). Phase 7 is complex enough without writing a second clone-the-world path that can drift from the first.

If serialization becomes a bottleneck later, upgrade to Strategy B — the public API (`begin_play()` / `end_play()`) doesn't change.

```
crates/rustforge-editor/src/play_mode/
├── mod.rs                # PlayMode state machine
├── snapshot.rs           # snapshot/restore via SceneFile
├── controls.rs           # toolbar widget, hotkeys
└── session.rs            # PlaySession: active run state, timing, step
```

## 3. Play mode state machine

```rust
pub enum PlayState {
    Edit,       // default; full editing; engine tick disabled (or renders only)
    Playing,    // engine ticking; edits allowed but revertible
    Paused,     // engine NOT ticking; edits allowed but revertible
}

pub struct PlayMode {
    state: PlayState,
    snapshot: Option<SceneFile>,     // holds original scene while in Playing/Paused
    session_time: Duration,          // elapsed play time
    step_pending: bool,              // one-shot "advance one frame" request
}
```

Transitions:

```
Edit ─play──▶ Playing ─pause──▶ Paused ─play──▶ Playing
 ▲             │                   │              │
 │             └─stop──────────────┴──────────────┘
 │                             │
 └─────────────────────────────┘
                (restores snapshot)
```

- **Edit → Playing:** take snapshot, start ticking, command stack freezes.
- **Playing → Paused:** stop ticking; snapshot still held; command stack stays frozen.
- **Paused → Playing:** resume ticking.
- **Playing/Paused → Edit:** restore snapshot, clear it, unfreeze command stack.
- **Paused + Step:** advance engine by exactly one frame, stay paused.

Stop is idempotent. Playing → Edit is the same path as Paused → Edit.

## 4. Toolbar controls

```
┌─ Toolbar ──────────────────────────────────────────┐
│  [▶ Play]  [⏸ Pause]  [⏭ Step]  [⏹ Stop]   00:12.4 │
└─────────────────────────────────────────────────────┘
```

States:
- **Edit:** only Play is enabled.
- **Playing:** Pause + Stop enabled.
- **Paused:** Play (resume), Step, Stop enabled.

Keyboard:
- `Ctrl+P` — toggle Play/Edit (start or stop).
- `Ctrl+Shift+P` — toggle Pause while playing.
- `F10` — Step (only meaningful while paused).

Visual affordance: while in Playing or Paused, color the viewport border (orange for playing, blue for paused). This is the single most important UX detail — users must *instantly* know they're in play mode. Otherwise they'll lose work thinking they're editing.

Consider also: tint the entire window chrome, or add a large "PLAY MODE" banner strip above the viewport. Unity's is surprisingly subtle; Unreal's is more prominent. Prefer prominent.

## 5. What happens to the editor during play

### Viewport
Keeps rendering. Gizmos still work. Selection still works. Camera fly-through still works. This is important — being able to inspect a running game is the whole point.

### Inspector
Editable. Changes apply to the running entity live. On Stop → reverted by snapshot restore.

### Hierarchy
Readable. Structural changes (spawn, despawn, reparent) work and are reverted on Stop. **Exception:** consider blocking disk operations (Create Prefab from entity) during play — confusing semantics.

### Content Browser
Fully functional. Importing new assets during play is fine. Dragging a mesh into the viewport spawns it (reverted on Stop). Editing an asset source file during play triggers hot-reload (see §7).

### Command stack
**Frozen.** No undo entries added during play. On Stop, stack is unchanged (not cleared) — user returns to their pre-play undo history intact. Ctrl+Z during play either does nothing or shows a toast "Can't undo in play mode."

This is the Phase 6 `CommandStack::set_enabled(false)` hook being used.

### Save
Ctrl+S during play is ambiguous. Two options:
- **Block it:** show "Stop play mode to save." Simplest.
- **Save the snapshot:** writes the pre-play state to disk.

Go with block. The second option sounds clever but introduces a mental model where "what's on disk" differs from "what's in the viewport," which is exactly the confusion PIE needs to avoid.

## 6. Engine tick integration

Phase 2 gave the engine a `tick(dt)` API. Phase 7 consumes it:

```rust
// Editor frame loop
match play_mode.state {
    PlayState::Edit => {
        // No tick. Just render for viewport.
        engine.render(&viewport_target);
    }
    PlayState::Playing => {
        let dt = clock.delta();
        engine.tick(dt);
        engine.render(&viewport_target);
        play_mode.session_time += dt;
    }
    PlayState::Paused => {
        if play_mode.step_pending {
            engine.tick(FIXED_STEP);
            play_mode.session_time += FIXED_STEP;
            play_mode.step_pending = false;
        }
        engine.render(&viewport_target);
    }
}
```

### 6.1 Time scaling (nice-to-have)

Some editors expose a time-scale slider (0.1x ... 4x) for gameplay debugging. Trivial to add — multiply `dt` in the Playing branch. Worth including in Phase 7.

### 6.2 What actually ticks

In Edit mode, some systems should still run so the viewport isn't dead:
- Transform propagation, skinning, rendering — always.
- Physics, scripts, animation playback, audio — only in Playing.

The engine needs a concept of "editor tick" vs "play tick". Two options:

- **Feature flag per system:** `#[run_in(Edit | Play)]` attribute.
- **Two update schedules:** `engine.tick_edit(dt)` and `engine.tick_play(dt)`.

Two schedules is cleaner — explicit at the call site. Add to `rustforge-core`:

```rust
impl Engine {
    pub fn tick_edit(&mut self, dt: f32);   // runs subset
    pub fn tick_play(&mut self, dt: f32);   // runs all systems
    pub fn render(&mut self, target: &RenderTarget);
}
```

## 7. Script hot-reload

RustForge uses WASM scripting. Play mode is where hot-reload matters most — otherwise you're stop-play-start-play for every script tweak.

### 7.1 Mechanism

Already partially in place (you've built a WASM scripting host). For Phase 7, extend it:

```
crates/rustforge-core/src/scripting/
├── host.rs               # (existing) WASM host
├── reload.rs             # NEW — hot reload, state migration
└── registry.rs           # script instance tracking
```

Flow on `.wasm` reimport event:
1. Asset registry fires `AssetEvent::Reimported(script_guid)`.
2. Scripting host iterates all live instances using that module.
3. For each instance: serialize its state (if the script defines a `serialize_state`/`deserialize_state` export), drop the old instance, instantiate from the new module, deserialize state back.
4. If no state hooks: drop and re-instantiate with defaults; log a warning.

### 7.2 Reload during pause vs during play

- **Paused:** safe, no frame in flight. Reload immediately.
- **Playing:** defer reload to end of current frame. Never swap modules mid-tick.
- **Edit:** reload silently in background.

### 7.3 Reload failures

Script fails to compile → reimport fails → editor shows error in console, old module keeps running. Never crash the editor because a user script has a syntax error. This is table stakes for a reload system that feels trustworthy.

### 7.4 Scope

Full state migration across script shape changes (add a new field to a `#[derive(Script)]` struct) is hard. Phase 7 minimum: reload works when script logic changes but state shape doesn't. Shape changes reset state to defaults with a warning. Structural migration can come later.

## 8. Live edit during play — the judgment call

Editing an entity while the game runs is powerful. It's also confusing. Some policies:

### Policy A: Full live edit (Unity-default)
Any change in inspector/gizmo/hierarchy applies to running instances. Reverted on Stop.

### Policy B: No live edit (Unreal-default)
Inspector is read-only during play. Must stop to edit.

### Policy C: Live edit with visual marker (recommended)
Changes apply. Inspector shows a small "🔴 runtime override" badge next to modified fields. Stop makes the badge and the change go away.

Policy C keeps the power of A while giving the user a visual reminder that their edit is ephemeral. The badge is cheap — reuse the dirty-field tracking infra from Phase 6.

**Decision point for Phase 7:** pick Policy C or simplify to A. Either is defensible; B is too restrictive for a modern editor.

## 9. Edge cases — the ones that will bite

- **Prefab instances spawned during play.** User drags a prefab into the viewport mid-play. Gets reverted on Stop, because the snapshot predates it. Correct, but potentially surprising. The visual playmode banner should cover this.
- **Assets reimported during play.** Handled by §7 hot-reload for scripts. For meshes/textures, runtime asset cache invalidates and the viewport updates live. This works today — Phase 5's reimport path doesn't care about play state.
- **Physics determinism.** If you Play → Stop → Play with the same inputs, do you get the same simulation? Only if Rapier's world is fully reset, which snapshot restore should do (it throws away the whole world). Verify with a test: seed input script, run 100 frames, snapshot, reset, re-run — compare final state.
- **Unsaved scene + Play.** User has unsaved edits, presses Play. Edits should not be lost. Snapshot captures current world state, not disk state. On Stop, they're back in their unsaved-but-edited state exactly as before. **Don't auto-save before Play** — that would be destructive.
- **Play with no scene open.** Disable Play button.
- **Crash in play mode.** A script panics during a tick. Ideally: catch (if WASM host supports it), log, stop play mode gracefully. Worst case: editor crashes. Phase 7 should at least log script panics with entity context.
- **Entity references across snapshot boundary.** During play, user selects an entity that was spawned at runtime (e.g., a bullet). On Stop, that entity is gone. `SelectionSet` needs to purge dead entities on restore. Easy to forget.
- **Camera position.** Does the editor viewport camera reset on Stop? No — it's editor state, not scene state. Viewport camera is orthogonal to game camera.
- **Gizmos during play.** Still work. User can grab the player entity and yank them around mid-play. Delightful for debugging, reverts on Stop. This is the Live-Edit Policy C payoff.

## 10. Build order within Phase 7

1. **`PlayMode` state machine** — no engine integration yet. Unit test all transitions.
2. **Toolbar widget + hotkeys** — Play/Pause/Step/Stop buttons that just log state changes. Verify UX before wiring to anything.
3. **Engine tick split** — `tick_edit` vs `tick_play` in `rustforge-core`. Audit each system, tag which mode it runs in.
4. **Snapshot/restore via SceneFile** — entering play serializes world, stopping restores. Test: modify world during play, verify byte-identical restore.
5. **Command stack freeze** — wire `PlayMode` into Phase 6's `CommandStack::set_enabled`.
6. **Viewport playmode banner** — visual indicator (colored border + small badge).
7. **Selection purge on restore** — drop dead entities from `SelectionSet`.
8. **Live-edit policy (C)** — runtime-override badge in inspector.
9. **Step** — add single-frame advance for paused mode. Fixed timestep (`1/60s`).
10. **Script hot-reload** — reimport event → scripting host swaps instances. Defer during play, immediate when paused.
11. **Time scale slider** — nice-to-have. Probably 30 minutes of work.
12. **Determinism test** — scripted input, play → stop → play, compare final state.
13. **Pre-save block during play** — Ctrl+S during play shows a warning toast.

## 11. Scope boundaries — what's NOT in Phase 7

- ❌ Multiple simultaneous play sessions (Unreal's PIE multi-window for multiplayer testing). Single session only.
- ❌ Networked play (dedicated server simulation). Single-process, single-world.
- ❌ Play-time-only components (Unreal's "Simulate" vs "Play"). One play mode.
- ❌ Deep script state migration across shape changes. Reset-to-defaults with warning is fine.
- ❌ Replay / record-and-playback of play sessions. Useful feature, separate phase.
- ❌ Playtest analytics / telemetry.
- ❌ Frame-by-frame debugger (à la Unreal's Rewind). Step is enough for Phase 7.
- ❌ Profiler panel integration (that's Phase 8+, the specialized editors phase).

## 12. Risks & gotchas

- **Snapshot cost on large scenes.** Serializing a 50k-entity scene every time the user presses Play is slow. Measure early. If it hurts, move to Strategy B (in-memory clone) — but only if measured.
- **Runtime-spawned entities reference other runtime-spawned entities.** A bullet's `EntityRef` to its shooter. On snapshot restore, both are gone — fine. But during play, if the user selects the bullet, the inspector queries the shooter ref, and so on — normal behavior. No special handling needed; just verify.
- **Non-serializable state lost on restore.** Anything held in a `NonReflect` component, a global resource, or editor-side caches evaporates on Stop. This is by design but can surprise. Document: *only reflected components survive snapshot/restore*. Anything else is assumed transient.
- **Play-mode rendering differences.** Shadows, volumetrics, SSR — do they look identical in Edit and Play? They should, since rendering is the same code. But if the engine has any `#[cfg(play_mode)]` paths, bugs live there. Audit.
- **GPU resource leak on repeated Play/Stop.** Create 10k entities during play, Stop, Play, Stop, repeat. Each cycle despawns them; are GPU buffers / textures associated with them actually freed? Phase 7 is a good time to stress-test this — memory leaks here compound quickly.
- **Audio continues after Stop.** A sound triggered during play keeps playing after snapshot restore because the audio mixer isn't part of the ECS. Explicit: `audio.stop_all_sources()` on Stop.
- **Physics contacts persisting.** Rapier maintains contact state across frames. Restore must reinit Rapier cleanly. If using a world-reset approach this is free; if you're more clever, this bites.
- **Script hot-reload race with tick.** Swap a module exactly as the tick starts calling into it → crash. The "defer to end of frame" rule prevents this if followed strictly.
- **Time-scale breaking physics.** Running physics at 0.1x might be fine; at 10x probably explodes due to large-step instability. Cap time-scale or warn when set high.
- **User hits Stop rapidly during heavy tick.** Snapshot restore takes 0.5s, editor appears frozen. Should be OK but verify no double-restore.

## 13. Exit criteria

Phase 7 is done when all of these are true:

- [ ] Toolbar has Play / Pause / Step / Stop buttons that enable/disable correctly per state.
- [ ] Ctrl+P toggles play mode; Ctrl+Shift+P toggles pause; F10 steps one frame.
- [ ] Viewport shows a prominent visual indicator (border/banner) when in Playing or Paused state.
- [ ] Pressing Play from Edit takes a snapshot, starts ticking scripts and physics.
- [ ] Pressing Stop restores the pre-play scene byte-for-byte (verified with round-trip test).
- [ ] Pressing Pause stops ticking but keeps the scene in play state; Step advances one frame.
- [ ] Inspector, Hierarchy, and Gizmos remain usable during play; changes apply live; Stop reverts.
- [ ] Fields modified during play show a runtime-override badge.
- [ ] Command stack is frozen during play; Stop leaves the pre-play undo history intact.
- [ ] Selection is purged of dead entities on Stop (no dangling-ref crashes).
- [ ] Editing a `.wasm` script during play reloads the script at end-of-frame; errors don't crash the editor.
- [ ] Ctrl+S during play is blocked with a clear message.
- [ ] Determinism test: play → stop → play with scripted input produces identical final state.
- [ ] 100 Play/Stop cycles in a row don't leak memory or GPU resources.
- [ ] `rustforge-core` still builds without the `editor` feature.
