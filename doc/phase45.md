# Phase 45 — Migration & Interop

Every prior phase assumed a clean slate — a team starting a new project in RustForge. That assumption is wrong for roughly every studio with more than one existing title. They have a Unity content library a decade deep, an Unreal Blueprint codebase wired into a shipping game, a Godot prototype they want to graduate, or a Blender scene that is the only authoritative source of an environment. "Rewrite your content from scratch to try our engine" is a conversation-ender. Phase 45 is not a scorecard category — nobody grades engines on how well they import Unity scenes — but it is the single most common real-world blocker to adoption. A studio evaluating RustForge will ask, on day one, "what happens to our existing project?" The honest answer is "most of it can come across; some of it can't; here is a report that tells you exactly which." This phase builds that pipeline.

The framing matters. This is not "RustForge can run Unity projects." That is a lie nobody should tell. This is "RustForge can import the static, declarative parts of a Unity / Unreal / Godot / Blender project, flag the behavioral parts as manual-port work, and produce a tracked migration report you can burn down over a sprint." Everything else — scripts, Blueprints, plugin code, runtime behavior parity — is explicitly out of scope.

## Goals

By end of Phase 45:

1. **Unity project importer** reads `.unity`, `.prefab`, `.asset`, and `ProjectSettings/` and emits an RF project with entities, components, prefabs, and material graphs where mappable.
2. **Unreal project importer** via an out-of-process headless UE exporter (recommended path) produces `.umap`/`.uasset`-derived RF scenes with actors, components, and mappable material nodes.
3. **Godot project importer** parses `.tscn` / `.tres` directly (text formats, no binary dance) and converts scene trees to entity hierarchies.
4. **Blender exporter plugin** runs Blender in headless mode with a Python adapter and extends Phase 5's importer pipeline with native `.blend` support.
5. **Asset conversion layer** copies textures, generates `.meta` sidecars, translates material nodes to Phase 20 graphs with Custom WGSL fallback, and retargets animation clips to Phase 19 `.rtimeline`.
6. **Migration report** — per-file markdown detailing what migrated cleanly, what needs manual review, and what was unsupported.
7. **Bidirectional lightweight export** — RF scene → Unity / Unreal project as glTF plus JSON metadata sidecars, for studios straddling engines.
8. **Import wizard UI** — "Import Project" panel: pick source engine, source root, RF target project, preview, commit.
9. **Post-migration cleanup helper** — panel listing flagged entities with "open external reference", "mark resolved", and notes.

## 1. The honest framing — what "migration" does and doesn't mean

Every engine migration tool ever shipped has over-promised and under-delivered. Unity's own Unreal-to-Unity project converter exists, limps, and is quietly disowned. This phase has to be honest with users about what the tool is.

**What migration means in Phase 45:**
- Declarative scene content — transforms, mesh references, light parameters, physics bodies, collider shapes, audio source config — converts.
- Assets — textures, meshes, audio clips — copy in via Phase 5 importers with `.meta` sidecars generated.
- Materials — best-effort node translation, falling back to Custom WGSL nodes (Phase 20 §3) containing the original shader source as a comment for manual port.
- Animation clips — keyframes extracted into Phase 19 `.rtimeline` format with retarget hints where skeletons differ.
- Prefabs / prefab variants — map to RF prefabs (Phase 4 §5); overrides drop to Phase 4.5 override format where supported.

**What migration does not mean:**
- Scripts. A Unity `MonoBehaviour` or an Unreal Blueprint is behavior, not content. Phase 45 flags these and does not attempt to port them. AI-assisted hand-translation is a separate tool, not a phase.
- Runtime parity. Physics tuned for PhysX at 90 Hz will not feel identical on Rapier at 60 Hz. Documented loudly on the landing page.
- Plugin code. Unity native plugins, UE C++ modules, Godot GDExtensions — all use engine-native APIs that don't port. Flagged, never auto-converted.
- Legacy engines. Source, id Tech, CryEngine, custom in-house formats — out of scope. If demand emerges, plugin authors can publish third-party importers via Phase 11's plugin API.

If a user leaves the import wizard thinking Phase 45 is going to move their whole project, the tool has failed. The wizard's first screen is a paragraph of prose saying exactly this, and the user clicks Continue.

## 2. Crate layout

```
crates/rustforge-migrate/            # new crate, editor-only
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── common/
    │   ├── mod.rs
    │   ├── report.rs              # MigrationReport + markdown writer
    │   ├── asset_copy.rs          # external asset -> assets/ + .meta
    │   ├── material_translate.rs  # node-by-node best-effort
    │   └── anim_translate.rs      # clip -> .rtimeline
    ├── unity/
    │   ├── mod.rs
    │   ├── yaml.rs                # Unity YAML (custom dialect) parser
    │   ├── scene.rs               # .unity -> SceneFile
    │   ├── prefab.rs              # .prefab -> PrefabFile
    │   └── component_map.rs       # Unity type -> RF component
    ├── unreal/
    │   ├── mod.rs
    │   ├── exporter_rpc.rs        # out-of-process UE headless dance
    │   ├── json_intermediate.rs   # UE's exported JSON schema
    │   └── actor_map.rs           # UE actor -> RF entity
    ├── godot/
    │   ├── mod.rs
    │   ├── tscn.rs                # .tscn text parser
    │   ├── tres.rs                # .tres resource parser
    │   └── node_map.rs            # Godot node -> RF entity
    ├── blender/
    │   ├── mod.rs
    │   ├── adapter.py             # ships inside the crate; runs in Blender
    │   └── driver.rs              # spawn blender --background, read JSON
    └── bidirectional/
        ├── mod.rs
        ├── unity_out.rs           # RF -> Unity package (GLTFFast)
        └── unreal_out.rs          # RF -> UE project (datasmith-lite)
```

The crate lives under the editor feature. Nothing in the migration pipeline runs at game runtime, and `rustforge-core` gains zero new dependencies.

## 3. Godot first — easiest path proves the pipeline

Build order starts with Godot for one reason: `.tscn` and `.tres` are line-oriented text formats that a competent Rust parser handles in a day. That means Phase 45's shared infrastructure — the report writer, asset copy, material translator, the wizard UI — can be wrung out against a real importer before the Unity YAML dragon and the UE binary dragon wake up.

```rust
pub struct TscnNode {
    pub name: String,
    pub ty: String,              // "Node3D", "MeshInstance3D", "RigidBody3D", ...
    pub parent: Option<String>,  // path like "." or "Enemies/Goblin"
    pub props: BTreeMap<String, TscnValue>,
    pub groups: Vec<String>,
}

pub fn import_tscn(path: &Path, ctx: &mut ImportCtx) -> Result<SceneFile> {
    let nodes = parse_tscn(path)?;
    let mut scene = SceneFile::new();
    for n in &nodes {
        let entity = scene.spawn(SceneId::random());
        map_godot_node(&n, entity, &mut scene, ctx)?;
    }
    Ok(scene)
}
```

Godot node → RF entity mapping is mostly one-to-one for 3D nodes. `Node3D` → `Transform`, `MeshInstance3D` → `Transform + MeshRenderer`, `DirectionalLight3D` → `DirectionalLight`, `RigidBody3D` → `RigidBody` + collider, `AudioStreamPlayer3D` → `AudioSource`. 2D nodes and UI (`Control`) get flagged as "unmapped — 2D support is a separate track" since Phase 45 focuses on 3D.

**GDScript is flagged.** For every node with an attached `.gd` script, the report gets an entry:

```
[manual-port] scenes/player.tscn
    Node: Player/PlayerController
    Attached script: res://scripts/player.gd
    Reason: GDScript is not auto-portable to RF. Port to WASM script (Rust/AssemblyScript).
    See: docs/scripting-migration-guide.md
```

The user gets the scene geometry and hierarchy; they do not get the behavior. That's the whole deal and it's said explicitly.

## 4. Unity — YAML, the long tail

Unity's `.unity` and `.prefab` files are YAML, but with Unity-specific `%TAG !u! tag:unity3d.com,2011:` headers and `fileID` / `guid` references that thread through the project. Use a proper YAML parser (`serde_yaml` won't handle the tag syntax directly — write a preprocessor that strips/records Unity tags, then feed the rest to serde).

```
--- !u!1 &1234567890
GameObject:
  m_Component:
    - component: {fileID: 1234567891}
    - component: {fileID: 1234567892}
  m_Name: Player
--- !u!4 &1234567891
Transform:
  m_LocalPosition: {x: 0, y: 0, z: 0}
  ...
```

Each `fileID` is a local reference within the file; cross-file references combine a `guid` (from `.meta`) with a `fileID`. The importer builds a two-level resolution table: `guid → file` and then per-file `fileID → GameObject`.

**Component map (core subset):**

| Unity component     | RF component                        | Notes                                  |
|---------------------|-------------------------------------|----------------------------------------|
| `Transform`         | `Transform`                         | Row-vector → column-vector math fix    |
| `MeshFilter` + `MeshRenderer` | `MeshRenderer` + `MaterialRef` | Merge into one RF component     |
| `SkinnedMeshRenderer` | `SkinnedMeshRenderer`             | Bone refs by `fileID` → `SceneId`      |
| `Rigidbody`         | `RigidBody`                         | Mass / drag / gravity copy             |
| `BoxCollider`       | `BoxCollider`                       | Center + size                          |
| `SphereCollider`    | `SphereCollider`                    |                                        |
| `CapsuleCollider`   | `CapsuleCollider`                   | Direction axis preserved               |
| `MeshCollider`      | `MeshCollider`                      | Mesh GUID remap                        |
| `AudioSource`       | `AudioSource`                       | Rolloff curves flagged                 |
| `Light`             | `DirectionalLight` / `PointLight` / `SpotLight` | Type enum switch       |
| `Camera`            | `Camera`                            | Projection mode + clipping             |
| `Animator`          | `AnimationGraphInstance` (Phase 24) | Controller asset flagged for review   |
| `MonoBehaviour`     | flagged                             | User script, manual port               |

Anything not in the table goes to `[unmapped]` in the report with the Unity type name preserved. Users filter the report by `[unmapped]` and decide per-case.

**Material translation.** Unity Standard, URP Lit, and HDRP Lit all have similar-enough PBR inputs that a master-graph template works: albedo tex, metallic tex, normal tex, smoothness, emission. Phase 20 master node accepts these directly. Custom shaders and Shader Graphs that don't match the template become a single Custom WGSL node with the original shader source inlined as a `/* unity-shader: ... */` comment.

**Animator Controllers.** The `.controller` asset maps conceptually to Phase 24's `.ranimgraph`, but the state-machine semantics differ enough (transition interruption, any-state, sub-state machines) that the importer does a structural best-effort and tags every transition with `// review: unity-interruption-source=CurrentState` comments. Users open the RF graph, walk the transitions, confirm.

## 5. Unreal — binary, hence out-of-process

`.umap` and `.uasset` are UE-versioned tagged binary. Writing a third-party reader means chasing every engine version's serialization tweaks. There are open-source parsers (CUE4Parse, UAssetAPI) but using them means linking a large C#/C++ surface area and tracking their update cadence. Prefer the other path:

**Recommended: out-of-process UE headless exporter.** The user points the wizard at their installed UE editor. We ship a small UE plugin (`RustForgeExporter.uplugin`) that the wizard can optionally install into the source project. The plugin adds a `rustforge-export` commandlet:

```
UEEditor.exe MyProject.uproject -run=rustforge-export \
    -OutputDir=C:\export\rustforge-intermediate \
    -Scenes=/Game/Maps/Arena,/Game/Maps/Hub
```

The commandlet walks the project via UE's reflection, writes out a JSON intermediate plus any referenced assets as glTF / PNG / WAV. The RF importer then consumes the JSON — which is our schema, versioned by us — and ignores UE's binary formats entirely.

```rust
pub fn import_unreal(src: &UnrealProject, ctx: &mut ImportCtx) -> Result<()> {
    let intermediate = run_ue_exporter(src, &ctx.temp_dir)?;
    // intermediate/ contains:
    //   scenes/*.json     (our schema)
    //   assets/**/*.gltf  (UE meshes -> glTF via DatasmithRuntime-style export)
    //   assets/**/*.png   (textures)
    //   report.json       (what UE couldn't export)
    for scene in intermediate.scenes() {
        convert_ue_scene(scene, ctx)?;
    }
    merge_exporter_report(&intermediate, &mut ctx.report);
    Ok(())
}
```

**Fallback: third-party readers.** When the user doesn't have UE installed, we offer a best-effort path through a bundled CUE4Parse wrapper. This is explicitly marked "best-effort; may fail on your UE version; consider installing the UE exporter plugin instead." The Unreal importer UI has two radio buttons at the top: "I have UE installed (recommended)" vs "I don't have UE (best-effort)".

**Blueprints are flagged, period.** A Blueprint class is native behavior graph. The importer records the class name and notes it exists; it does not attempt conversion. "AI-assisted Blueprint → Rust translation" is a separate tool that reads the exporter JSON and suggests Rust code; it is not part of Phase 45.

**Materials.** UE Material node graphs can be exported by our plugin as a node list plus edges. The translator walks the list, maps each node to its Phase 20 equivalent where a 1:1 mapping exists (Multiply → Math node, TexCoord → UV, Time → Time, TextureSample → Sample, etc.), and falls back to Custom WGSL for Material Functions, custom HLSL nodes, and parameters the translator doesn't recognize. Conversion fidelity is tracked per-graph: a material that translates with zero fallbacks is "clean"; one with any fallback is "review".

## 6. Blender — extends Phase 5

Blender doesn't need a separate wizard step; it extends Phase 5's importer pipeline. A `.blend` file dropped into the content browser triggers the Blender adapter plugin:

```
Phase 5 importer dispatch:
    extension == .blend?
        -> BlendImporter::import(src, settings, out)
            -> spawn: blender --background --python adapter.py -- <src> <tempdir>
            -> adapter.py: bpy walks scenes/meshes/materials/actions -> glTF + JSON
            -> RF importer consumes glTF via existing Phase 5 GLTF pipeline
            -> JSON covers what glTF can't represent (collection hierarchy, custom props)
```

The adapter is Python because it runs inside Blender; driver code is Rust. The adapter script ships with the crate; user installs Blender separately, and the importer auto-detects Blender on common install paths or asks in settings.

Material translation: Principled BSDF is the happy path — its inputs map directly to the Phase 20 master node. Shader node trees that aren't Principled BSDF (custom node groups, emission trees, mix shaders) get dumped as a Custom WGSL fallback containing a comment with the Blender node graph structure serialized. Armatures convert to RF skeletons; actions convert to `.rtimeline` clips.

Grease pencil, particle systems, geometry nodes, and simulation nodes are all flagged as unmapped — they have no clean RF equivalent and inventing one is out of scope.

## 7. Asset conversion layer

External assets must land in `assets/` and get `.meta` sidecars (Phase 4 §4). The conversion layer centralizes this so every importer shares it:

```rust
pub struct AssetCopyCtx<'a> {
    pub project: &'a Project,
    pub report: &'a mut MigrationReport,
    pub remap: &'a mut HashMap<SourceAssetKey, AssetGuid>, // dedup identical copies
}

pub fn copy_external_asset(
    src: &Path,
    target_subdir: &str,           // "textures" / "meshes" / "audio"
    ctx: &mut AssetCopyCtx,
) -> Result<AssetGuid> {
    if let Some(existing) = ctx.remap.get(&key(src)) {
        return Ok(*existing);
    }
    let dest = ctx.project.assets_root().join(target_subdir).join(src.file_name());
    fs::copy(src, &dest)?;
    let guid = AssetGuid::new_v4();
    write_meta_sidecar(&dest, guid, default_settings_for(src))?;
    ctx.remap.insert(key(src), guid);
    ctx.report.note_asset_copied(src, &dest, guid);
    Ok(guid)
}
```

Dedup is important — a Unity project referencing `albedo.png` from 40 materials should produce one copy, not 40.

**Material node translator (`material_translate.rs`).** Per-engine node maps are data, not code:

```rust
pub struct NodeRule {
    pub source_type: &'static str,      // "UE:Multiply", "Unity:MulOp", "Blender:MATH:MULTIPLY"
    pub target: TargetNode,             // Phase 20 node kind
    pub port_map: &'static [PortMap],   // named in/out mapping
}

pub enum TargetNode {
    Direct(&'static str),               // Phase 20 node name
    Custom(WgslFallback),                // Custom WGSL with source comment
}
```

When no rule matches, the translator emits a Custom WGSL node with the best-available source (HLSL for UE custom nodes, shader source for Unity custom shaders, Python/node-graph dump for Blender). The user opens the material in Phase 20, sees one red node, and knows exactly where to port.

**Animation clips.** Keyframes → Phase 19 `Curve<T>` per tangent mode. Bezier survives; auto/linear/stepped preserve. Retargeting: if the source skeleton differs from the RF destination skeleton (UE mannequin → custom RF skeleton, say), emit a retarget hint — a `.rretarget` asset pre-populated with best-guess bone mapping by name. User opens it and fixes the wrong rows.

## 8. Migration report

Every importer writes into a shared `MigrationReport` during the import pass. After import, the report is serialized to `migration-report.md` in the project root.

```rust
pub struct MigrationReport {
    entries: Vec<ReportEntry>,
    source_engine: SourceEngine,
    source_root: PathBuf,
    imported_at: SystemTime,
}

pub enum ReportEntry {
    Clean        { source_file: PathBuf, what: String },
    Review       { source_file: PathBuf, what: String, reason: String, hint: String },
    ManualPort   { source_file: PathBuf, what: String, reason: String, doc_link: Option<String> },
    Unsupported  { source_file: PathBuf, what: String, reason: String },
    AssetCopied  { src: PathBuf, dest: PathBuf, guid: AssetGuid },
    Error        { source_file: PathBuf, err: String },
}
```

Excerpt from a generated report:

```markdown
# Migration Report — my-unity-game

Source: Unity 2022.3.10 LTS, C:/projects/MyGame
Imported: 2026-04-16 14:21 UTC
Target: C:/rustforge/MyGame-rf

## Summary
- Scenes migrated clean     : 4
- Scenes needing review     : 2
- Prefabs migrated          : 38
- Materials clean           : 22
- Materials with fallback   : 7
- Assets copied             : 312 (1.4 GB)
- Scripts flagged           : 64 MonoBehaviours (manual port)
- Errors                    : 0

## Needs Review
### scenes/Arena.unity -> scenes/arena.ron
- [review] Light "Sun" uses cookie texture; cookies unsupported, flagged entity tagged `rf.review.cookie`
- [review] Rigidbody on "Crate_041" has interpolation = Extrapolate; mapped to Interpolate with warning
- [review] Material "M_WaterSurface" uses Custom Shader — imported as Custom WGSL node; port HLSL body to WGSL manually

### prefabs/Goblin.prefab -> prefabs/goblin.ron
- [manual-port] MonoBehaviour "GoblinAI.cs" on root; port to WASM script
    - doc: docs/scripting-migration-guide.md#unity-monobehaviour

## Unsupported (not migrated)
- ProBuilder meshes in Arena.unity (7 entities) — convert to regular meshes in Unity first
- Timeline assets (2) — Phase 45 does not yet import Unity Timeline; Phase 19 format differs
```

The markdown is the artifact the team works off. It's checked into the new RF project so progress is tracked in git diffs: reviewers burn down entries over a sprint and the diff shows the shrinkage.

## 9. Bidirectional — lightweight export out

Some studios want RF to sit alongside Unity or Unreal for a while: use RF for one level, keep the rest in UE. Phase 45 ships a lightweight export path — not a full round-trip, but enough that an RF scene can be handed back as glTF + JSON metadata sidecars that the target engine's native GLTF importer can swallow.

```rust
pub fn export_scene_to_unity(scene: &SceneFile, dest: &Path) -> Result<()> {
    let gltf = serialize_scene_as_gltf(scene)?;   // geometry + transforms + materials via KHR_materials_*
    fs::write(dest.join("scene.gltf"), gltf)?;
    let sidecar = unity_component_sidecar(scene)?; // JSON describing RF components Unity doesn't know
    fs::write(dest.join("scene.rf-components.json"), sidecar)?;
    write_import_readme(dest)?;                    // "how to consume this in Unity"
    Ok(())
}
```

Unity reads the glTF via GLTFast; the sidecar JSON drives a small Unity helper script (`RustForgeImport.cs`, ships with the bidirectional export) that walks the imported hierarchy and attaches Unity-side stand-ins for RF-specific components. Unreal export is the same pattern, consumed by DatasmithRuntime plus a UE plugin side-loader.

This is explicitly lightweight. It supports the geometry / transform / basic-material contract; it does not carry over RF-specific systems (timeline, animation graph, material graph past PBR inputs). Users who need full round-trip should pick one engine. The path exists to lower commitment risk during evaluation.

## 10. Import wizard UI

```
┌─ Import Project ───────────────────────────────────────────────┐
│                                                                │
│  Step 1 of 4 — Source engine                                  │
│                                                                │
│   ( ) Unity   ( ) Unreal   (o) Godot   ( ) Blender (.blend)   │
│                                                                │
│  Source project root:                                          │
│   [ C:/projects/MyGodotGame                             ] [..] │
│                                                                │
│  Target RustForge project:                                     │
│   (o) Create new   ( ) Merge into existing                    │
│   [ C:/rustforge/MyGodotGame-rf                         ] [..] │
│                                                                │
│                                                                │
│                                      [ Cancel ]  [ Preview > ] │
└────────────────────────────────────────────────────────────────┘

┌─ Import Project ───────────────────────────────────────────────┐
│  Step 2 of 4 — Preview (dry run)                              │
│                                                                │
│  Found:                                                        │
│    Scenes (.tscn)              : 12                            │
│    Resources (.tres)           : 47                            │
│    Textures                    : 312                           │
│    Meshes (.obj/.glb)          : 58                            │
│    Audio (.wav/.ogg)           : 89                            │
│    GDScripts (will be flagged) : 34                            │
│                                                                │
│  Estimated:                                                    │
│    Clean migrations  : 84%                                     │
│    Needs review      : 11%                                     │
│    Manual port       :  5%                                     │
│    Copy size         : ~1.1 GB                                 │
│                                                                │
│              [ < Back ]    [ See Report ]    [ Import > ]      │
└────────────────────────────────────────────────────────────────┘
```

Preview runs the importers with a dry-run flag: parse + translate + report, no file writes. The report is fully generated and shown inline; the user reads it, flips back to Unity/UE/Blender to fix anything they want to fix before commit, and returns. Commit does the real copy + write.

Wizard is a modal panel, not a persistent dock. It vanishes after commit; the Cleanup helper (§11) takes over.

## 11. Cleanup helper

Once import commits, the project has some number of flagged entities and materials. The Cleanup helper is a dedicated dockable panel that reads the `migration-report.md` back and shows a burn-down list:

```
┌─ Migration Cleanup ────────────────────────────────────────────┐
│ Filter: [ All ▾ ] [ Open ▾ ]   Sort: [ Severity ▾ ]           │
│                                                                │
│  [ ] scenes/arena.ron — Sun light has cookie texture          │
│      [ Open in editor ] [ Open source ] [ Mark resolved ]     │
│      Notes: [ _______________________________________ ]        │
│                                                                │
│  [ ] prefabs/goblin.ron — MonoBehaviour GoblinAI (manual port) │
│      [ Open in editor ] [ Open source ] [ Mark resolved ]     │
│      Notes: [ Ported to scripts/goblin_ai.rs; verify          │
│               aggro radius matches Unity value ]               │
│                                                                │
│  [x] materials/water.rmat — Custom HLSL shader (ported)        │
│      resolved 2026-04-15 by liz                                │
│                                                                │
│  ...                                                           │
│                                                                │
│  42 open · 18 resolved · 60 total                              │
└────────────────────────────────────────────────────────────────┘
```

"Open in editor" jumps to the flagged entity in the RF hierarchy. "Open source" invokes the OS file handler on the original Unity/UE/Godot/Blender file so the user can check context. "Mark resolved" moves the entry to the resolved section with a timestamp and the current editor user's name, and writes a commit-friendly line to `migration-report.md` (git history then tracks it).

The helper persists — a six-month-old project still has the panel available, with the history intact, for onboarding new team members ("here's what we inherited and what's left to touch").

## 12. Build order

1. **Godot importer (.tscn + .tres parser, node map)** — proves the shared pipeline end-to-end against the easiest format.
2. **Migration report + markdown writer** — extracted early; every subsequent importer writes into it.
3. **Asset conversion layer** — copy, `.meta` generation, dedup, shared across all importers.
4. **Unity importer** — YAML parser with Unity tag preprocessor, `fileID`/`guid` resolution, component map, prefab support.
5. **Material translator crate-wide** — rule table, Phase 20 fallback, used by Unity and Blender first.
6. **Blender adapter** — Python script, Rust driver, Phase 5 importer dispatch on `.blend`.
7. **Unreal headless exporter** — UE plugin (C++) + JSON intermediate schema + Rust consumer. Best-effort CUE4Parse fallback last.
8. **Animation translator** — `.anim` / UE sequence / `.blend` action → `.rtimeline` with retarget hints.
9. **Bidirectional glTF export** — RF → Unity, RF → UE with sidecar JSON + receiver scripts.
10. **Import wizard UI** — four-step modal, preview, commit.
11. **Cleanup helper panel** — reads `migration-report.md`, provides filter/resolve UI, writes resolution history back.
12. **End-to-end test projects** — one real project per source engine, committed to a test-data repo, CI runs full import and asserts no regressions in clean/review/unsupported counts.

## Scope ❌

- ❌ Automatic script conversion — no MonoBehaviour-to-Rust, no GDScript-to-Rust, no UE C++-to-Rust. AI-assisted hand-porting is a separate product.
- ❌ Blueprint-to-Rust — Blueprints are behavior graphs over UE's reflection; porting them means reimplementing UE gameplay types. Out of scope forever.
- ❌ Runtime behavior parity — physics, particles, audio DSP differ between engines. Tuned values don't transfer 1:1; we import them and warn.
- ❌ Plugin migration — Unity native plugins, UE modules, Godot GDExtensions use engine APIs. No translation.
- ❌ Legacy engines — Source, id Tech, CryEngine, custom. Third parties can ship via Phase 11 plugin API; not built in.
- ❌ Full round-trip bidirectional — lightweight glTF + JSON is the contract; anything richer is a future phase.
- ❌ Unity Timeline / Cinemachine deep import — shape mismatch with Phase 19; flagged, not translated.
- ❌ Incremental / continuous sync — import is a one-time batch; "keep my Unity project and RF project in sync forever" is not a thing we offer.
- ❌ Auto-fix of flagged entries — every review / manual-port item is a human decision.

## Risks

- **YAML dialects bite.** Unity's YAML is mostly-but-not-quite spec YAML — tags, floats, anchors all have edge cases. Expect test coverage to be ~200 real-world files before the parser is trustworthy. Ship with a "report parser error with file sample" prompt; treat parser misses as P0.
- **UE version skew.** Every UE minor version changes `.uasset` serialization. Relying on the headless exporter sidesteps this — the user's own UE install is the parser. The CUE4Parse fallback will bitrot and should be documented as such.
- **Guid collisions.** Unity `.meta` GUIDs and RF `AssetGuid`s are both 128-bit but different namespaces. Do not reuse Unity GUIDs as RF GUIDs — generate fresh RF GUIDs and record the mapping in the report. Reusing would cause collisions if the same Unity asset is imported into two RF projects that then merge.
- **Blender headless fragility.** Users have five Blender versions, custom addons, broken Python envs. Detect Blender version on wizard entry; refuse below 3.6 with a clear error; log stderr on failure.
- **Over-promising in marketing.** The instant this ships, someone will put "Unity-compatible" on a slide. Ship with explicit prose in the wizard, the report, and the website saying what migration does and does not do. Under-promise; the tool will out-perform the promise.
- **Script flagging avalanche.** A Unity project with 500 MonoBehaviours produces 500 report entries. Good. The user filters by `[manual-port]` and plans. Bad if we try to hide the count.
- **Material fidelity debt.** "Best-effort" node translation means most PBR materials look right but some look wrong. A visual diff tool — render both, eyeball — is not in Phase 45, and that's a real gap. Flag as future work; document which node kinds are verified.
- **Huge projects.** A 100 GB Unity project will take hours to import. The wizard must show progress, allow cancel, and resume from partial state. No long-running step should be an all-or-nothing commit.
- **License surface.** The UE exporter plugin ships under UE's EULA; the CUE4Parse path ships under its MIT/GPL mix; Blender is GPL. The migration crate itself is permissive; each adapter sits behind its own license notice. Audit before Phase 45 ships.

## Exit criteria

Phase 45 is done when all of these are true:

- [ ] Godot importer converts a real `.tscn` project with ≥50 nodes into a valid RF project; opening it in the RF editor shows matching hierarchy and meshes.
- [ ] Unity importer converts a real `.unity` + `.prefab` project (≥10 scenes, ≥50 prefabs, ≥100 materials) with ≥80% clean migration rate on its declarative content.
- [ ] Unreal importer via headless exporter converts a real UE project (≥5 maps) with ≥70% clean migration on declarative content.
- [ ] Blender importer extends Phase 5 — `.blend` drag-drop in content browser runs adapter and produces usable meshes + materials + animations.
- [ ] Material translator produces either a Phase 20 graph or a Custom WGSL fallback for every material input; no material throws.
- [ ] Animation clips convert to `.rtimeline` with retarget hints for non-identity skeletons.
- [ ] Migration report markdown generates for every import, lists every flagged entry with source file + reason + hint, and is committable to git.
- [ ] Bidirectional: an RF scene exports to a Unity-importable glTF + sidecar pair that Unity opens with components attached by the receiver script.
- [ ] Import wizard modal implements all four steps; preview produces the same report the commit will, minus file writes.
- [ ] Cleanup helper panel reads the report, supports filter / resolve / notes, writes resolutions back to `migration-report.md`.
- [ ] End-to-end CI test runs a canonical test project per source engine through full import and asserts clean / review / unsupported counts.
- [ ] Wizard entry screen shows prose that truthfully describes what migration does and does not do; product and docs pages match that prose verbatim.
- [ ] `rustforge-core` builds without `rustforge-migrate`; the migration crate is editor-only.
