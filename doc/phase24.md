# Phase 24 — Animation Graph & Runtime

Phase 19 shipped the timeline and keyframe authoring tools (curve editor, Sequencer, retarget tables) but explicitly punted two features on the grounds that neither was authoring and both needed a runtime evaluator first: **state machines** and **blend spaces**. Phase 24 closes that deferral. It builds the `.ranimgraph` asset format, the per-actor runtime, the node-graph editor UI (reusing Phase 20's `rustforge-node-graph` crate), and the Persona-parity feature set — layered blends, IK, montages, root motion — so gameplay code can drive locomotion, combat, and reactive animation without writing bespoke blend code per character.

Phase 8's animation preview was a read-only scrubber. Phase 19 made clips editable. Phase 24 makes them **composable at runtime**.

## Goals

By end of Phase 24:

1. **`.ranimgraph` asset** — RON document containing nodes, transitions, parameters, and sync groups, round-trippable through the editor.
2. **State machine runtime** — states, transitions with condition expressions, min-duration, cross-fade curves, entry/exit events.
3. **Blend spaces** — 1D and 2D coordinate-driven blends (speed, direction) with configurable sample positions.
4. **Pose blend nodes** — additive, layered (masked per-bone), and weighted N-way blends.
5. **IK nodes** — two-bone IK, FABRIK chain, foot-placement IK with ground raycast.
6. **Parameter system** — typed bool/float/int/trigger slots, set from scripts or native gameplay each frame.
7. **Graph editor panel** — visual authoring on top of the Phase 20 shared node-graph crate.
8. **Retargeting integration** — the graph consumes clips through Phase 19 retarget tables; one graph drives many skeletons.
9. **Root motion** — extraction from clip, delta fed into the character controller; remaining pose applied in-place.
10. **Montage system** — one-shot clips that override the base graph on a named slot (attack over locomotion).
11. **Sequencer bridge** — Phase 19 Sequencer tracks can drive graph parameters during cinematics.
12. **Debugging** — live parameter inspector, active-state highlight, breakpoint-on-transition.

## 1. Asset format: `.ranimgraph`

One graph per asset. RON, versioned, flat node table (no nesting — groups are editor-only visual affordances stored as metadata).

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct AnimGraphAsset {
    pub version: u32,
    pub parameters: Vec<ParamDecl>,
    pub nodes: Vec<GraphNode>,            // dense, indexed by NodeId
    pub state_machines: Vec<StateMachine>,
    pub root: NodeId,                     // output pose
    pub sync_groups: Vec<SyncGroupDecl>,
    pub skeleton: AssetGuid,              // source skeleton (retargeting handles others)
    pub editor: EditorMetadata,           // panel positions, group boxes, colors
}

#[derive(Reflect, Serialize, Deserialize)]
pub enum GraphNode {
    ClipPlayer { clip: AssetGuid, speed: ParamRef<f32>, loop_: bool, sync: Option<SyncGroupId> },
    StateMachineRef(StateMachineId),
    BlendSpace1D { samples: Vec<Sample1D>, axis: ParamRef<f32> },
    BlendSpace2D { samples: Vec<Sample2D>, x: ParamRef<f32>, y: ParamRef<f32> },
    Blend2 { a: NodeId, b: NodeId, alpha: ParamRef<f32> },
    BlendN { inputs: Vec<NodeId>, weights: Vec<ParamRef<f32>> },
    LayeredBlend { base: NodeId, layer: NodeId, mask: BoneMaskId, alpha: ParamRef<f32> },
    Additive { base: NodeId, add: NodeId, alpha: ParamRef<f32> },
    TwoBoneIK { input: NodeId, chain: IkChain, target: ParamRef<Vec3>, pole: ParamRef<Vec3> },
    Fabrik { input: NodeId, chain: Vec<BoneId>, target: ParamRef<Vec3>, iterations: u8 },
    FootPlacement { input: NodeId, feet: [FootConfig; 2], enable: ParamRef<bool> },
    SlotNode { input: NodeId, slot: SlotId },   // montage attachment point
}
```

Keep the node enum deliberately flat. Dispatch per-frame touches every actor, every node — a tag match is cheaper than a trait object vtable. Saving nodes in a `Vec<GraphNode>` indexed by a `NodeId(u32)` wins over a graph-of-boxes for cache locality and for hot-reload (whole asset swap, no pointer fix-up).

### 1.1 Parameters

```rust
pub struct ParamDecl { pub name: String, pub kind: ParamKind, pub default: ParamValue }

pub enum ParamKind { Bool, Float, Int, Trigger, Vec3 }

pub struct ParamRef<T> { pub idx: ParamIdx, _p: PhantomData<T> }
```

Triggers are one-shot booleans consumed by the first transition condition that reads them in a tick, then cleared. This is the same shape Unreal's Animation Blueprints use and it avoids the "why did my jump fire twice" class of bug.

## 2. State machine

A state machine is a list of states plus a list of transitions. States wrap any `GraphNode`, typically a `ClipPlayer` or `BlendSpace`.

```
┌──────────────────── Locomotion SM ───────────────────┐
│                                                       │
│       ┌─────────┐  speed>0.1       ┌──────────────┐  │
│   ┌──►│  Idle   │ ───────────────► │  Locomotion  │  │
│   │   └─────────┘                  │ (BlendSpace) │  │
│   │      ▲                         └──────┬───────┘  │
│   │      │  speed<0.05                    │          │
│   │      │  dur>=0.25                     │          │
│   │      └─────────────────────────────── ┘          │
│   │                                                   │
│   │   on Trigger(jump)       ┌──────────┐            │
│   └───────────────────────── │  Jump    │            │
│          (any state)         └──────────┘            │
└───────────────────────────────────────────────────────┘
```

```rust
pub struct StateMachine {
    pub id: StateMachineId,
    pub states: Vec<State>,          // index = StateId
    pub entry: StateId,
    pub transitions: Vec<Transition>,
    pub any_state_transitions: Vec<Transition>,  // evaluated from every state
}

pub struct State {
    pub name: String,
    pub node: NodeId,
    pub on_entry: Vec<GraphEvent>,
    pub on_exit:  Vec<GraphEvent>,
}

pub struct Transition {
    pub from: StateId,
    pub to: StateId,
    pub condition: Condition,
    pub duration: f32,
    pub curve: BlendCurve,           // Linear | Ease | Custom(CurveId)
    pub min_source_duration: f32,    // anti-oscillation lower bound
    pub priority: i16,
    pub interruptible: bool,         // can this transition be interrupted mid-blend
}
```

### 2.1 Conditions

Avoid embedding a full expression language here. Phase 19 already defined `FormulaBinding`; reuse it. Most conditions are `param > const` or `trigger`, so also offer a first-class shape:

```rust
pub enum Condition {
    Always,
    Trigger(ParamIdx),
    FloatCmp { param: ParamIdx, op: Cmp, value: f32 },
    IntCmp   { param: ParamIdx, op: Cmp, value: i32 },
    Bool     { param: ParamIdx, value: bool },
    And(Vec<Condition>),
    Or(Vec<Condition>),
    Not(Box<Condition>),
    Formula(FormulaId),              // escape hatch to Phase 19 formulas
}
```

`Formula` is the catch-all; the typed shapes cover >90% of real graphs and evaluate without allocating.

### 2.2 Transition evaluation order

Per tick, per state machine:

1. Evaluate `any_state_transitions` in priority order. First match wins.
2. Otherwise evaluate transitions outgoing from current state in priority order.
3. If current state has been active < `min_source_duration`, skip.
4. If already blending and the pending transition is not `interruptible`, skip.
5. On match, start a cross-fade; fire `on_exit` events for source, `on_entry` for target.

## 3. Blend spaces

1D: a speed axis with samples at 0.0 (Idle), 2.0 (Walk), 5.0 (Run), 8.0 (Sprint). 2D: add strafe direction.

```rust
pub struct Sample1D { pub clip: AssetGuid, pub x: f32 }
pub struct Sample2D { pub clip: AssetGuid, pub x: f32, pub y: f32 }
```

Eval for 2D: Delaunay-triangulate sample positions once at load, cache in the graph instance. Per tick, locate the triangle containing `(x, y)`, barycentric-weight the three clips. Outside the hull: clamp to nearest edge. For 1D: linear between the two enclosing samples.

Sync groups bind blend-space members to a shared normalized time so a Walk and a Run at 50% blend stay foot-phase-locked. The sync group declares a leader (usually the highest-weight clip); other clips are time-warped so their normalized phase matches.

## 4. Pose blending nodes

### 4.1 Representation

A pose is `Vec<Transform>` indexed by `BoneId`. Blending is SLERP for rotation, LERP for translation and scale. Do not LERP rotation as four-component vectors — noticeable under long blends. Use `glam::Quat::slerp`.

### 4.2 Masked blend

A `BoneMask` is a weight per bone (0..=1). Layered blend applies `layer` on top of `base` with the mask:

```rust
for bone in 0..skeleton.bone_count() {
    let w = alpha * mask.weight(bone);
    out[bone].rotation = base[bone].rotation.slerp(layer[bone].rotation, w);
    // translation/scale similar
}
```

Bone masks are their own asset — `.rbonemask`, RON list of (bone_path, weight). Edited alongside the graph but reusable across graphs on the same skeleton.

### 4.3 Additive

Additive clips are **authored** as a delta against a reference pose (usually the first frame). At runtime:

```
out[b].rot = base[b].rot * (Quat::IDENTITY.slerp(add[b].rot, alpha))
out[b].pos = base[b].pos + add[b].pos * alpha
```

Store the additive-ness as a flag on the clip asset, not in the graph — it's a property of the source animation, not of how it's used.

## 5. IK nodes

Three flavors, each runs *after* the blended pose is produced.

### 5.1 Two-bone IK

Closed-form for three-joint chains (upper arm, forearm, hand). Resolves with a pole vector to disambiguate elbow direction. Cheap, stable, predictable. Default for arm reach and simple leg plants.

```rust
pub struct IkChain { pub root: BoneId, pub mid: BoneId, pub tip: BoneId }
fn solve_two_bone(out: &mut Pose, chain: &IkChain, target: Vec3, pole: Vec3) { /* ... */ }
```

### 5.2 FABRIK

Forward-And-Backward Reaching Inverse Kinematics. Iterative, handles N-bone chains. Default iteration count 4; expose in the node. Use for spines, tails, long tentacles. Stop when tip-to-target < epsilon.

### 5.3 Foot placement

Per-foot: raycast from pelvis-down to find ground, adjust foot bone to contact, adjust pelvis to keep both legs solvable (limit: do not lift the pelvis more than `max_pelvis_offset`), resolve each leg with two-bone IK. `enable` parameter gates at the graph level — disable during jumps and ragdoll.

## 6. Parameter system

Parameters live on the **graph instance** (per actor), not the asset. Writes come from:

- Gameplay code: `graph.set_float("speed", velocity.length())` each tick.
- Scripts: via the Phase 7 script host; reflection-driven binding to the param block.
- Sequencer (Phase 19): a param track writes into the graph during cinematics — see §11.

```rust
pub struct GraphInstance {
    asset: Handle<AnimGraphAsset>,
    params: ParamBlock,              // SoA by kind
    active_state: Vec<StateId>,      // one per state machine
    blend: Option<ActiveBlend>,
    pose_a: Pose,                    // double-buffered
    pose_b: Pose,
    front: u8,
    montages: SmallVec<[ActiveMontage; 2]>,
    sync_cache: SyncState,
}
```

Triggers are special: `set_trigger(name)` queues a pending bit; the next `tick` sets the bit for one frame's transition evaluation, then clears it. This guarantees a trigger is observed exactly once.

## 7. Graph editor panel

The editor reuses the Phase 20 shared node-graph crate, `rustforge-node-graph`. That crate owns pan/zoom, port hit-testing, wire routing, rubber-band selection, and copy/paste. Phase 24 layers domain-specific node types and a transition-pane sub-editor on top.

```
┌─ Animation Graph: hero_locomotion.ranimgraph ───────────────────────┐
│ Params ▼              │                                              │
│  speed   float  0.0   │   ┌──────────┐        ┌──────────────┐       │
│  dir     float  0.0   │   │ BlendSp  │        │ LayeredBlend │       │
│  jump    trigger      │   │ (loco)   │───pose─┤ base         │       │
│  aim_t   vec3   _     │   └──────────┘        │              │─► out │
│ [+]                   │   ┌──────────┐        │ layer        │       │
│                       │   │  Aim SM  │───pose─┤              │       │
│ Masks ▼               │   └──────────┘        └──────────────┘       │
│  upper_body           │                                               │
│  left_arm             │                                               │
│ [+]                   │                                               │
└────────────────────────────────────────────────────────────────────────┘
```

Double-click a `StateMachineRef` node → opens a sub-panel showing states and transitions. Double-click a state → opens the sub-graph backing that state. Breadcrumb bar at the top shows the stack.

Transitions in the state-machine sub-view render as arrows. Selecting an arrow exposes its condition, duration, curve, priority in the inspector; edits route through Phase 6 commands exactly like every other inspector edit.

## 8. Runtime evaluation

Per actor per tick:

1. Host game code writes parameter values.
2. State machines evaluate conditions (§2.2), possibly starting new blends.
3. Walk the node tree from `root` in post-order, producing a pose per node.
4. Apply IK nodes (they are ordinary nodes, but by convention placed near the output).
5. Extract root motion delta if enabled (§10).
6. Apply active montages as overrides on named slots (§9).
7. Write final pose to the skinning buffer (Phase 8 §4 preview; Phase 19 retargeting for non-source skeletons).
8. Fire queued graph events to the host.

Double-buffered poses: tick writes to `pose_back`, render reads from `pose_front`, swap at tick end. Job scheduler (Phase 11) runs all graph instances in parallel — they share no state.

Pose cache: nodes whose inputs and parameters didn't change since last tick reuse last tick's pose. Worth the ~128 bytes of hash state per node once graphs grow past a dozen nodes.

## 9. Montage system

A montage is a short-lived override played on a **slot**. The graph declares slots by name (`"upper_body_attack"`, `"full_body_hit_react"`); gameplay code calls:

```rust
graph.play_montage("upper_body_attack", clip_guid, MontageOptions {
    blend_in: 0.1, blend_out: 0.15, weight: 1.0, mask: Some(upper_body_mask),
});
```

The montage tracks its own time and blends itself in/out on the identified `SlotNode`. Multiple montages on the same slot: the latest wins, previous blends out. On completion, fires `OnMontageComplete` to the host.

Montages are not in the asset — they are runtime-only instances. The asset only declares the slot nodes that accept them. This keeps one-shots (react, attack, emote) outside the graph authoring surface, which otherwise would bloat with every context-specific animation.

## 10. Root motion

Clips flagged `has_root_motion` carry a root-bone channel that moves the character through space. The graph extracts per-tick delta:

```rust
let delta = clip.root_motion_delta(prev_time, this_time);   // (translation, rotation)
```

Blend spaces: weighted sum of deltas across active samples. State machine transitions: interpolate between source and target deltas by the same `alpha` driving the pose blend.

The delta is handed to the character controller (Phase — wherever player physics lives), which interprets it — usually as desired velocity. The pose written into skinning has the root bone zeroed so the mesh doesn't "double-move."

Opt-in per clip and per graph. Default off; many games drive motion purely from input and animation is decorative.

## 11. Sequencer integration

Phase 19 Sequencer runs cinematics. During a cinematic, the director often wants to set animation state directly: "enter state `CrouchIdle`, hold for 2.5 s, trigger `stand_up`."

Approach: a new Sequencer track type `AnimGraphParameter` binds to a target actor's graph and to one parameter. Keyframing the track writes that parameter each tick the Sequencer is active. A companion `AnimGraphTrigger` track fires triggers at specific times. No new concepts in the graph — the Sequencer is just another parameter writer.

Full-body takeover (no graph at all during a cutscene) is already supported by the Phase 19 clip track; Phase 24 does not duplicate it.

## 12. Debugging

### 12.1 Live parameter inspector

Selecting an actor in PIE (Phase 7) with a graph shows its param block in a read-write inspector. Writing a param from the inspector bypasses the script/gameplay source — use it for bisecting "is my input wrong or is my graph wrong."

### 12.2 Active-state highlight

When the Graph editor is open on a running actor's graph, the active state(s) render with a bright outline, blends show as gradients between source and target. One frame's lag is acceptable.

### 12.3 Breakpoint on transition

Right-click a transition → "Break when this fires." Next time the transition evaluates true, the editor pauses PIE (Phase 7 `PlayState::Paused`) and scrolls the graph view to the transition. Useful for catching oscillation bugs.

### 12.4 Transition log

A ring buffer of the last 128 transitions (from, to, time, condition that matched) per actor. Exported with profiler JSON (Phase 8 §6).

## 13. Hot reload

`.ranimgraph` follows Phase 5 file-watcher rules. On reimport: build the new asset, for every live `GraphInstance` map the old active state by **name** to the new asset's state of the same name. Unknown name → fall back to entry state. Parameters migrate by name and kind; kind mismatch resets to new-asset default with a log warning. Active montages and in-flight blends are cancelled — trying to preserve them across structural edits is not worth the complexity.

## 14. Build order

Land in this sequence; each step is shippable on its own.

1. **Asset format + loader** (§1) — RON round-trip, reflection, hot-reload hook. No runtime yet; editor can open a file and show params.
2. **Runtime skeleton** (§6, §8) — `GraphInstance`, `ParamBlock`, tick scaffolding. Single `ClipPlayer` node evaluates; output goes to skinning. Replaces Phase 8 preview for test actors.
3. **State machine** (§2) — states, transitions, cross-fade, events. Feature-complete SM evaluator with the typed-condition subset.
4. **Blend spaces** (§3) + sync groups. 1D then 2D.
5. **Blend nodes** (§4) — additive, layered, N-way.
6. **IK** (§5) — two-bone first (one week), FABRIK (one week), foot placement last.
7. **Graph editor panel** (§7) — on top of Phase 20's shared crate.
8. **Montages** (§9) and **root motion** (§10).
9. **Sequencer bridge** (§11).
10. **Debug tooling** (§12) — do not skimp. Transition breakpoint alone pays for itself by end of Phase 24.

## 15. Scope boundaries — what's NOT in Phase 24

- ❌ **Motion matching.** Data-driven clip selection from a large corpus is a different architecture (nearest-neighbor search over pose descriptors). Separate future phase.
- ❌ **Physics-driven animation.** PhAT (physics asset tool), Chaos Cloth-equivalent, ragdoll blending — Phase 25.
- ❌ **ML-based animation.** Learned controllers, motion synthesis networks. Not a near-term priority.
- ❌ **Facial rigging & animation pipeline.** Blend shape evaluator, lip sync, LiveLink ingestion — separate future phase. The graph has no blend-shape nodes in Phase 24.
- ❌ **Curve-only tracks on the graph.** If you need to animate a material scalar, use the Phase 19 Sequencer or a script. Do not add curve-output nodes to the animation graph.
- ❌ **Subgraphs as reusable assets.** Only state machines compose. A full "AnimGraphFunction" asset is a nontrivial redesign; defer until there is clear demand.
- ❌ **Networked animation.** Replication of graph state over the wire. Phase 24's graph is authoritative per-client; a future netcode phase handles prediction and reconciliation.
- ❌ **Dynamic graph rebuild at runtime** (generating nodes from code). Assets are immutable at runtime outside hot-reload.

## 16. Risks & gotchas

- **Transition oscillation.** Two transitions with symmetric conditions ping-pong. `min_source_duration` is the primary mitigation; the transition-log debug view is the secondary one. Document both in the tooltip.
- **Trigger lost if set outside a tick.** A script sets `trigger("jump")`, next tick the graph isn't ticked (actor culled?), the trigger is never observed. Policy: triggers persist across ticks until consumed or until the graph instance is reset. Document explicitly — the "fire exactly once" guarantee is per *consumption*, not per frame.
- **Blend-space sample Delaunay cost.** Triangulation on asset load is fine; triangulation on every hot-reload is fine. Don't do it per tick. Cache the triangulation on the graph instance.
- **IK instability at chain limits.** FABRIK with target past chain length oscillates unless clamped. Clamp target to chain length minus epsilon during solve; expose visually in the editor as a reach indicator.
- **Retargeting + IK.** The IK chain is defined on the source skeleton's bone IDs. Retargeted output skeletons have different bone IDs. Solve: retarget tables (Phase 19) must include IK-chain name mappings. Bake resolved chains per retarget at load, not per tick.
- **Sync group leader changes mid-blend.** A 2D blend-space whose leader (highest weight) switches from Walk to Run mid-tick produces a phase pop. Smooth by keeping the prior leader's phase as the authority until the transition completes; swap at the next full cycle boundary of the new leader.
- **Parameter name collisions on hot reload.** Two different float params share a name across a reload because a designer renamed one. Names are the only migration key available; accept the loss and log. Renaming UIs should offer a "migrate data from" dropdown to avoid silent drops.
- **Root motion double-apply.** The character controller consumes the delta *and* the pose contains a moving root bone. Mitigation: zeroing the root bone in the pose is the graph's responsibility, not the controller's, and it must happen after blending, before skinning.
- **Montage on a slot that doesn't exist in the graph.** Fail loudly in editor builds, silently drop in shipped builds. Never panic on gameplay-supplied slot names.
- **Hot-reload during an active transition.** Blend pointers refer to node indices that may have changed. Cancel the transition on reload; don't try to remap it. The one-frame pop is better than a crash.
- **Editor highlight lag.** The editor inspects a runtime data structure owned by the tick system. Share via a read-only snapshot published at tick end, not by locking. A one-frame stale view is fine; a stall is not.
- **Graph vs. Sequencer authority.** Both write parameters. Last-writer-wins is confusing. Sequencer parameter tracks should mark a parameter "driven" for the duration of the track, and the inspector should show a lock icon; gameplay writes during that window are queued and applied after the track ends.

## 17. Exit criteria

Phase 24 is done when all of these hold:

- [ ] `.ranimgraph` assets load, save, and round-trip without diff noise.
- [ ] A humanoid actor can be driven entirely by a graph containing: blend-space locomotion, aim state machine layered on top, jump state, and foot-placement IK — with one parameter (speed) and one trigger (jump) as the only host writes.
- [ ] State-machine transitions fire in priority order, honor `min_source_duration`, and cross-fade on the configured curve.
- [ ] Blend-space 1D and 2D sample correctly at hull interior, hull edge, and outside-hull (clamp) positions; sync groups keep foot phase locked.
- [ ] Layered blend with a bone mask produces an upper-body override that leaves the legs bit-identical to the base pose (verified by test comparing bones in the zero-mask region).
- [ ] Additive layer applies on top of a base without drift over 10 000 ticks (additive identity preserved at `alpha = 0`).
- [ ] Two-bone IK, FABRIK, and foot placement each have a visual test scene and pass their respective unit tests (target reached within tolerance, stable under target jitter).
- [ ] Trigger parameter is observed exactly once per `set_trigger` call, regardless of how many transitions read it.
- [ ] The graph editor panel opens, edits a graph, saves, and the running PIE actor picks up the change via hot reload.
- [ ] Transition breakpoint pauses PIE when its transition fires and highlights the transition in the editor.
- [ ] Active-state highlight updates in the editor within one frame of the state change in PIE.
- [ ] Root motion drives the character controller on a test clip; the skinned mesh stays in place relative to the controller.
- [ ] A montage plays on `upper_body_attack` while the base graph continues to drive legs; completion fires `OnMontageComplete`.
- [ ] A Phase 19 Sequencer with an `AnimGraphParameter` track drives a graph parameter over a 3-second clip; gameplay writes during that window are queued and applied after.
- [ ] Retargeting: the same graph asset drives two different skeletons correctly via Phase 19 retarget tables, including IK chain mappings.
- [ ] Graph evaluation parallelizes across actors with no shared mutable state (verified by running 512 graph instances on the Phase 11 job system without contention warnings).
- [ ] `rustforge-core` still builds and runs without the `editor` feature; the graph runtime is not gated but the graph editor panel is.
