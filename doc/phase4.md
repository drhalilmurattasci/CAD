# Phase 4 — Scene I/O, Prefabs, Project Structure

Phase 3 gave you a fully interactive editor, but the moment you close it, everything vanishes. Phase 4 makes the editor productive: scenes persist to disk, projects have a structure, and prefabs let you reuse entity templates across scenes.

## Goals

By end of Phase 4:

1. **Projects** are a real concept with a root folder, `rustforge-project.toml` manifest, and an `assets/` + `scenes/` convention.
2. **Scenes** serialize to `.ron` on disk, round-trip losslessly (load → save produces identical file), and can be opened/saved via File menu.
3. **Prefabs** let you save an entity subtree as a reusable template and instantiate it into scenes; prefab changes propagate to instances.
4. **Stable IDs** exist for both entities (within a scene) and assets (across the project), making scene files diff-friendly and merge-friendly.

## 1. The core tension — `Entity` is runtime-only

`hecs::Entity` is a generational index. Two problems:
- IDs differ between runs (`Entity(3, gen 0)` on save ≠ `Entity(3, gen 0)` on load).
- IDs are meaningless across files — `Parent(Entity(5))` in a scene file is noise.

Every scene format needs a **persistent ID layer** on top. Decide this upfront; it touches everything.

**Recommendation:** add a `SceneId` component.

```rust
#[derive(Reflect, Clone, Copy)]
pub struct SceneId(pub u64);   // random u64, stable per entity within a scene
```

Every entity saved in a scene gets a `SceneId`. References between entities in the scene (parent, child, component fields of type `Entity`) serialize as `SceneId`, not `Entity`. On load, build a `HashMap<SceneId, Entity>` and resolve.

This has a cost: every `Entity`-typed field in any component needs special serialization. Either:
- Tag them: `#[reflect(entity_ref)]` attribute on the field.
- Use a newtype: `EntityRef(Entity)` with its own `Reflect` impl.

Newtype is cleaner. Go with `EntityRef`.

## 2. Project structure

```
my-game/
├── rustforge-project.toml        # project manifest
├── .rustforge/                   # editor-managed, gitignored-friendly cache
│   ├── thumbnails/
│   └── editor-state.ron          # last open scene, window layout override
├── assets/                       # user assets (.gltf, .png, .wav, etc.)
│   └── meshes/
├── scenes/                       # .ron scene files
│   └── main.ron
├── prefabs/                      # .ron prefab files
│   └── enemies/
│       └── goblin.ron
└── scripts/                      # WASM scripts
    └── player.rs
```

Manifest (`rustforge-project.toml`):

```toml
[project]
name = "My Game"
rustforge_version = "0.1.0"

[scenes]
default = "scenes/main.ron"

[assets]
root = "assets"
```

Keep it minimal. Don't preemptively add engine config, graphics settings, or build options — those earn their way in later.

### Project vs. engine split

```
crates/rustforge-core/src/project/     # NEW module
├── mod.rs
├── manifest.rs                         # Project, ProjectLoader
├── paths.rs                            # canonical path resolution
└── layout.rs                           # directory conventions
```

The editor doesn't hardcode these paths — it asks the project. Games ship without editor but may or may not need the project concept at runtime; behind the `editor` feature is safest for now.

## 3. Scene file format

Use `.ron`. Rationale:

- Human-readable → diffable in git, debuggable by hand.
- Serde-compatible → minimal code.
- Supports enums and nested structs naturally.
- Comments allowed (via `#`).

```
crates/rustforge-core/src/scene/
├── mod.rs
├── hierarchy.rs          # (from Phase 3)
├── transform_system.rs   # (from Phase 3)
├── scene_id.rs           # SceneId component + EntityRef newtype
├── format.rs             # SceneFile struct, top-level wrapper
├── serialize.rs          # World -> SceneFile
├── deserialize.rs        # SceneFile -> World
└── version.rs            # format_version + migration stubs
```

### 3.1 File shape

```ron
(
    format_version: 1,
    entities: [
        (
            id: 0x8f2a9c...,        // SceneId as hex u64
            name: Some("Player"),    // if Name component present
            parent: None,
            components: {
                "Transform": (position: (0, 0, 0), rotation: (0,0,0,1), scale: (1,1,1)),
                "MeshRenderer": (mesh: AssetRef("a1b2..."), material: AssetRef("c3d4...")),
                "PlayerController": (speed: 5.0),
            },
        ),
        (
            id: 0x1e7b4d...,
            name: Some("Camera"),
            parent: Some(0x8f2a9c...),
            components: { ... },
        ),
    ],
)
```

Keys:
- **Entity list, not tree.** Tree structure comes from `parent`. Flat lists are easier to serialize, diff, and merge.
- **Components keyed by type name**, not numeric ID. More verbose, but resilient to registration order.
- **Field ordering in components is deterministic** (sort alphabetically or by declaration order). Non-deterministic ordering will cause spurious git diffs.

### 3.2 Serialization flow

```
serialize(world):
    visit all entities with SceneId
    for each: collect registered components via reflection
    build SceneFile { entities: [...] }
    serde_ron::to_string_pretty(SceneFile)

deserialize(file):
    parse SceneFile
    spawn empty entities for each SceneId; build SceneId -> Entity map
    resolve parents (translate SceneId -> Entity)
    deserialize each component via registry.deserialize_fn
    for EntityRef fields: translate SceneId -> Entity
    validate hierarchy invariants
```

The `EntityRef` translation pass is the tricky one. Easiest approach: serialize `EntityRef` as `SceneId`, then during deserialize do a two-pass walk — first spawn + map IDs, then resolve refs.

### 3.3 Versioning & migration

Include `format_version: u32` at the top. Even if you never need it, the cost of having it is zero; the cost of retrofitting later is painful. Start at `1`.

```
crates/rustforge-core/src/scene/version.rs:
    pub const CURRENT_VERSION: u32 = 1;
    pub fn migrate(value: &mut ron::Value, from: u32) -> Result<()>;
```

Don't write migration code in Phase 4. Just the scaffolding.

## 4. Asset GUIDs

Prefabs (§5) and scene files (§3) both reference assets. Paths are fragile (rename breaks everything) — use GUIDs.

```
crates/rustforge-core/src/assets/
├── guid.rs               # AssetGuid(u128), generated via uuid v4
├── meta.rs               # .meta sidecar: { guid, importer_version, settings }
└── registry.rs           # AssetRegistry: GUID -> current path, reverse lookup
```

Convention (borrowed from Unity, which got this right):

```
assets/meshes/player.gltf
assets/meshes/player.gltf.meta     ← sidecar, committed to git
```

`.meta`:
```ron
(
    guid: "8f2a9c1e-...",
    importer_version: 1,
    settings: ( generate_collision: true ),
)
```

Rules:
- `.meta` is generated the first time an asset is seen by the editor.
- `.meta` is committed to source control alongside the asset.
- Renaming/moving an asset preserves its `.meta` (same GUID, new path in registry).
- Deleting an asset deletes its `.meta`.
- Scene and prefab files reference assets by GUID, never by path.

This is **infrastructure for Phase 5** (Content Browser), not fully wired up here. Phase 4 only needs:
- The `AssetGuid` type.
- `AssetRef<T>(AssetGuid)` wrapper used in component fields.
- A minimal registry that walks `assets/` at project load, generates missing `.meta` files, and builds the GUID → path map.

Full file watching, thumbnails, and drag-drop wait for Phase 5.

## 5. Prefabs

A prefab is a serialized entity subtree. Dropping a prefab into a scene creates a copy; editing the prefab propagates to all copies.

```
crates/rustforge-core/src/scene/
├── prefab.rs             # PrefabFile, PrefabInstance component
└── prefab_system.rs      # override tracking, propagation
```

### 5.1 File format

Same as scene format, but:
- Top-level wrapper is `PrefabFile`, not `SceneFile`.
- Exactly one **root** entity (may have descendants).
- Stored as `prefabs/**/*.ron` with a companion `.meta` (prefabs have GUIDs like other assets).

```ron
(
    format_version: 1,
    prefab_guid: "...",
    root: 0xabcd...,
    entities: [ ... ],   // same structure as scene
)
```

### 5.2 Instance tracking

```rust
#[derive(Reflect)]
pub struct PrefabInstance {
    pub prefab: AssetRef<Prefab>,
    pub overrides: Vec<PropertyOverride>,
}

pub struct PropertyOverride {
    pub entity_in_prefab: SceneId,   // which sub-entity
    pub component: String,           // type name
    pub field_path: String,          // e.g. "transform.position.x"
    pub value: ron::Value,
}
```

On scene load:
1. For each `PrefabInstance`, load the referenced prefab.
2. Spawn prefab contents as children of the instance root.
3. Apply overrides on top.

### 5.3 Overrides — the hard problem

Full Unity-style nested prefabs with per-property override tracking is a multi-phase feature. **Don't do that in Phase 4.** Instead:

**Phase 4 prefab scope:**
- Create prefab from selection (editor → `prefabs/foo.ron`).
- Instantiate prefab into scene (drag from browser in Phase 5, menu command for now).
- **No overrides.** Instances are exact copies at spawn time. If you edit the prefab, you need to re-instantiate manually.
- **No nesting.** Prefabs can't contain other prefab instances.

This is a lot less than "real" prefabs, but it's enough to be useful, and it doesn't paint you into a corner. Full override-aware prefabs get their own phase later (call it Phase 4.5 or Phase 8).

In the inspector, show a "Prefab: goblin.ron" header on instances with an "Unpack" button (break the link, treat as normal entities). That's it.

## 6. Editor integration

### 6.1 File menu

```
File
  New Project...
  Open Project...
  ─────
  New Scene            Ctrl+N
  Open Scene...        Ctrl+O
  Save Scene           Ctrl+S
  Save Scene As...     Ctrl+Shift+S
  ─────
  Recent Projects   ▸
  Recent Scenes     ▸
  ─────
  Exit
```

Use `rfd` for native file dialogs. Save scene-path in `EditorApp`; dirty flag triggers asterisk in window title.

### 6.2 Dirty tracking

Everything that mutates the scene sets `scene_dirty = true`. Phase 3's inspector and gizmos already mutate; route them through a single `world.mark_dirty()` helper now so Phase 6 (undo) has one integration point.

Unsaved-changes prompt on close / open / new. Standard three-button dialog: Save, Discard, Cancel.

### 6.3 Prefab creation flow

Right-click entity in Hierarchy → "Create Prefab..." → file dialog inside `prefabs/` → writes file, replaces original entity with a `PrefabInstance` pointing at the new prefab.

### 6.4 Startup

```
editor launch:
    if last-opened project exists -> open it
    else -> show "Create or Open Project" modal
    if project has default scene -> load it
    else -> blank scene titled "Untitled"
```

Store last-opened project in platform config dir (not inside the project itself).

## 7. Build order within Phase 4

1. **Project concept** — `rustforge-project.toml`, `Project` struct, loader, path resolution. Editor opens with a hardcoded test project first.
2. **`SceneId` + `EntityRef`** — core types, reflection integration. Every spawn in the editor gets a `SceneId` (auto-inserted, like `Name`).
3. **Asset GUID minimum** — `AssetGuid`, `AssetRef<T>`, `.meta` read/write, a startup walker that scans `assets/` and generates missing `.meta`s. No watching yet.
4. **Scene serialize** — `World → SceneFile → ron::to_string`. Round-trip test: spawn test entities, serialize, deserialize into new world, diff.
5. **Scene deserialize** — full two-pass with ID resolution. More tests.
6. **File menu + dirty tracking** — New/Open/Save with file dialogs.
7. **Recent projects/scenes** — persistent list in editor config.
8. **Prefab format + creation** — "Create Prefab" command writes the file.
9. **Prefab instantiation** — loader, `PrefabInstance` component, inspector shows prefab origin + Unpack button.
10. **Startup flow** — project picker modal, last-opened project auto-load.

## 8. Scope boundaries — what's NOT in Phase 4

- ❌ Content Browser UI (Phase 5) — opening/instantiating prefabs happens via file dialog for now.
- ❌ Asset file watcher (Phase 5).
- ❌ Thumbnails (Phase 5).
- ❌ Prefab overrides & nested prefabs (Phase 4.5 or later).
- ❌ Undo/redo for scene operations (Phase 6). Save and you're safe; anything else is on you.
- ❌ Scene streaming / sub-scenes — far-future problem.
- ❌ Binary scene format — `.ron` only. Cooked/binary scenes can come during a future "shipping" phase.

## 9. Risks & gotchas

- **Reflection must handle `Vec`, `Option`, `HashMap` before Phase 4.** Scene serialization will hit these in real components. If deferred from Phase 3, now is when it blocks everything. Do it first or accept that some components don't round-trip.
- **Non-registered components become invisible on save.** An entity with a `CustomAIState` component that forgot `#[derive(Reflect)]` will load back missing that component. At minimum log a warning; ideally fail loudly during save. Silent loss is the worst outcome.
- **Float precision in `.ron`.** Default float formatting can round-trip lossy. Use `ron::ser::PrettyConfig::new().compact_arrays(true)` and double-check with a round-trip test. If precision matters (physics, animation), serialize as hex or raw bits.
- **`SceneId` collisions.** u64 random is 1-in-2^32 collision at ~4 billion entities; fine in practice. But if you copy-paste a scene file section, you'll get duplicates. Loader should detect and renumber duplicates with a warning.
- **Circular prefab references.** Prefab A contains Prefab B contains Prefab A → infinite expand. Even without nesting, the *file* could reference itself. Cycle-check at load.
- **Asset GUID conflicts on git merge.** Two developers add the same file on different branches → two `.meta` files with different GUIDs for the same asset path. No clean fix. Document it: treat `.meta` conflicts as "pick one, delete the other, re-link references manually." It's rare in practice.
- **`EntityRef` after entity deletion.** A scene file references an entity that was deleted. Either: (a) null it out with a warning, (b) spawn a placeholder "broken reference" entity. Go with (a).
- **Path separators on Windows.** `.ron` file paths as `assets/meshes/foo.gltf` vs `assets\meshes\foo.gltf`. Always normalize to forward slashes in serialized form; convert at I/O boundary.

## 10. Exit criteria

Phase 4 is done when all of these are true:

- [ ] Editor opens with a project picker and loads a valid project.
- [ ] `File → New Scene` creates an empty scene; `File → Save Scene` writes to disk.
- [ ] Round-trip test: save scene, reload, diff → identical.
- [ ] Scene file is human-readable, uses SceneIds not Entity indices, references assets by GUID.
- [ ] Parent/child relationships persist correctly across save/load.
- [ ] `EntityRef` fields in components persist correctly across save/load.
- [ ] Dirty indicator (asterisk in title) appears after edits and clears on save.
- [ ] Unsaved-changes prompt blocks Open/New/Exit when dirty.
- [ ] "Create Prefab from Entity" writes a prefab file.
- [ ] Instantiating a prefab from File menu spawns its entities into the scene, as children of a `PrefabInstance`-tagged root.
- [ ] Recent projects and recent scenes persist across editor restarts.
- [ ] Missing assets referenced by GUID produce a warning, not a crash.
- [ ] `rustforge-core` still builds without the `editor` feature (core serialization may live in core; UI paths are feature-gated).
