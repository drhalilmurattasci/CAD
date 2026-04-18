# Phase 35 — Advanced Animation: Motion Matching, Physics-Driven, Facial

Phase 24 shipped the classic animation graph: state machines, blend spaces, two-bone IK, FABRIK, foot placement, layered blends, montages, root motion. That is the 2015-era AAA stack and covers perhaps 80% of what a serious game needs. Phase 35 is the other 20% — the modern AAA techniques that ship in *The Last of Us Part II*, *For Honor*, *Death Stranding*, *Horizon Forbidden West*, *Spider-Man 2*. Motion matching instead of blend-space locomotion. Physics-driven hit reactions instead of authored flinches. Full-body IK instead of per-limb. Facial rigs and lip sync. Performance-capture retargeting. Keyframe compression. The goal is not to replace Phase 24 — the graph still drives combat, AI, cinematics — but to layer data-driven and physics-coupled techniques on top, as optional node types and dedicated pipelines.

Upstream: Phase 19 (timeline, curves, retargeting tables), Phase 24 (graph runtime, IK, montages, parameter system), Phase 25 (ragdoll runtime, PhAT assets, joint motors), Phase 31 (world-partition streaming pattern — reused for animation sets), Phase 11 (job system for parallel pose-database queries).

## Goals

By end of Phase 35:

1. **Motion matching runtime** — per-frame nearest-neighbor pose lookup against a pose-feature database; KD-tree + trajectory cost function.
2. **Motion database authoring** — corpus import, feature-vector extraction, bake, in-editor visualization.
3. **Physics-driven animation** — powered-ragdoll blend on Phase 25 ragdoll; hit reactions, balance recovery, joint-motor PD targets.
4. **Full-body IK (FBIK) solver** — COM-preserving whole-skeleton solver as an optional Phase 24 graph node.
5. **Facial animation rig** — FACS blendshape + bone hybrid; Maya/Blender importer extension; runtime evaluator.
6. **Phoneme-to-mouth lip sync** — audio → phoneme stream → mouth blend weights with a baseline phoneme set.
7. **Performance capture retargeting** — `.bvh` and `.fbx` mocap import, cleanup UI, retarget onto engine skeletons.
8. **Animation compression** — ACL-style curve-fit keyframe compression; 8-10× ratios on large libraries.
9. **Additive animations** — first-class authoring and graph integration (breathing, flinch, aim-offset).
10. **Animation streaming** — large per-character sets stream on demand, preloaded on spawn.
11. **Editor panels** — Motion Matching, Facial Rig, Lip Sync.
12. **Performance budget** — documented cost per technique per platform tier.

## 1. Motion matching — the pose-database lookup

Motion matching replaces or supplements a blend-space locomotion state machine. Instead of authoring samples and blending between them, you capture a large corpus of motion clips, chop them into per-frame pose descriptors, and every tick search for the database frame whose pose and trajectory best match the character's current state and desired future trajectory. Play two or three frames of that clip, then search again. It feels ultra-responsive because the system effectively cuts to the best-matching animation every few frames, using variations an author would never encode. Technique is decades old in academia but shipped mainstream with *For Honor* (Ubisoft, 2017); *The Last of Us Part II* (2020) refined it into the current standard.

### 1.1 Feature vector

Each database frame is a fixed-width feature vector. Two classes:

- **Trajectory** — predicted positions + facing directions at +0.33, +0.66, +1.0 s, character-local. Three `Vec2` positions + three `Vec2` facings → 12 floats.
- **Pose** — positions and velocities of hand, foot, head bones in character-local space. Six bones × (pos + vel) × 3 = 36 floats.

Total ~48 floats. Tunable in the database header.

```rust
pub struct PoseFeature {
    pub traj_pos: [Vec2; 3],
    pub traj_dir: [Vec2; 3],
    pub bones: [BoneFeature; 6],   // hands, feet, head
}
pub struct BoneFeature { pub pos: Vec3, pub vel: Vec3 }
```

### 1.2 Database asset

```rust
pub struct MotionDatabase {
    pub skeleton: AssetGuid,
    pub feature_schema: FeatureSchema,
    pub features: Vec<PoseFeature>,        // one per corpus frame
    pub source:   Vec<SourceFrame>,        // (clip, frame) per feature
    pub kd_tree:  KdTree,                  // built at bake
    pub cost_weights: CostWeights,
}
pub struct SourceFrame { pub clip: AssetGuid, pub frame: u32 }
```

`.rmmdb`. Sizes: locomotion-only ~500 frames; locomotion + combat ~2-5k; everything ~20k. Cost scales with KD-tree query, not linearly.

### 1.3 Runtime query

Per tick: compute target feature from current pose + gameplay-requested trajectory; weight-multiply against `cost_weights` (trajectory horizons usually dominate); K-NN the KD-tree (K=8); prefer candidates close to the *currently playing* clip (continuity bonus); if best cost < threshold and current clip still valid, don't switch; otherwise switch and play forward `min_dwell` frames (5-10) before next query.

```rust
impl MotionMatcher {
    pub fn tick(&mut self, target: &PoseFeature) -> SourceFrame {
        if self.dwell_remaining > 0 { self.dwell_remaining -= 1; return self.advance(); }
        let best = self.pick_with_continuity(&self.db.kd_tree.knn(target, 8));
        if best.cost < self.last_cost * CONTINUITY_THRESHOLD {
            self.current = best.source;
            self.dwell_remaining = MIN_DWELL;
        }
        self.advance()
    }
}
```

K-NN on 48-D degenerates toward linear scan. Mitigation: PCA-reduce to ~16 dims at bake, search reduced, re-score top K=32 on full vectors. Standard trick; benchmark both paths.

### 1.4 Motion matching as a graph node

```rust
GraphNode::MotionMatch {
    db: AssetGuid,
    trajectory_source: ParamRef<TrajectoryInput>,
    weights: CostWeights,
}
```

Outputs a pose like any other node. Character can use matched lower body + authored upper-body SM via layered blend. Motion matching is a node type, not a graph replacement.

## 2. Motion database authoring

Offline bake, CPU-only, runs as a Phase 4 import step:

```
corpus ─► import clips ─► concat ─► extract features ─► PCA ─► KD-tree ─► .rmmdb
```

### 2.1 Motion Matching panel

```
┌─ Motion Matching: hero_locomotion.rmmdb ─────────────────────┐
│ Corpus 47 clips / 8,412 frames    Skeleton hero.skel         │
│ Features: horizons 0.33/0.66/1.00 s                          │
│           bones LHand RHand LFoot RFoot Head Hips            │
│ Weights:  traj_pos 1.5  traj_dir 2.0  bone_vel 0.5  cont 1.0 │
│ Viz:      [scatter plot: PCA axes 1×2, per-clip hue]         │
│           hover clip:frame, click previews                   │
│ Stats:    avg 0.21 ms / max 0.47 ms / depth 14               │
│ [ Rebake ]  [ Test in PIE → ]                                │
└──────────────────────────────────────────────────────────────┘
```

PCA scatter is load-bearing: it reveals corpus gaps ("character slow left because only two left clips"). Sliders/rebake are Phase 6 commands.

## 3. Physics-driven animation

Phase 25 ships passive ragdoll (flip to dynamic, let gravity win). Phase 35 adds *powered* ragdoll — joints run motors that try to drive the body toward an authored reference pose. Characters get hit, spin, stagger, recover balance, stand back up — without authored hit-reacts per case.

### 3.1 Powered ragdoll

Each PhAT joint gains an optional PD drive targeting the animated pose rotation:

```rust
pub struct JointDrive {
    pub stiffness: f32,     // P gain
    pub damping:   f32,     // D gain
    pub max_torque: f32,    // saturation
    pub target_source: DriveTarget,
}
pub enum DriveTarget { AnimatedPose, Custom(BoneId) }
```

Per step:

```
torque = stiffness * (target_rot - current_rot) - damping * angvel
torque = clamp(torque, ±max_torque)
```

Stiffness gradient across body is authored: high on spine/hips, medium on shoulders/knees, low at wrists/ankles. The blend between powered and passive, per bone, is the runtime knob.

### 3.2 Hit reaction

On damage event the game passes an impulse. Runtime records it on the nearest `PhatBody`, reduces stiffness on joints by skeleton-graph distance from the hit (elbow hit weakens shoulder), then over `recovery_time` (0.6-1.2 s) ramps stiffness back to baseline. While reduced, physics dominates; authored pose is "suggested." Character staggers, recoils, returns. *Uncharted 4* / *RDR2* model. Replaces ~200 authored hit-reacts with one tuning curve.

### 3.3 Balance recovery

COM estimator per tick. If COM leaves support polygon between feet, a foot-placement controller steps to regain support. Small state machine: `Balanced → Stumbling → Fallen`. Ambitious, easy to get wrong, often unnecessary. Behind per-character toggle; default off.

## 4. Full-body IK

Phase 24 has per-chain IK. FBIK is a whole-skeleton solver: given constraints on hands, feet, head, COM, produce a consistent pose. Useful for climbing (hand + foot holds), sitting, whole-body pushing.

Constraint-based Jacobian-transpose solver with a COM preservation term. Slower than FABRIK (0.3-0.8 ms on a 30-bone skeleton) but handles multi-constraint cases FABRIK cannot.

```rust
pub struct FbikConstraint {
    pub bone: BoneId,
    pub target: Transform,
    pub weight: f32,
    pub kind: FbikKind,     // Position | PosRot | LookAt
}
pub struct FbikSolver {
    pub max_iters: u8,       // 8 default
    pub tolerance: f32,      // 0.5 cm
    pub preserve_com: bool,
    pub preserve_shape: f32, // pose-stiffness term
}
```

Exposed as a Phase 24 node `GraphNode::FullBodyIK { input, constraints, solver }`. Gated per graph; default off. Most characters do not need it.

## 5. Facial animation rig

Faces are hybrid: bones for jaw/eyes/lids + blendshapes (morph targets) for expression. FACS (Ekman) catalogs ~46 action units; a production rig ships 60-150 blendshapes mapped to those units plus asymmetric variants.

### 5.1 Asset

```rust
pub struct FacialRig {
    pub mesh: AssetGuid,
    pub skeleton: AssetGuid,            // face bones (jaw, eyes, lids)
    pub blendshapes: Vec<FaceBlendshape>,
    pub face_bones:  Vec<FaceBone>,
    pub facs_map:    Vec<FacsMapping>,  // AU → blendshape composition
}
pub struct FaceBlendshape {
    pub name: String,
    pub vertex_deltas: Vec<(u32, Vec3)>,  // sparse
    pub normal_deltas: Option<Vec<(u32, Vec3)>>,
}
pub struct FacsMapping {
    pub au: FacsUnit,
    pub composition: Vec<(BlendshapeId, f32)>,  // usually 1:1, sometimes 1:N
}
```

```
 Upper face (AU1/2/4 brow, AU5/6/7 eye)   Lower face (AU10/12/15 lip, AU17/20/25 chin)
                    │                                    │
                    └────────── 0..1 weights ────────────┘
                                    ▼
                        ┌───────────────┐     ┌──────────┐
                        │ Graph FACS    │◄────│ Lip sync │ (§6)
                        │ track         │     │ phonemes │
                        └───────────────┘     └──────────┘
```

### 5.2 DCC importer extensions

Phase 4 FBX/glTF importers gain:

- **FBX blendshape tracks** — Maya/MotionBuilder "shape deformer" channels → sparse vertex deltas.
- **Blender shape-keys** — glTF 2.0 `morph_targets`; honor `extras.facs_au` naming when present.
- **ARKit 52 preset** — if blendshape names match ARKit (`browInnerUp`, `jawOpen`, ...) auto-populate `facs_map`.

Unknown names ship as-is with empty `facs_map`; user authors mapping in the panel.

### 5.3 Facial Rig panel

```
┌─ Facial Rig: hero_face.rfacial ─────────────────────────────┐
│ [3D preview — neutral, drag to rotate]                      │
│ Blendshapes (86, searchable):                               │
│   brow_inner_up_L  [●───] 0.00   FACS AU1_L                 │
│   jaw_open         [─●──] 0.25   FACS AU26                  │
│ FACS AUs (46)  [Auto-map from ARKit names]                  │
│   AU1  → brow_inner_up_L/R (1.0)                            │
│   AU12 → lip_corner_up_L/R (1.0)                            │
│ Presets: Neutral / Smile / Anger / Surprise / Speech-O      │
└─────────────────────────────────────────────────────────────┘
```

Slider scrubs one blendshape; presets load combinations; FACS rows map AUs to blendshapes. Phase 6 commands.

### 5.4 Runtime evaluator

Per tick: graph evaluates facial track → per-blendshape weights + per-face-bone transforms → skinning applies blendshape deltas *before* bone skinning (blendshapes deform rest pose, bones then pose that mesh). Sparse deltas in a storage buffer; compute pass accumulates weighted deltas before the skin compute. 60-120 blendshapes at 500-2000 verts each: well under 0.1 ms at HD face density.

## 6. Phoneme-to-mouth lip sync

Input: audio + optional transcript. Output: time-series of mouth blendshape weights. 2026 SOTA is ML-based (JALI, Audio2Face); explicitly out of scope. We ship forced alignment + phoneme-to-viseme mapping.

### 6.1 Pipeline

```
audio + transcript ─► aligner ─► (phoneme, t0, t1)+ ─► viseme table ─► blendshape curves ─► .rlipsync
```

Aligner is a sidecar CLI (Montreal Forced Aligner or similar) — do not vendor weights. Without transcript, user integrates ASR externally.

Standard 14-viseme set (Preston Blair-ish): `sil AA AE AH AO B/M/P CH D/S/T EE F/V K L OO W`. Each maps to weighted blendshape combos, authored in the panel.

### 6.2 Lip Sync panel

```
┌─ Lip Sync: dialog_int_01.rlipsync ──────────────────────────┐
│ Audio int_01.wav  ▶   Transcript "Hello, how are you today?"│
│ Alignment [Re-run]:  0.00-0.12 sil  0.12-0.20 HH  0.20-0.35 EH  ...│
│ Waveform + phoneme track:                                   │
│   [ ░▒▓█▓▒░▒▓█▒░▒▓█▓▒░▓█▓▒░  waveform        ]              │
│   [  sil HH EH L OW , HH AW AA ...  phoneme lane ]          │
│ Viseme mapping:                                             │
│   HH → open(0.4) + jaw_open(0.2)                            │
│   EH → smile_slight(0.3) + jaw_open(0.3)                    │
│   OW → o_shape(0.8)                                         │
│ Smoothing [──●──] 0.2   [Rebake]  [Preview →]               │
└─────────────────────────────────────────────────────────────┘
```

Output `.rlipsync` = curve-per-blendshape track playing in lockstep with audio. Runtime: montage on facial slot, layered over idle facial animation.

## 7. Performance capture retargeting

Phase 19 retargets between engine skeletons. Phase 35 adds mocap file import.

- **`.bvh`** — Biovision Hierarchy. Text, legacy-but-common. Each line is a joint's Euler + translation per frame. Bakes into a Phase 19 clip on a generated skeleton; user maps via Phase 19 retarget table.
- **`.fbx` mocap tracks** — FBX containing animation on raw mocap skeleton (OptiTrack / Vicon / Movella). Importer identifies skeleton, bakes each track.

### 7.1 Cleanup UI

Raw mocap is noisy: foot-slide, jitter, drift. Phase 19 curve editor handles per-channel filtering; mocap-specific actions deserve first-class buttons:

```
┌─ Mocap Cleanup: take_034.rtimeline ─────────────────────────┐
│ Source vicon_take_034.fbx   1,842 frames @ 120 FPS          │
│ [Remove jitter (Butterworth fc=8 Hz)]                       │
│ [Lock feet to ground (plant detect thresh 0.02 m/s)]        │
│ [Align root to +Z forward]  [Trim silence]                  │
│ Foot-plant: 412/1842 planted (22%)  [Visualize]             │
│ [Bake clip]  [Open in Timeline →]                           │
└─────────────────────────────────────────────────────────────┘
```

Filters produce a new clip rather than mutating the raw import — raw stays as ground-truth on reimport. Each button is a Phase 6 command.

## 8. Animation compression

Large modern games ship 5-20 GB animation uncompressed. Industry reference is ACL (Animation Compression Library) — curve-fit keyframe compression, 8-10× ratio with imperceptible error at default settings. We ship the algorithm class, not ACL verbatim.

Per bone, per channel: (1) **quantize** — rotations 16-bit/component (quat drop-w reconstruct), translations 16-bit fixed-point in clip AABB, scales similar; (2) **error-bounded key reduction** — drop keys whose interpolation from neighbors is within per-bone tolerance measured in *skinned vertex space*; (3) **variable-bit encoding** — constant tracks → single key, high-frequency → more keys.

```rust
pub struct CompressedClip {
    pub duration: f32,
    pub bone_tracks: Vec<CompressedBoneTrack>,
    pub root_motion: Option<CompressedBoneTrack>,
}
```

Offline pass at asset-build time; runtime decompresses per frame per bone needed. Few ns per bone, negligible.

Per-project `CompressionProfile`: face bones tight (0.1 mm), fingers 0.2 mm, spine/limbs 0.5 mm default, tail/cloth bones looser.

## 9. Additive animations

Phase 24 has the `Additive` node. Phase 35 upgrades authoring: breathing, flinch, aim offsets, idle variations as first-class additive clips. Phase 19 clip editor gains "mark as additive, reference frame = 0." Graph layers them through `LayeredBlend`:

```
[base locomotion] ──┬──► LayeredBlend (upper-body mask) ──► output
                    │              ▲
[aim offset add] ───┘              │
                    ┌──────────────┘
[breathing add] ────┘
```

Convention: filenames ending `_additive` auto-toggle the flag on import.

## 10. Animation streaming

Modern characters have 500+ clips; loading all at spawn is wasteful. Piggyback on Phase 31 streaming. An `AnimationSet` bundles clips with a `LoadTrigger` (`Spawn | GameplayState(tag) | Distance(f32)`); the character manifest lists its sets.

```rust
pub struct AnimationSet { pub clips: Vec<AssetGuid>, pub priority: StreamPriority, pub preload: bool }
pub struct AnimationSetRef { pub set: AssetGuid, pub trigger: LoadTrigger }
```

Missing clips at tick time fall back to A-pose with a one-frame warning; graph does not stall. Same discipline as Phase 31 terrain streaming.

## 11. Performance budget

Shipping targets per platform tier:

| Feature               | Desktop hi | Desktop mid | Console mid | Mobile hi |
| --------------------- | ---------- | ----------- | ----------- | --------- |
| Motion match (1k db)  | 0.20 ms    | 0.35 ms     | 0.50 ms     | 1.20 ms   |
| Motion match (5k db)  | 0.35 ms    | 0.60 ms     | 0.90 ms     | n/a       |
| Powered ragdoll (14j) | 0.15 ms    | 0.25 ms     | 0.35 ms     | 0.60 ms   |
| FBIK (30 bones)       | 0.35 ms    | 0.60 ms     | 0.90 ms     | 1.80 ms   |
| Facial eval (80 bs)   | 0.08 ms    | 0.12 ms     | 0.18 ms     | 0.35 ms   |

Per character; Phase 11 parallelizes across characters. Every panel's status bar shows live cost against budget in PIE.

## 12. Build order

1. **Additive authoring + streaming** (§9, §10) — isolated, unlocks content teams immediately.
2. **Animation compression** (§8) — orthogonal; land early so teams work on compressed assets.
3. **Motion matching runtime** (§1) without editor — stub CLI bakes databases. Prove numbers first.
4. **Motion Matching panel** (§2.1) with PCA visualization.
5. **Mocap import + cleanup** (§7) — unblocks mocap stages.
6. **Facial rig** asset + importer + runtime (§5) — no lip sync yet.
7. **Facial Rig panel** (§5.3).
8. **Phoneme aligner + Lip Sync panel** (§6).
9. **Powered ragdoll** (§3.1-3.2) on Phase 25. Hit reactions ship as the killer app.
10. **Balance recovery** (§3.3) — optional, gated.
11. **FBIK** (§4) — optional graph node.

## Scope ❌

- ❌ **Real-time ML pose generation** (diffusion models, LLM-driven pose synthesis, latent-space motion models). Research, not near-term.
- ❌ **Neural motion synthesis** (DeepMimic-style learned controllers, MotionVAEs). Separate R&D phase.
- ❌ **Real-time facial mocap solver** (single-camera to rig). Use MetaHuman Animator, ARKit, Movella externally; import the result via the Facial Rig pipeline.
- ❌ **Generative animation from text prompts** ("make him wave and walk left"). Out of engineering scope.
- ❌ **Hand/finger IK rigs beyond basic two-bone.** Separate sub-phase; Phase 35 treats fingers as additive overrides or keyframed tracks.
- ❌ **Speech synthesis.** Lip sync consumes audio + transcript; does not generate speech.
- ❌ **Muscle / anatomy deformer systems** (Ziva-style). Offline DCC territory; runtime stays on blendshapes + linear skinning.
- ❌ **Crowd animation / instanced LOD animation.** Separate crowds phase on top of motion matching.
- ❌ **Cloth-aware animation authoring** (cloth driving pose decisions). Keep cloth downstream, per Phase 25.
- ❌ **Runtime motion-database editing** (adding clips at runtime). Databases are baked, immutable outside hot-reload.
- ❌ **Cross-skeleton motion matching** (one database drives many topologies). Retargeting bridges at the clip layer, not the database layer.

## Risks

- **Motion database curation cost.** Good corpus is a content burden, not programming. Phase 35 ships the tech; the game team ships the motion. Document "what makes a good corpus" as part of the deliverable.
- **KD-tree degeneracy at high dimensionality.** 48-D k-NN approaches linear scan untreated. PCA reduction is not optional; benchmark both paths and fail CI if query breaks budget.
- **Powered ragdoll tuning.** Stiffness curves are character-specific. Ship a "default humanoid" profile that is 70% right; expose the remaining 30% per-character.
- **Facial DCC interchange pain.** Every DCC exports blendshapes slightly differently. ARKit 52 preset handles one convention; document the recognized names.
- **Lip sync aligner dependency.** MFA / Kaldi are heavy. Sidecar CLI (like ffmpeg) keeps engine binary small and licensing clean.
- **Compression error regressions.** Per-clip "compare-to-uncompressed" button in the timeline is essential, not optional. Diffuse quality drops are hard to debug otherwise.
- **Streaming pop.** Graph state-enter awaits set load (stays in source state until target loaded). A longer transition beats a gameplay-visible A-pose pop.
- **FBIK instability at infeasible constraint sets.** Solver returns partial solution with per-constraint residuals; gameplay decides acceptability.
- **Hot reload of motion databases.** Rebaked `.rmmdb` has different indices. On reload reset every matcher to "re-query next tick"; don't map old to new. One-frame pop acceptable.
- **Determinism vs. motion matching.** FP k-NN has ordering subtleties under parallel execution. In networked games either query is deterministic (single-thread + stable tiebreak) or result is replicated. Phase 14 decides; Phase 35 honors.

## Exit criteria

Phase 35 is done when all of these hold:

- [ ] Humanoid character on motion matching + upper-body SM runs 10 minutes with no unintended cuts; average query ≤ 0.5 ms on mid-desktop at 1k-pose database.
- [ ] `.rmmdb` round-trips, bakes reproducibly, hot-reloads without crash.
- [ ] Motion Matching panel's PCA scatter renders 10k points at 60 Hz; clicking previews the frame.
- [ ] Powered-ragdoll character takes 500 N impulse, staggers, recovers to animated pose within 1.2 s; no penetrations over 100 sequential hits.
- [ ] FBIK solves three-constraint (two-hand + one-foot) within tolerance in ≤ 8 iterations, stable under ±5 cm target jitter at 60 Hz.
- [ ] Facial rig imports from Maya FBX with 52 ARKit-named blendshapes, auto-maps to FACS, renders correctly.
- [ ] Lip sync on English dialog: phoneme boundaries align within ±40 ms; mouth shapes read correctly.
- [ ] `.bvh` and mocap `.fbx` both import; foot-plant detection catches ≥ 90% of authored plants on a test take; baked clip plays on engine skeleton via Phase 19 retarget.
- [ ] Animation compression: ≥ 8× ratio on a 1000-clip library with max skinned-vertex error ≤ 0.5 mm on default profile.
- [ ] Additive breathing + aim-offset compose on motion-matched locomotion without drift over 10 minutes.
- [ ] Character with 400 clips spawns in under 50 ms loading only the `spawn` set (~30 clips); combat set streams in within 200 ms on gameplay state change.
- [ ] All four new panels route mutations through Phase 6; Ctrl+Z never broken.
- [ ] Every feature respects the §11 budget table; CI fails when budget exceeded by ≥ 20%.
- [ ] `rustforge-core` builds without the `editor` feature; panel code gated, runtime not.
