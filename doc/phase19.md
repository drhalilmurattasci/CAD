# Phase 19 — Timeline, Sequencer & Keyframe Animation

Phase 8 shipped animation *preview* — scrub, play, pause, confirm the import. It explicitly deferred keyframe authoring (§4.2) on the grounds that keyframe editing is a full authoring tool, not a preview. Phase 19 closes that deferral. It also answers a different question that has been implicit since Phase 7: how do you script cinematic moments in a scene without writing a WASM script for every cut? The answer — borrowed in spirit from Unreal's Sequencer and Persona — is a timeline asset that can drive either a skeleton (animation clip) or a whole scene (sequencer), with a shared keyframe + curve editor underneath.

The same `.rtimeline` asset type serves both roles. An animation clip is a timeline bound to a skeleton. A sequencer cutscene is a timeline bound to entities, reflected properties, subsequences, and camera cuts. One data model, one editor, two framings.

## Goals

By end of Phase 19:

1. `.rtimeline` asset (RON) with tracks, keyframes, curves, markers — shared between sequencer and clip authoring.
2. Track types: transform, reflected-property, animation-clip, audio, event, subsequence, camera cut.
3. Sequencer panel: multi-track view, playhead scrubber, zoom, named markers, region loop.
4. Keyframe + curve editor: tangent modes (auto / linear / stepped / bezier), multi-curve selection, onion skin.
5. Animation clip = timeline bound to a skeleton; retarget via bone map + basic IK adjust.
6. Sequencer drives the world through an *override map*, not a world snapshot. Restored on Stop/eject exactly like Phase 7's snapshot restore.
7. Events fire at timestamps into WASM scripts.
8. Take Recorder: auto-keyframe reflected-field changes during PIE.
9. Every edit flows through Phase 6's command stack; scrubs are transactions.
10. Camera-cut track overrides the active camera.
11. PNG-sequence export in-tree; video export via sidecar ffmpeg.

## 1. The `.rtimeline` asset

One format, RON, human-diffable. Skeletal clips and cutscenes only differ in what they bind to.

```
crates/rustforge-core/src/timeline/
├── mod.rs                 # TimelineAsset, TimelinePlayer
├── track.rs               # Track enum + TrackData variants
├── curve.rs               # Curve<T>, Keyframe<T>, TangentMode
├── marker.rs              # Marker { time, label, color }
├── binding.rs             # TrackBinding: entity / skeleton / asset ref
├── eval.rs                # sample(t) -> OverrideMap
└── event.rs               # EventTrack + dispatch
```

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct TimelineAsset {
    pub duration: f32,           // seconds
    pub frame_rate: f32,         // authoring rate, not enforced at runtime
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
    pub loop_range: Option<(f32, f32)>,
}

#[derive(Reflect, Serialize, Deserialize)]
pub enum Track {
    Transform   { binding: TrackBinding, pos: Curve<Vec3>, rot: Curve<Quat>, scale: Curve<Vec3> },
    Property    { binding: TrackBinding, field: FieldPath, curve: CurveAny },
    AnimClip    { binding: TrackBinding, clip: AssetRef<TimelineAsset>, start: f32, scale: f32 },
    Audio       { binding: TrackBinding, source: AssetRef<AudioClip>, start: f32, gain: f32 },
    Event       { times: Vec<(f32, String)> },       // script dispatch
    Subsequence { timeline: AssetRef<TimelineAsset>, start: f32, scale: f32 },
    CameraCut   { entries: Vec<(f32, TrackBinding)> }, // piecewise-constant
}
```

`CurveAny` is a discriminated union over `Curve<f32> | Curve<Vec2> | Curve<Vec3> | Curve<Quat> | Curve<Color> | Curve<bool>`. Anything more exotic is rejected at binding time — keep the surface narrow.

`TrackBinding` resolves via Phase 4's `SceneId`, not `Entity`. A sequencer referencing entity 42 must survive a Stop/Play cycle (which recycles `Entity` handles under hecs). Same discipline as Phase 6 §13.

## 2. Curves and tangents

Keyframes carry value + in/out tangents + a mode. Evaluation is a cubic Hermite for bezier, a lerp for linear, a left-hold for stepped, and a Catmull-Rom-ish computed tangent for auto.

```rust
#[derive(Reflect, Serialize, Deserialize, Clone)]
pub enum TangentMode { Auto, Linear, Stepped, Bezier }

#[derive(Reflect, Serialize, Deserialize, Clone)]
pub struct Keyframe<T> {
    pub time: f32,
    pub value: T,
    pub in_tan: T,
    pub out_tan: T,
    pub mode: TangentMode,
}

#[derive(Reflect, Serialize, Deserialize, Clone)]
pub struct Curve<T> { pub keys: Vec<Keyframe<T>> }  // sorted by time
```

Quaternions are stored un-normalized per key; evaluation slerps along shortest arc between neighbors and re-normalizes. Tangents on `Quat` are treated as angular velocity in the local frame — fine for 99% of cases, and the 1% wants a raw euler track anyway.

Opinion: **no hand-authored "step" tangents on floats in the default mode.** Auto should produce usably smooth curves out of the box. Step is opt-in per key. Blender's default of linear everywhere is wrong for character work; Maya's Auto is better.

## 3. Sequencer panel

```
crates/rustforge-editor/src/panels/sequencer/
├── mod.rs                 # SequencerPanel
├── ruler.rs               # time ruler, frame snapping
├── tracks.rs              # left pane: track list, folds, mute/solo
├── timeline_view.rs       # right pane: bars, keyframes, clip ranges
├── markers.rs             # marker overlays
├── playback.rs            # transport controls, loop, rate
└── drag.rs                # drag-to-move keys, rubber-band select
```

Panel layout:

```
+- Sequencer: intro_cutscene.rtimeline -----------------------------+
| [<] [>] [> ] [O]  00:02.41 / 00:08.00  [1.0x v]  [M][S]          |
|                                                                    |
|  Tracks                |0s       1s       2s       3s       4s    |
| +----------------------+------v-----------v--------------------+  |
| | > Hero (Transform)   | *----*---*------o*---*                |  |
| |   > Pos.x            | ~~~~^~~~~~~~~~/~~~~~~~                |  |
| |   > Rot (quat)       | *~~~~~~~~~~~~~*~~~~~~~~               |  |
| | > Camera Cuts        | [CamA          |CamB    |CamA]         |  |
| | > Voice (Audio)      | |===hero_01.ogg=======|                 |  |
| | > Events             | *       *            *                 |  |
| | > fade (Material)    | o-------o------------o                 |  |
| +----------------------+-----------------^-------------------+    |
|                                        playhead                    |
|  Markers: |intro_start  |mid-beat  |end                            |
+-------------------------------------------------------------------+
```

- Left pane is a flat list of tracks with tree-folds for composite tracks (transform expands to pos/rot/scale sub-rows).
- Right pane is a GPU-drawn bar strip via egui custom painter (egui's default widgets choke at >200 keys). Keys are little diamonds; bezier handles appear when a key is selected.
- Ctrl + mouse-wheel zooms around the cursor, not around zero. Unreal gets this right; Unity gets it wrong.
- Middle-mouse pan. Space to play/pause without stealing keyboard focus from the track list.
- Frame snap is on by default; hold Alt to disable per drag.

### 3.1 Multi-track, mute, solo

Every track gets `mute` and `solo` booleans. Solo on any track mutes everything non-solo. This is stage discipline — you want to audition one layer without deleting the others.

## 4. Keyframe + curve editor

A second tab in the same panel group, or a separate dockable panel that locks to the sequencer's current selection. Same data, different projection.

```
+- Curves: Hero.Pos  (3 curves selected) --------------------------+
|  12.0 +                                                           |
|       |                   ___                                     |
|       |               ___/   \___                                 |
|   0.0 +---o-----o----o---------o------o-----------o-------->      |
|       |    \___/                             \___/                |
| -12.0 +                                                           |
|        0s        1s         2s         3s         4s              |
|                                                                    |
| Tangents:  [Auto] [Linear] [Stepped] [Bezier]      Onion: [3 ^]   |
+-------------------------------------------------------------------+
```

- Multi-curve editing: Shift-click to stack curves in one view (X/Y/Z of a position), color-coded.
- Drag keys with box-select + marquee; Shift constrains to vertical (value-only) or horizontal (time-only).
- Bezier handles drag independently with Alt for asymmetric tangents.
- Onion skin renders the skeleton pose at `t - n*step`, `t`, `t + n*step` as translucent ghosts, driven by the same evaluator the runtime uses. Step configurable; default 1 frame.

### 4.1 Performance

A 10-second clip at 60 Hz, three curves per bone, 60 bones → ~108 000 potential keys. In practice an authored clip is maybe 2 000 keys. Render the bars as a single instanced quad draw per track; only fully rebuild the display list when keys change, not every frame.

## 5. Animation clip = timeline + skeleton

An animation clip is a `TimelineAsset` whose `TrackBinding`s are all skeletal bone paths instead of `SceneId`s, plus a required `SkeletonRef` header. The clip editor is the sequencer with the entity binding UI swapped for a bone picker.

```rust
pub struct AnimClipHeader {
    pub skeleton: AssetRef<Skeleton>,
    pub root_motion: RootMotionExtraction,  // None | FromBone(BoneName) | Explicit
}
```

The preview viewport from Phase 8 §4 stays. It already renders a skinned mesh into an isolated world. All Phase 19 adds is writable keyframes on top.

### 5.1 Retargeting

Retargeting between two skeletons with similar topology is a bone-map table + per-bone scale/offset correction + an optional IK pass for feet and hands.

```
crates/rustforge-core/src/anim/retarget/
├── map.rs                 # BoneMap: src_name -> dst_name (+ rot offset)
├── scale.rs               # per-chain scale fix (limb length ratio)
└── ik.rs                  # two-bone IK for feet/hands, elbow/knee hints
```

- **Bone map** is an asset. The editor auto-generates a best-effort map by name similarity (Levenshtein + common rig dictionary), user corrects.
- **Scale fix**: measure limb lengths in bind pose, scale position channels so foot ends at the same world-space point.
- **Basic IK-adjusted retarget**: after copying rotations, run a two-bone IK from hip to foot and shoulder to hand to pin the end effector to where the source clip put it. This catches most foot-slide.

Hard-retarget (full motion matching, pose space rebinding) is out of scope — that's a Phase 24 animation-graph topic. Phase 19 retargets are "good enough for cutscenes."

## 6. Sequencer in editor: overrides, not snapshots

Phase 7 takes a full world snapshot on Play and restores byte-identical on Stop. The sequencer needs something finer-grained — it drives many small properties on many entities, and playing a sequence in Edit mode (to author it) must not dirty the undo stack.

The mechanism is an **override map**:

```rust
pub struct OverrideMap {
    // (entity, TypeId, FieldPath) -> original value
    originals: HashMap<OverrideKey, ReflectValue>,
    active: bool,
}

impl OverrideMap {
    pub fn write(&mut self, key: OverrideKey, new_value: ReflectValue, world: &mut World) {
        self.originals.entry(key).or_insert_with(|| world.read_field(&key));
        world.write_field(&key, new_value);
    }
    pub fn restore_all(&mut self, world: &mut World) {
        for (k, v) in self.originals.drain() { world.write_field(&k, v); }
    }
}
```

On every sequencer tick in Edit mode, the player evaluates the timeline at the playhead, produces a set of (key, value) writes, and funnels them through `OverrideMap::write`. Eject / Stop scrubbing / closing the panel → `restore_all`. No world snapshot, no undo-stack churn, and scrubbing a 50k-entity scene costs only the entities the timeline touches.

### 6.1 PIE ordering

Phase 7 left ordering implicit. Phase 19 pins it:

1. `engine.tick_play(dt)` runs scripts + physics.
2. `sequencer_player.tick(dt)` evaluates the active timeline and pushes overrides *after* step 1 but *before* animation sampling.
3. Animation systems sample skeletal clips — which are just timelines — and write bone poses.
4. Transform propagation, skinning, rendering.

The sequencer wins any conflict against scripts because the cutscene is meant to be deterministic in Edit preview and in Play. If a script wants to react to the cutscene ending, it listens for an event track, it doesn't fight the overrides.

### 6.2 Relationship with Phase 7 snapshot

When the user presses Play while authoring a timeline, Phase 7's snapshot captures the *pre-override* world — because overrides are restored before snapshot per §6 (this is the important bit). Stop then restores to that pre-override state, and the sequencer panel reattaches its overrides for the scrubbed time. Round-trip preserved.

## 7. Events and script integration

Event tracks fire named events at timestamps. The scripting host already has a message queue (Phase 7 WASM host). Timeline events post into it:

```rust
fn on_event(&mut self, event: &TimelineEvent) {
    self.script_host.post_message(event.binding, TIMELINE_EVENT_CHANNEL, &event.payload);
}
```

Events fire exactly once per forward pass. Scrubbing backward does *not* re-fire (users hate rubber-banding a dialog trigger by dragging the playhead). Looping forward through the same time-window re-fires; this matches Unreal's behavior and is what users expect.

### 7.1 Scrub vs play

During active *playback*, events fire. During *scrub* (user dragging playhead), only reflected-property overrides update; events are suppressed. An explicit "fire event as I cross it" modifier (Alt+drag) is available for sound design work.

## 8. Take Recorder

The one feature that turns this from "a sequencer" into "a thing people actually author cutscenes with." While in PIE, a record button captures reflected-field changes onto a new timeline.

```
crates/rustforge-editor/src/panels/sequencer/
└── take_recorder.rs
```

Mechanism:

1. User presses Record → sequencer creates a blank `TimelineAsset`, subscribes to Phase 7's live-edit-override channel (the Policy C "runtime override" hook).
2. Every override during the take records a keyframe at the current session time on the appropriate track.
3. Transform drags record to transform tracks; inspector edits record to property tracks. Coalescing: if two writes occur within 1/60s on the same field, keep the later.
4. Press Stop on the recorder → timeline asset is saved with a `.take.rtimeline` extension into a dated folder. User promotes it to a regular asset when happy.
5. If the user grabs the player entity and yanks them around for 3 seconds while recording, they get a 3-second transform track with auto-tangent keys at whatever recording rate is configured (default 30 Hz, thinned to minimum-key via curve simplification on stop).

Curve simplification is a classic Ramer-Douglas-Peucker pass on the recorded samples: discard any key whose removal would perturb the curve by less than `epsilon` (default 0.005 world units / 0.5 degrees). This is what keeps a recorded take from being a wall of keys.

## 9. Commands and transactions

Every timeline edit is a Phase 6 command:

- `InsertKeyCommand { track_id, curve_id, keyframe, index }`
- `DeleteKeyCommand { track_id, curve_id, index, before: Keyframe }`
- `MoveKeyCommand { track_id, curve_id, index, before: (t, v), after: (t, v) }`
- `SetTangentCommand { ... }`
- `AddTrackCommand { track }` / `RemoveTrackCommand { track, index, before: Track }`
- `SetMarkerCommand { before, after }`
- `ResizeClipCommand { track_id, before_range, after_range }`

### 9.1 Scrub as transaction

Dragging the playhead rapidly evaluates the timeline and writes overrides — that's pure reads and reversible writes, never a command. But dragging a *key* along the timebar is a drag with a clear begin/end, so:

```rust
// drag start
stack.begin_transaction("Move keyframes");
// per-frame: update overrides + key positions in working copy
// drag end
stack.push(CompositeCommand::of(moves));
stack.end_transaction();
```

This is the same pattern as Phase 6 §4.1 gizmo drags, just lifted to 2D (time, value).

### 9.2 Take-recorder commits

A completed take is a single `AddTrackCommand` per recorded track, wrapped in one `CompositeCommand` with label `"Record take (3.2s)"`. One Ctrl+Z undoes the whole take — discarding it is cheap.

## 10. Camera cuts

Camera-cut tracks are piecewise-constant: at each key-time, the "active camera" binding switches to the referenced entity. At evaluation time this produces one override — `ActiveCameraResource = Some(entity_id)` — which the renderer reads on the next frame.

```
| Camera Cuts:  [CamA     |CamB  | CamA   |CamTop    ] |
```

Design rule: camera-cut is one track per timeline, not per-entity. Nested subsequences have their own camera-cut tracks; the outer timeline's cut wins when both are active (Sequencer precedence, as in Unreal).

While in Edit-mode authoring, the editor viewport camera is orthogonal to the game camera (Phase 7 §9) — but a "Lock to Sequencer Camera" button in the viewport follows the cut track, which is essential for blocking shots.

## 11. Export

Two pipelines, both start from the sequencer's "Export" menu:

- **PNG sequence** (in-tree): advance the timeline by `1/frame_rate` seconds, render the viewport at the chosen resolution to an offscreen target, read back, write one PNG per frame into an output folder. Uses the Phase 2 offscreen-render path plus a readback. Slow but reliable.
- **Video** (sidecar): write the same PNG sequence, then shell out to a user-provided `ffmpeg` binary with a preset (H.264/H.265/ProRes). We do not bundle ffmpeg — licensing. A first-run prompt asks for the ffmpeg path, stored in editor prefs.

```toml
# .rustforge/editor.toml
[export.video]
ffmpeg = "C:/tools/ffmpeg/bin/ffmpeg.exe"
preset = "h264_yuv420p"
```

Audio export: mixdown of audio tracks during the PNG pass into a WAV, muxed by ffmpeg in the sidecar step. An in-process audio encoder is out of scope.

## 12. Build order within Phase 19

1. **`TimelineAsset` + `Curve` + evaluator** — pure data, unit-tested. Round-trip RON, sample at t, tangent modes, onion-skin query.
2. **`OverrideMap`** — write-through and restore-all, independent of the panel. Unit tests: "write N, restore, world equals original."
3. **Sequencer panel (read-only)** — ruler, track list, timeline bars. Can load an asset and display it. No editing, no playback.
4. **Playback** — play/pause/scrub the playhead, drive `OverrideMap`, eject restores. In Edit mode only.
5. **Keyframe editing commands** — insert, delete, move, retangent. Wire through Phase 6. Ctrl+Z parity with the rest of the editor.
6. **Curve editor view** — multi-curve, tangent handles, onion skin.
7. **Track types** — transform, property, audio, event, camera-cut. Subsequence last.
8. **Animation clip binding** — skeleton-bound timeline, preview viewport integration, bone picker in the track list.
9. **Retargeting** — bone map asset, auto-map, scale fix, IK pass.
10. **Events** — dispatch into WASM host, fire-once semantics, scrub suppression.
11. **PIE integration** — ordering per §6.1, Phase 7 snapshot interaction, determinism test.
12. **Take Recorder** — subscribe to live overrides, write keys, curve simplification on stop.
13. **Export** — PNG sequence, then ffmpeg sidecar prompt.
14. **Random-walk test** — 1000 random timeline edits, undo all, redo all; asset round-trips byte-identical.

## 13. Scope boundaries — what's NOT in Phase 19

- ❌ **Animation state machines and blend spaces.** That's Phase 24. An `AnimClip` track plays one clip linearly; layering, transitions, and 2D blend spaces are a separate authoring tool.
- ❌ **Non-linear editor (NLE).** No ripple edits, no track compositing, no transitions between sequences. Linear timeline only.
- ❌ **In-process video encoding.** ffmpeg sidecar only. No x264/libvpx linked in.
- ❌ **Motion capture import.** FBX/BVH mocap is its own ingest; Phase 19 assumes clips are already `.rtimeline` assets produced by DCC export or Take Recorder.
- ❌ **Live collaborative sequencing** (multi-user editing the same timeline). Single-author only.
- ❌ **Audio mixing UI beyond gain on a track.** No per-clip EQ, no bus routing. Reuse Phase 8's future audio-mixer work.
- ❌ **Full IK solver / constraints authoring.** The retarget IK is hard-coded two-bone. General constraint authoring lives elsewhere.
- ❌ **Physics simulation recording.** Take Recorder captures reflected-field changes; it does not run physics into a timeline and bake.
- ❌ **Dope sheet as a distinct panel.** The track view *is* the dope sheet. A separate Maya-style dope sheet is redundant.

## 14. Risks and gotchas

- **Entity-ID churn across PIE.** Covered by using `SceneId` in bindings, not `Entity`. Re-tested every cycle; the random-walk test should include Play/Stop in its mix.
- **Override leaks on panel close.** User closes the sequencer panel while the playhead is off-zero. Overrides must restore. Tie `OverrideMap::restore_all` to panel Drop, not just an eject button.
- **Curve evaluation determinism.** Bezier evaluation must be bit-exact across runs for PIE determinism. Don't use `f64` in one path and `f32` in another; pick `f32` everywhere and stick with it.
- **Scrub thrash.** Rapid scrubbing re-evaluates every frame. At 50k entities and 200 tracks this is still cheap — tracks are sparse — but a pathological sequence could chew. Profile; cache per-track last-sample-index so neighbouring evaluations don't binary-search from zero.
- **Onion skin cost.** Rendering three skinned poses instead of one triples the skinning cost on the preview viewport. Downsample the onion targets to half-res; they're translucent anyway.
- **Retargeting bone-map drift.** Source rig renames a bone; the map points at an orphan. Loader must warn and show the orphans in the bone-map editor in red. Don't silently drop them.
- **Take Recorder undo pollution.** Without coalescing, a take generates thousands of transient commands. Solution in §9.2: push nothing until Stop, then emit one `CompositeCommand`. The naive implementation that pushes per-key is a memory bomb.
- **Event-fire-on-scrub.** Classic bug: user scrubs from 0 → 5s, every "play sound" event fires simultaneously, wall of noise. §7.1 rule prevents this; verify with a test.
- **Camera-cut across snapshot restore.** PIE records the active camera as a runtime override; Stop restores. If the Edit-mode sequencer was driving the camera at the time of Play, the snapshot captures the *overridden* camera. Rule: restore sequencer overrides *before* taking the Phase 7 snapshot. Added as a test in step 11.
- **ffmpeg path quoting.** The sidecar shell-out is a quoting hazard on Windows. Use `std::process::Command` with argv, never shell string concatenation. Same discipline as Phase 8 §5 external-editor launch.
- **Audio drift during export.** PNG rendering is slower than real time; audio must be mixed on the wallclock budget of the timeline, not the render loop. Mix to a separate buffer keyed by timeline time, not tick count.
- **Subsequence infinite recursion.** Timeline A contains a subsequence track referencing itself (or A → B → A). Detect at load with a DFS cycle check; refuse to play; show a clear error in the panel.
- **Tangent-mode round-trip on import.** Importing an FBX animation clip to `.rtimeline` must choose a tangent mode. FBX's "auto" is not our Auto. Document the mapping; don't pretend they're identical.

## 15. Exit criteria

Phase 19 is done when all of these are true:

- [ ] `.rtimeline` assets load, save, and round-trip byte-identical.
- [ ] Sequencer panel opens any `.rtimeline`; shows ruler, tracks, keys, markers, playhead.
- [ ] Play / Pause / Scrub / Loop / frame-rate-switch all work in Edit mode.
- [ ] All seven track types (transform, property, anim-clip, audio, event, subsequence, camera-cut) evaluate correctly.
- [ ] Keyframe editor supports Auto / Linear / Stepped / Bezier tangents, verified by round-trip eval test.
- [ ] Multi-curve view edits X/Y/Z of a `Vec3` track together with correct axis-lock modifiers.
- [ ] Onion skin displays at N-1, N, N+1 frames for an animation clip bound to a skeleton.
- [ ] Bone-map retargeting plays a source clip on a target skeleton with foot-pin IK within 5% error.
- [ ] Every timeline edit is a Phase 6 command; Ctrl+Z undoes individual key moves and whole takes.
- [ ] Closing the sequencer panel restores all overrides (verified by byte-compare on reflected fields).
- [ ] Entering PIE with a sequencer active snapshots the *pre-override* world; Stop restores to that state.
- [ ] Events fire exactly once on forward play, never on backward scrub, configurable on forward scrub.
- [ ] Take Recorder produces a `.take.rtimeline` with curve-simplified keys after a PIE session.
- [ ] Camera-cut track overrides the active camera at key-times; "Lock to Sequencer Camera" follows in the viewport.
- [ ] PNG-sequence export produces `duration * frame_rate` frames of correct size.
- [ ] ffmpeg sidecar export muxes PNG + WAV into an MP4 given a configured ffmpeg path.
- [ ] Random-walk test (1000 random edits + 50 Play/Stop cycles) leaves asset + world byte-identical after full undo.
- [ ] `rustforge-core` still builds without the `editor` feature (timeline runtime stays; authoring stays editor-side).
