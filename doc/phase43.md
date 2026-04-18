# Phase 43 — Procedural Characters

Phase 35 gave us a facial rig authoring pipeline — FACS blendshape+bone hybrid, Maya/Blender importer, runtime evaluator, phoneme lip sync. Phase 24 gave us the animation graph, FBIK, blend spaces, montages. Phase 25 gave us cloth sim and ragdoll on skinned meshes. Phase 41 gave us a grooming system: guide curves, interpolated strand expansion, cards, LOD, physics. Each of those phases assumed *someone had already authored a character*. Phase 43 is the someone.

This is the MetaHuman-equivalent phase — a parametric biped character generator that produces a fully rigged, groomed, clothed character from a panel of sliders in about the time it takes to open an asset editor. The output is not photorealistic scan data (see §Scope ❌) and it is not text-to-character (that's Phase 39). It is a deterministic, slider-driven, artist-steerable generator over a shipped base-mesh + morph + texture library, welded to the existing rig, clothing, and groom systems so that the character works with every downstream runtime on day one.

Upstream dependencies: Phase 2 (reflection, RTT — slider state is reflected), Phase 6 (undo stack — every slider drag coalesces into a command), Phase 8 (AssetEditor trait + isolated preview viewport pattern — §9 reuses it wholesale), Phase 19 (skeleton + retargeting), Phase 24 (animation graph, FBIK auto-config), Phase 25 (cloth sim on clothing layer), Phase 35 (FACS facial rig, auto-configured from generated face topology), Phase 40 (PCG — consumes character collections), Phase 41 (groom asset, preset library).

## Goals

By end of Phase 43:

1. **Parametric body generator** — slider-driven morph over masculine/feminine/androgynous base meshes; age, height, weight, proportions.
2. **Parametric face generator** — slider-driven morph over FACS-compatible face topology; jaw, cheekbones, nose, brow, eyes, lips.
3. **Base mesh library** — three shipped base meshes (M/F/A) with fully open morph blending between them.
4. **Rig auto-setup** — skeleton proportions retargeted to morph, FBIK (Phase 24) wired, FACS rig (Phase 35) wired, deterministic.
5. **Texture library** — skin (albedo/normal/roughness/SSS), eyes, hair colors; user-extensible via drop-in assets.
6. **Clothing layer** — layered meshes that drape on the body morph; shipped wardrobe (casual/formal/armor/sci-fi); Phase 25 cloth compatible.
7. **Groom integration** — Phase 41 hair/beard grooms with shipped presets (short, long, curly, bald, etc.).
8. **Runtime LOD** — mesh LODs, texture resolution tier, strand-count reduction, SSS toggle by distance and tier.
9. **Same-rig guarantee** — every generated character shares identical bone names and hierarchy; graphs and motion databases work unmodified.
10. **Character Designer panel** — Body / Face / Groom / Clothing tabs with live isolated preview.
11. **`.rchar` asset** — stores generation *parameters*, not baked mesh; tiny file; regenerable.
12. **Bake-to-static** — option to bake the resolved mesh/skeleton/textures for ship or external export.
13. **Randomizer** — "Randomize all / face / clothing" buttons with seed control for NPC crowds.
14. **`.rcharset` collection** — N characters with shared rules, consumed by Phase 40 PCG.
15. **Performance budget** — documented generation cost + runtime cost per LOD tier.

## 1. The `.rchar` asset — parameters, not baked mesh

The single most important design decision in this phase is that `.rchar` stores only the slider state plus library asset references. It does *not* store the morphed mesh, the merged skeleton, the composited textures, or the draped clothing. A villager `.rchar` is about 2 KB. Regenerating it on load is a few milliseconds on a background job.

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct CharacterAsset {
    pub version: u32,
    pub base_blend: BaseBlend,              // M/F/A barycentric
    pub body: BodyParams,
    pub face: FaceParams,
    pub skin: SkinParams,                   // texture refs + tint
    pub eyes: EyeParams,
    pub groom: GroomSlot,                   // preset ref + color override
    pub clothing: Vec<ClothingLayer>,       // ordered outer -> inner
    pub rig_overrides: RigOverrides,        // user-tweaked bone lengths
    pub seed: u64,                          // reproducible randomization
    pub bake: Option<BakeRef>,              // Some(...) if user chose to bake
}

pub struct BaseBlend { pub masculine: f32, pub feminine: f32, pub androgynous: f32 } // sum = 1
```

Why parameters: (a) 2 KB instead of 40 MB; (b) re-export on base-library update without per-asset migration; (c) trivially diffable in source control; (d) composable with Phase 40 PCG, which wants to synthesize thousands of variants without storing thousands of meshes; (e) `seed` + deterministic morph = bit-identical regeneration.

Bake-to-static is still offered (§12) for users who want to ship fixed meshes to a closed build target, or hand off to an external DCC. Baking produces a sibling `.rstaticchar` with the full mesh/textures/rig baked flat.

## 2. Base mesh library — M / F / A

Three shipped base meshes, identical in topology, UV, and bone hierarchy. Only the *vertex positions* differ. That topological agreement is the whole engineering trick: any vertex-space morph applies to all three the same way, and barycentric blends between them produce a continuum with no seams.

```rust
pub struct BaseMeshLibrary {
    pub masculine:   MeshAssetGuid,   // topology-locked to schema
    pub feminine:    MeshAssetGuid,
    pub androgynous: MeshAssetGuid,
    pub topology_hash: u64,           // CI asserts all three match
}
```

Morphology framing: the sliders are ethnicity-agnostic. The generator exposes *morphological* controls (jaw width, cheekbone prominence, nasal bridge angle, skin melanin, etc.) — it does not expose "ethnicity presets" that gate which combinations are reachable. A user can reach any point in the morphology space from any starting slider.

Topology is locked: ~28k tris body, ~12k tris head, 4 UV islands, 78 bones in the default rig, 412 FACS-compatible blendshape targets reserved on the head. A CI check refuses to ship a base mesh that drifts off the schema.

## 3. Body morph system

Body morphs are an ordered stack over the base-blend result.

```rust
pub struct BodyParams {
    pub age: f32,           // 0.0 (20yo) ... 1.0 (80yo), soft-clamped
    pub height_cm: f32,     // 140 ... 220, linear bone scale on spine/legs
    pub weight: f32,        // BMI-ish, drives fat-pass morphs
    pub muscle: f32,        // 0 ... 1, drives muscle-pass morphs
    pub torso_length: f32,  // -1 ... +1
    pub leg_length:   f32,
    pub shoulder_width: f32,
    pub hip_width:      f32,
    pub neck_length:    f32,
}
```

Height is *not* a morph — it is a bone-length change, applied during rig auto-setup (§5). Weight and muscle are paired morph passes; interaction is authored (heavy+muscular ≠ heavy × muscular). Age drives a joint morph-set (sagging, jowling, skin looseness via normal-map blend) plus a bone-curvature term for stooped posture.

Ordering matters — we apply *structural* morphs (proportion, height frame) first because subsequent morphs are authored against post-structural topology.

```rust
fn apply_body(base: &Mesh, p: &BodyParams) -> Mesh {
    let mut m = base.clone();
    apply_morph(&mut m, TORSO_LENGTH, p.torso_length);
    apply_morph(&mut m, LEG_LENGTH,   p.leg_length);
    apply_morph(&mut m, SHOULDER,     p.shoulder_width);
    apply_morph(&mut m, HIP,          p.hip_width);
    apply_morph(&mut m, WEIGHT,       p.weight);
    apply_morph(&mut m, MUSCLE,       p.muscle);
    apply_morph(&mut m, AGE_BODY,     p.age);
    m
}
```

Each `apply_morph` is a linear vertex delta add. Non-linear interactions (weight×muscle) are encoded as auxiliary morph targets keyed on paired slider values — authored offline, blended at runtime.

## 4. Face morph system

The head shares topology with the FACS rig from Phase 35. That is the hinge: identity morphs live on the same vertex set that FACS expressions animate. A generated character inherits the full FACS repertoire for free.

```rust
pub struct FaceParams {
    pub jaw_width:    f32, pub jaw_length: f32, pub chin_point: f32,
    pub cheek_prominence: f32, pub cheek_height: f32,
    pub nose_length: f32, pub nose_width: f32, pub nose_bridge: f32, pub nose_tip: f32,
    pub brow_height: f32, pub brow_ridge:  f32,
    pub eye_size: f32, pub eye_spacing: f32, pub eye_tilt: f32, pub eye_depth: f32,
    pub lip_fullness_upper: f32, pub lip_fullness_lower: f32, pub mouth_width: f32,
    pub forehead_height: f32, pub skull_width: f32,
}
```

20 sliders, each bound to a morph target authored in Phase 35's face modeling tool. The sliders are signed (`-1..+1`) — zero is the base mean face. Randomization (§11) samples each slider from a truncated normal, not uniform, because uniform distribution over face space produces mostly ugly outliers.

We *do not* ship ethnicity-preset packs. A user can save their own slider preset as a `.rcharpreset` (see §11.1) and share it, and the built-in randomizer can be seeded with a user-supplied preset library for style-consistent crowds.

## 5. Rig auto-setup

Called after body morphs resolve. Walks the template skeleton, rescales bone lengths to match morphed geometry, re-weights vertex skinning where the changes are significant, and wires downstream rigs.

```rust
pub struct GeneratedRig {
    pub skeleton: Skeleton,         // bone names stable across all generated chars
    pub skin:     SkinBinding,      // same weights as base, re-normalized
    pub fbik:     FbikConfig,       // Phase 24 FBIK node config
    pub facs:     FacsRig,          // Phase 35 FACS rig binding
    pub ragdoll_bodies: Vec<RagdollProxy>, // Phase 25 PhAT — rescaled with bone lengths
}

fn build_rig(base_rig: &Skeleton, body: &BodyParams, face_topology: &Mesh) -> GeneratedRig {
    let mut sk = base_rig.clone();
    scale_spine(&mut sk, body.height_cm, body.torso_length);
    scale_limbs(&mut sk, body.height_cm, body.leg_length);
    adjust_shoulder_hip(&mut sk, body.shoulder_width, body.hip_width);
    let fbik  = auto_fbik(&sk);
    let facs  = bind_facs(face_topology, &FACS_TEMPLATE);
    let skin  = renormalize_skin(&sk, &BASE_SKIN);
    let prox  = scale_ragdoll(&sk, &BASE_RAGDOLL);
    GeneratedRig { skeleton: sk, skin, fbik, facs: facs, ragdoll_bodies: prox }
}
```

**Same-rig guarantee.** Bone names and hierarchy are identical across every generated character. That's the property Phase 24 animation graphs and Phase 35 motion databases depend on — a single graph drives a whole village. We do *not* generate novel skeleton topologies; see §Scope ❌.

## 6. Texture library

Skin is the hard one. Four channels per variant: albedo, tangent-space normal, roughness, SSS profile id.

```rust
pub struct SkinSet {
    pub albedo:    TextureGuid,
    pub normal:    TextureGuid,
    pub roughness: TextureGuid,
    pub sss_profile: SssProfileId,
    pub melanin_tint: Vec3,      // user pigmentation slider modulates albedo
    pub freckle_intensity: f32,
    pub age_blend: f32,           // blended toward "_aged" variant of same set
}
```

Shipped base: 12 skin sets spanning melanin low-to-high. The melanin tint is a runtime multiplier so the user slider produces a continuum rather than 12 steps. Age pushes toward an aged variant (wrinkle normals, pigmentation spots) authored per set.

Eyes, teeth, hair color are smaller libraries (6–12 variants each). Everything is under `/content/characters/library/` and user-extensible — a project-local drop-in with matching `*.skinset.ron` manifest files gets picked up on asset scan (Phase 5 file watcher).

## 7. Clothing layer

Clothing meshes drape on top of the morphed body. Authored against the base mesh, projected onto the morphed result at bind time.

```rust
pub struct ClothingLayer {
    pub mesh:  ClothingMeshGuid,    // authored against base, with skin weights
    pub slot:  ClothingSlot,        // Torso / Legs / Feet / Head / Hands / Outer
    pub material: MaterialGuid,
    pub sim:      Option<ClothSimConfig>,  // Phase 25 cloth; None = rigid skinned
    pub mask_body: BodyMaskId,      // which body verts to hide under this layer
}
```

Drape pipeline: (1) project clothing verts onto base body → closest-point + offset; (2) re-evaluate against morphed body with the stored offsets → clothing follows morph; (3) if `sim` is set, register with Phase 25 cloth on play; (4) body-vertex masking hides skin that would z-fight through the clothing.

Shipped wardrobe: casual (t-shirt, jeans, hoodie, sneakers), formal (shirt, trousers, jacket, dress, heels, flats), armor (chest plate, pauldrons, greaves, helm), sci-fi (bodysuit, boots, gloves, visor). ~40 items, mix-and-match across slots. Same-topology requirement does not apply to clothing — each piece authors its own mesh.

## 8. Groom integration

Groom slots reference Phase 41 groom assets directly. No duplication of groom machinery here.

```rust
pub struct GroomSlot {
    pub scalp:  Option<GroomPresetGuid>,   // Phase 41 preset
    pub brows:  Option<GroomPresetGuid>,
    pub beard:  Option<GroomPresetGuid>,
    pub lashes: Option<GroomPresetGuid>,
    pub color:  HairColor,                 // RGB + root/tip gradient
    pub length_scale: f32,                 // per-slot override 0.5..1.5
}
```

Shipped presets: 8 scalp (buzz, short, medium, long, curly, braided, bald, undercut), 4 brow (thin/med/thick/bushy), 6 beard (none, stubble, short, medium, long, goatee), lashes default. Presets are just `.rgroom` assets from Phase 41 — users can drop their own into `content/characters/library/grooms/`.

Scalp groom binds to skull-cap UV of the head mesh, which is shared across generated characters (topology lock, §2), so a preset works on every character without per-instance retargeting.

## 9. Character Designer panel

Reuses Phase 8's `AssetEditor` scaffolding wholesale — dockable, reflection-driven for simple fields, isolated preview viewport, dirty-tracking, undo via Phase 6 commands.

```
┌─ Character Designer ─ [villager_01.rchar] * ─────────────────────────────┐
│ [ Body ] [ Face ] [ Groom ] [ Clothing ]              Preview: [■] [⟲]   │
│ ┌──────────────────────────┐ ┌────────────────────────────────────────┐ │
│ │ Base Blend               │ │                                        │ │
│ │   Masculine    [▓▓▓░░]   │ │                                        │ │
│ │   Feminine     [░░▓▓▓]   │ │             (isolated preview           │ │
│ │   Androgynous  [░▓▓░░]   │ │              viewport — character      │ │
│ │                          │ │              on turntable, three-       │ │
│ │ Body                     │ │              point light, neutral       │ │
│ │   Age          [▓▓░░░]   │ │              pose or user-chosen        │ │
│ │   Height cm    [  178 ]  │ │              graph state)               │ │
│ │   Weight       [▓▓▓░░]   │ │                                        │ │
│ │   Muscle       [▓▓░░░]   │ │                                        │ │
│ │   Torso len    [░░▓░░]   │ │                                        │ │
│ │   Leg len      [░░▓░░]   │ │                                        │ │
│ │   Shoulders    [░░▓▓░]   │ │                                        │ │
│ │   Hips         [░░▓░░]   │ │                                        │ │
│ │   Neck         [░░▓░░]   │ │                                        │ │
│ │                          │ │                                        │ │
│ │ [Randomize Body] [Reset] │ │                                        │ │
│ └──────────────────────────┘ └────────────────────────────────────────┘ │
│ Seed: 0xA17F9C   LOD: [Cinematic]  [Randomize All] [Bake...] [Save]      │
└──────────────────────────────────────────────────────────────────────────┘
```

The preview viewport is the Phase 8 pattern: its own egui paint callback, its own camera, its own lighting rig, one-draw-call-per-frame on slider change, skipped when the tab is occluded. Pose can be neutral A-pose, T-pose, or any Phase 24 graph; the default uses a built-in locomotion graph so clothing and hair look alive.

Tab switching (Body/Face/Groom/Clothing) doesn't rebuild anything — all four tabs are views into the same `CharacterAsset` and the same live generated mesh.

## 10. Runtime LOD

Four tiers. The generated character carries enough information to dispatch to the right tier at runtime without per-character authoring.

| Tier | Mesh | Texture | Groom | SSS | Use |
|------|------|---------|-------|-----|-----|
| Cinematic | LOD0 full | 4K | strands | on | cutscenes, hero shots |
| High | LOD1 | 2K | strands thin | on | third-person player, close NPCs |
| Medium | LOD2 | 1K | cards | off | mid-distance NPCs |
| Low | LOD3 | 512 | single-card | off | crowd, far |

LOD0–LOD3 meshes are generated at bake time by Phase N (mesh simplification — already exists). Strand→card swap lives in Phase 41. Texture mip floor is set in the material. SSS toggle drops the shader path to a plain diffuse. The `LODSelector` system picks tier per frame from distance + platform-tier budget.

## 11. Randomizer

Seeded PRNG over slider space. Three entry points: full, face-only, clothing-only.

```rust
pub fn randomize(ch: &mut CharacterAsset, scope: RandScope, seed: u64) {
    let mut rng = Xoshiro256StarStar::from_seed_u64(seed);
    if scope.contains(BODY)     { randomize_body(&mut ch.body, &mut rng); }
    if scope.contains(FACE)     { randomize_face(&mut ch.face, &mut rng); }
    if scope.contains(SKIN)     { randomize_skin(&mut ch.skin, &mut rng); }
    if scope.contains(GROOM)    { randomize_groom(&mut ch.groom, &mut rng); }
    if scope.contains(CLOTHING) { randomize_clothing(&mut ch.clothing, &mut rng); }
    ch.seed = seed;
}
```

`randomize_face` samples each slider from a truncated normal (μ=0, σ=0.35, clamped to ±1). Uniform sampling produces caricatures; truncated normal produces recognizable-looking humans with variation concentrated near the mean. Body uses a mix — metric fields (height) from a height-distribution prior, morphology fields from truncated normal.

### 11.1 Presets

`.rcharpreset` captures a subset of a `.rchar` (face-only, body-only, wardrobe-only, full). Drop presets in `content/characters/presets/` and they become randomizer draws: "Randomize Face" can be configured to pick *from* the preset set rather than the raw distribution, which is how you get style-consistent fantasy elves or stylized anime faces without retraining anything.

## 12. Bake-to-static

A button in the panel. Resolves the full pipeline, writes a `.rstaticchar` sibling asset containing: fully morphed mesh at all LODs, baked skeleton with final bone lengths, composited textures (skin tint flattened, freckles painted in), materialized clothing layers as skinned submeshes, groom resolved to strands or cards per LOD. ~40–120 MB depending on LOD count. Useful for: external DCC handoff, closed-platform ship where build size is fixed, determinism pins for cinematics.

The original `.rchar` is retained; bake is one-way but reversible in the sense that you can re-edit the `.rchar` and re-bake.

## 13. `.rcharset` — character collection asset

A group of characters with shared rules. The canonical use case is "village NPCs": 50 generated characters, same wardrobe palette, same age distribution, seed range reserved so re-runs are stable.

```rust
#[derive(Reflect, Serialize, Deserialize)]
pub struct CharacterSetAsset {
    pub version: u32,
    pub count: u32,
    pub seed_base: u64,
    pub rules: SetRules,       // age range, gender mix, wardrobe pool, groom pool
    pub baseline: CharacterAsset,   // "average" — randomization perturbs this
    pub overrides: Vec<(u32, CharacterAsset)>, // user-pinned slots
}
```

Generation: for `i in 0..count`, seed `seed_base ^ i`, draw a `CharacterAsset` by perturbing `baseline` within `rules`, except when `i` is in `overrides` — then use the pinned asset. Deterministic, shareable, re-editable.

## 14. PCG integration (Phase 40)

PCG graphs can consume a `.rcharset` as a point-spawner payload. The PCG node `SpawnFromCharacterSet` emits an actor per point, resolving character index `(point_index) mod set.count`, so a village PCG graph over a splat of points produces a populated village in one evaluate. The generated character is a regular actor — it uses the standard animation graph, cloth, groom, and ragdoll stacks.

```rust
// Phase 40 node
pub struct SpawnFromCharacterSet {
    pub set: AssetGuid,          // .rcharset
    pub anim_graph: AssetGuid,   // .ranimgraph applied to all spawned
    pub lod_policy: LodPolicy,
}
```

No new runtime is introduced for PCG — character generation is fast enough at load that we resolve at spawn and pool the resulting meshes in the normal actor instance pool.

## 15. Performance budget

Generation cost (one character, release build, 2024 desktop reference):

- Body morph eval: 0.4 ms
- Face morph eval: 0.3 ms
- Rig auto-setup: 0.8 ms
- Clothing drape (per layer): 0.2 ms × N
- Groom instancing: 0.6 ms
- Texture composite (skin tint): 1.2 ms GPU
- **Total for a typical NPC (~4 clothing layers, 1 scalp + 1 brow groom): ~4 ms**

A 50-NPC village generates in ~200 ms, easily a background load-time job. Runtime cost beyond generation is standard animated-character cost — no per-character overhead from procedural origins.

## 16. Build order, Scope ❌, risks, exit

### Build order

1. Base mesh library + morph apply system (§2, §3 first half) — assert topology lock in CI.
2. Face morph system (§4) against FACS-compatible template from Phase 35.
3. Full body morph system (§3) including age and weight×muscle interactions.
4. Rig auto-setup (§5) — skeleton scaling, FBIK wiring, FACS binding, ragdoll scaling, same-rig CI assertion.
5. Texture library (§6) — skin sets, eye sets, hair colors, user-extensible scan path.
6. Clothing layer (§7) — drape + masking + Phase 25 cloth hookup.
7. Groom integration (§8) — Phase 41 preset binding to skull cap UV.
8. Character Designer panel (§9) — Phase 8 AssetEditor, isolated preview, four tabs, live regen.
9. Randomizer (§11) with preset library support.
10. `.rcharset` collection asset (§13).
11. Phase 40 PCG integration node (§14).
12. Bake-to-static (§12).
13. LOD runtime polish (§10) — tier switching, strand→card swap, SSS toggle.

### Scope ❌

- Photorealistic scan-based characters (MetaHuman Creator territory — users import MH via Phase 45 migration).
- Quadrupeds — auto-rig is biped-only; quadruped generator is a separate future phase if justified.
- IK retarget UI for novel skeletons — generated characters share one rig topology; arbitrary-topology retarget already belongs to Phase 19.
- ML-based character generation from text prompts — that is Phase 39.
- Face-from-photo — photogrammetric identity capture is out of scope.
- Custom skeleton topology generation — the rig is parametric in bone *lengths*, not bone *set*.

### Risks

- **Topology drift** between base meshes breaks morph continuity. Mitigation: topology-hash CI gate (§2), no base-mesh merge without matching hash.
- **Slider space too wide** produces caricatures in the randomizer. Mitigation: truncated-normal sampling (§11), preset libraries as a style filter, per-slider σ tuning in the generator config.
- **Clothing clipping** through bodies with extreme morphs (weight+muscle). Mitigation: body-vertex masking (§7), QA preset library of "extreme morph" characters run against every clothing item before ship.
- **Groom mismatch** on skull-cap variants. Mitigation: skull-cap UV is in the topology lock; all grooms authored against that UV always bind.
- **Bake size inflation** — if every NPC is baked, build size explodes. Mitigation: `.rchar` is the default, bake is opt-in (§12), docs push the parametric path.
- **Determinism** across runs for `.rcharset`. Mitigation: `seed_base ^ i` addressing, explicit PRNG choice (Xoshiro256\*\*), no floating-point nondeterminism in morph apply (fixed-order accumulation).

### Exit criteria

- Designer can open a fresh `.rchar`, drag sliders, get a clothed, groomed, rigged character in a preview viewport in under 100 ms per slider change.
- "Randomize All" produces a recognizably human character every time across 1000-seed sweep, no caricature outliers past σ=3.
- A Phase 24 animation graph authored against the base rig plays on any generated character without modification.
- A Phase 35 motion matching database runs on any generated character without re-baking.
- A `.rcharset` of 50 characters regenerates bit-identically on reload.
- Phase 40 PCG village demo spawns a populated village from one `.rcharset` + one point-cloud evaluate.
- Bake-to-static round-trips through an external DCC (Blender import, re-export) without rig breakage.
- 2 KB average `.rchar` file size.
- CI asserts base-mesh topology lock and same-rig guarantee on every commit to the character-library crate.
