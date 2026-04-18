# Phase 5 — Content Browser & Asset Pipeline

Phase 4 gave you asset GUIDs and `.meta` sidecars, but no UI to see or manage them. Phase 5 turns the `assets/` folder into a first-class part of the editor: a browsable panel, live file watching, thumbnails, drag-drop into the scene and inspector, and a proper import pipeline for source files like `.gltf` and `.png`.

## Goals

By end of Phase 5:

1. **Content Browser panel** shows the project's `assets/` folder as a filesystem tree + grid view, with thumbnails, filtering, and search.
2. **File watcher** detects external changes (asset added, moved, deleted, modified) and updates the registry in real time.
3. **Drag-and-drop** works in both directions: from OS/explorer into the browser (import), and from the browser into viewport/inspector (assign).
4. **Import pipeline** converts source files (`.gltf`, `.png`, `.wav`) into engine-native formats (cooked meshes, textures, audio clips) with per-importer settings stored in `.meta`.
5. **Thumbnails** exist for at least textures and meshes, generated async in the background.

## 1. Architecture — editor vs. engine split

This is the phase where "editor features" and "core engine" start rubbing against each other. Draw the line clearly:

**`rustforge-core`** (shipped with games):
- `AssetGuid`, `AssetRef<T>`, cooked asset loading, runtime asset cache.
- Basic registry: GUID → loaded asset.

**`rustforge-core` with `editor` feature**:
- `.meta` sidecar read/write (already exists from Phase 4).
- Source asset → cooked asset importers (`.gltf` → cooked mesh, etc.).
- Registry extension: GUID → source path + cooked path.

**`rustforge-editor`** (never shipped):
- Content Browser panel.
- File watcher.
- Thumbnail cache.
- Drag-drop handling.
- Import settings UI.

Importers live in `rustforge-core` because game builds may need to re-cook assets during a bake step, but editing/browsing lives in `rustforge-editor`.

## 2. Registry, upgraded

Phase 4's registry was a minimal GUID → path map at startup. Phase 5 needs it to be live.

```
crates/rustforge-core/src/assets/
├── guid.rs               # (from Phase 4)
├── meta.rs               # (from Phase 4)
├── registry.rs           # EXPANDED — live registry with events
├── source.rs             # SourceAsset { path, kind, meta }
├── cooked.rs             # CookedAsset { guid, kind, bytes/path }
├── kind.rs               # AssetKind enum (Mesh, Texture, Material, ...)
├── handle.rs             # Handle<T> — runtime-only typed reference
└── events.rs             # AssetEvent (Added, Modified, Removed, Moved)
```

Key types:

```rust
pub struct AssetRegistry {
    by_guid: HashMap<AssetGuid, RegistryEntry>,
    by_path: HashMap<PathBuf, AssetGuid>,
    events: VecDeque<AssetEvent>,
}

pub struct RegistryEntry {
    pub guid: AssetGuid,
    pub kind: AssetKind,
    pub source_path: PathBuf,         // assets/meshes/player.gltf
    pub cooked_path: Option<PathBuf>, // .rustforge/cache/<guid>.bin
    pub state: AssetState,            // Unimported | Importing | Ready | Failed
    pub importer_version: u32,
    pub last_imported: SystemTime,
}

pub enum AssetEvent {
    Added(AssetGuid),
    Modified(AssetGuid),
    Removed(AssetGuid),
    Moved { guid: AssetGuid, from: PathBuf, to: PathBuf },
    Reimported(AssetGuid),
}
```

Events are how everything downstream learns about changes. The Content Browser polls them each frame to refresh its view. The asset cache uses them to hot-reload.

## 3. File watching

```
crates/rustforge-editor/src/assets/
├── mod.rs
├── watcher.rs            # notify-rs → AssetEvent
├── importer.rs           # dispatches to core importers
├── thumbnails.rs         # async thumbnail generation
└── meta_ui.rs            # import settings inspector
```

Use the `notify` crate. Debounce aggressively — a single save in an editor can fire 3-5 events. 200ms debounce is reasonable.

Watcher logic:

```
on event:
    classify by path + extension
    if path is *.meta -> ignore (we write those; don't self-trigger)
    if path is in .rustforge/ -> ignore (cache)
    if event is Create -> enqueue Import
    if event is Modify -> enqueue Reimport (preserving guid)
    if event is Remove -> enqueue Remove
    if event is Rename -> enqueue Move (preserving guid)
```

The watcher produces work items; a worker thread drains them and updates the registry.

### 3.1 The `.meta` write-loop trap

The editor writes `.meta` files → watcher sees the write → editor tries to re-import → writes `.meta` again → infinite loop.

Prevention:
- Filter by extension (`*.meta` → ignore).
- Or: track a set of "paths we just wrote" and skip events within ~500ms.

The extension filter is cleaner. Go with that.

## 4. Importers

An importer is "source format → cooked format + import metadata". One importer per source file type.

```
crates/rustforge-core/src/assets/importers/
├── mod.rs                # Importer trait, ImporterRegistry
├── gltf.rs               # GltfImporter: .gltf/.glb → Mesh, Material, Texture, Skeleton
├── image.rs              # ImageImporter: .png/.jpg/.exr → Texture
├── audio.rs              # AudioImporter: .wav/.ogg → AudioClip  (stub OK for Phase 5)
├── shader.rs             # ShaderImporter: .wgsl → ShaderModule  (stub OK)
└── prefab.rs             # PrefabImporter: .ron → PrefabAsset
```

Trait shape:

```rust
pub trait Importer: Send + Sync {
    type Settings: Serialize + DeserializeOwned + Default + Reflect;

    fn name() -> &'static str;
    fn version() -> u32;
    fn extensions() -> &'static [&'static str];

    fn import(
        &self,
        source: &Path,
        settings: &Self::Settings,
        output: &mut ImportOutput,
    ) -> Result<()>;
}

pub struct ImportOutput {
    pub primary: AssetGuid,         // the main artifact
    pub sub_assets: Vec<SubAsset>,  // e.g. gltf → many meshes, textures
    pub dependencies: Vec<AssetGuid>,
}
```

### 4.1 Sub-assets

A single `.gltf` file can produce many assets: meshes, materials, textures, a skeleton, animations. Each sub-asset gets its own GUID. The `.meta` file for `player.gltf` records the parent GUID and all sub-asset GUIDs with stable keys (sub-asset index or name):

```ron
(
    guid: "aaaa-...",
    importer: "gltf",
    importer_version: 1,
    settings: ( generate_collision: true, scale: 1.0 ),
    sub_assets: {
        "mesh:0": "bbbb-...",
        "mesh:1": "cccc-...",
        "material:0": "dddd-...",
        "texture:albedo": "eeee-...",
    },
)
```

Critical rule: **re-importing must preserve sub-asset GUIDs.** The key (name or stable index) is what maps old GUIDs to new artifacts. Without this, every re-import breaks every reference. Unity learned this the hard way.

### 4.2 Cooked output

```
.rustforge/cache/
├── aa/aaaa-....mesh           # first 2 chars = sharding to avoid huge dirs
├── bb/bbbb-....mesh
└── dd/dddd-....material
```

Two-char sharding keeps directory listings fast at scale. Cooked format is opaque binary — each asset kind defines its own. Runtime `AssetCache` memory-maps or reads these on demand.

### 4.3 Importer versioning

Each importer has a version integer. If `importer_version` in `.meta` differs from `Importer::version()`, trigger re-import. This is how you propagate importer bug fixes to existing projects without manual intervention.

## 5. Content Browser panel

```
crates/rustforge-editor/src/panels/
└── content_browser.rs
```

### 5.1 Layout

```
┌──────────────┬────────────────────────────────────────────┐
│ ▼ assets/    │ [grid of thumbnails]                       │
│   ▼ meshes/  │  ┌──┐ ┌──┐ ┌──┐ ┌──┐                      │
│     props/   │  │  │ │  │ │  │ │  │                      │
│   ▶ textures/│  └──┘ └──┘ └──┘ └──┘                      │
│   ▶ audio/   │  player.gltf  crate.gltf  wall.gltf ...    │
│ ▼ prefabs/   │                                            │
│   enemies/   │                                            │
├──────────────┴────────────────────────────────────────────┤
│ [filter: ____] [type: All ▾]  [size: ──▣──]              │
└────────────────────────────────────────────────────────────┘
```

- **Left:** folder tree (matches disk layout under project root).
- **Right:** grid (or list) view of contents of selected folder.
- **Bottom:** search filter, type filter dropdown, thumbnail size slider.

### 5.2 Interactions

- Click folder → shows its contents on the right.
- Click asset → selects it; Inspector panel switches to asset inspector (import settings).
- Double-click mesh/prefab → instantiate into scene at origin.
- Right-click → context menu: Open, Rename, Delete, Show in Explorer, Reimport, Create New…
- Drag asset from browser → drop on viewport (spawn) or inspector field (assign `AssetRef`).
- Drag file from OS into browser → copy into current folder, trigger import.

### 5.3 Breadcrumb navigation

Above the grid, show `assets / meshes / props /` as clickable breadcrumbs. Small UX detail that pays for itself immediately.

### 5.4 Selection independence

Content Browser has its own selection, separate from scene `SelectionSet`. When an asset is selected in the browser, the Inspector shows the asset's import settings, not a scene entity. Decide precedence: last-touched panel wins. Scene selection takes over as soon as an entity is clicked.

## 6. Thumbnails

```
crates/rustforge-editor/src/assets/thumbnails.rs
```

### 6.1 Storage

```
.rustforge/thumbnails/
├── aa/aaaa-....png    # 128x128 png per GUID
└── ...
```

Same sharding as cooked assets.

### 6.2 Generation

Per asset kind:
- **Texture** — resize source image to 128×128, sRGB-correct.
- **Mesh** — render in a headless wgpu pass: camera framed on mesh bounds, default material, single key light, transparent background. Cache result.
- **Material** — render a sphere with the material under the same framing.
- **Prefab** — render the prefab root's mesh (if any) or a default icon.
- **Audio** — static waveform icon, or render an actual waveform mini-image (nice-to-have).
- **Everything else** — type-based icon (folder, ron, unknown).

Mesh/material thumbnails need the engine's render pipeline — another reason the editor links against `rustforge-core`. Run these on a worker thread with a dedicated `wgpu::Device` (separate from main render device, or shared with explicit command encoder discipline).

### 6.3 Async generation

Never block the UI for a thumbnail. Strategy:

```
on browser open folder:
    for each asset without thumbnail in cache:
        enqueue thumbnail job
    render placeholder icon immediately

on thumbnail job complete:
    write png to cache
    mark asset dirty -> UI refreshes next frame
```

Simple futures-based worker pool is fine. Don't over-engineer; this is not a hot path.

### 6.4 Invalidation

Thumbnail is valid if `thumbnail_mtime > source_mtime`. On re-import, delete the thumbnail; next browser view regenerates. `.rustforge/` can be nuked at any time with no data loss — design for this.

## 7. Drag-and-drop

### 7.1 OS → browser (import)

`winit` exposes `Event::WindowEvent::DroppedFile`. Route to Content Browser:
- If over the browser panel → copy file into current folder, queue import.
- If over the viewport → copy file into a default location, import, and immediately spawn its primary asset into the scene.

Ask before overwriting existing files with the same name.

### 7.2 Browser → viewport (spawn)

Mesh/prefab dropped on viewport:
- Raycast from cursor into scene → spawn at hit point (or at camera focus distance if no hit).
- For prefabs: full prefab instantiation.
- For meshes: spawn a new entity with `Transform + MeshRenderer + default material`.

### 7.3 Browser → inspector (assign)

An `AssetRef<Mesh>` field in the inspector is a drop target. When a compatible asset (matching type) is dragged over it, highlight; on drop, assign the GUID. Reject type mismatches visually (red highlight).

Phase 3's `asset_ref.rs` inspector widget (which was a stub) becomes real here.

## 8. Asset inspector

When an asset is selected in the browser, Inspector panel switches mode:

```
┌─ player.gltf ─────────────┐
│ GUID: aaaa-...            │
│ Type: Model               │
│ ─────────────────────────  │
│ Import Settings           │
│   [x] Generate collision  │
│   Scale:  [ 1.0   ]       │
│   Smooth normals: [x]     │
│ ─────────────────────────  │
│ Sub-assets:               │
│   ▸ Mesh "body"           │
│   ▸ Mesh "head"           │
│   ▸ Material "skin"       │
│ ─────────────────────────  │
│        [ Reimport ]       │
└───────────────────────────┘
```

Changing an import setting marks the `.meta` dirty; applying triggers a re-import. Sub-assets expandable, clicking one selects it as a separate asset.

The asset inspector reuses the reflection/inspect infrastructure — each importer's `Settings` type is `Reflect`, so the UI generates automatically.

## 9. Build order within Phase 5

1. **Registry expansion** — event queue, state tracking, move/remove handling, path ↔ GUID bidirectional map.
2. **File watcher** — `notify` integration, debounce, `.meta` extension filter, event forwarding into registry.
3. **Importer trait + ImporterRegistry** — infrastructure only, no real importers yet.
4. **Image importer** — simplest case; source PNG → cooked texture. Validate the full pipeline end-to-end.
5. **GLTF importer** — the big one. Meshes, materials, textures, sub-asset GUID stability across re-imports.
6. **Content Browser panel (read-only)** — folder tree + grid, no thumbnails, no interactions. Just shows what's in the project.
7. **Thumbnail system** — async worker, texture thumbnails first (easy), then mesh thumbnails (hard).
8. **Browser interactions** — click/double-click/context menu, breadcrumbs, search filter.
9. **Asset inspector mode** — Inspector panel switches based on selection source.
10. **OS → browser drag-drop** — file import flow.
11. **Browser → inspector drag-drop** — `AssetRef` field assignment. Phase 3's stub becomes real.
12. **Browser → viewport drag-drop** — spawn mesh/prefab at cursor raycast.
13. **Reimport + hot-reload** — asset event → runtime cache invalidation → visual update in viewport.

## 10. Scope boundaries — what's NOT in Phase 5

- ❌ Shader editor / preview (its own specialized panel, much later).
- ❌ Material graph editor — materials are edited as structs via reflection inspector. Node-based editor is a separate phase.
- ❌ Animation import beyond keyframe tracks — no retargeting, no blend trees yet.
- ❌ Audio importers with real DSP — stub importer is fine.
- ❌ Compression / streaming — cooked assets are uncompressed for now.
- ❌ Asset bundles / pak files — ship prep concern, not editor concern.
- ❌ Remote asset sources (HTTP, S3) — local files only.
- ❌ Collaborative editing / asset locking — out of scope.
- ❌ Undo/redo for asset operations (delete/rename) — Phase 6 covers scene ops; asset ops remain unversioned.

## 11. Risks & gotchas

- **Path canonicalization.** `assets/meshes/player.gltf`, `assets/meshes/./player.gltf`, `assets\meshes\player.gltf`, symlinks — all must map to the same registry entry. Always canonicalize at ingestion (`dunce::canonicalize` on Windows). Store paths as project-relative `String` with forward slashes.
- **Watcher misses events under load.** `notify` can drop events if the OS buffer fills. On startup, always do a full re-scan and reconcile against registry state. Don't trust the watcher as the single source of truth.
- **GLTF importer scope creep.** Full glTF 2.0 is huge. Phase 5 minimum: static meshes + PBR materials + textures. Skinned meshes, morph targets, animations, extensions — each can be follow-up patches. Otherwise you're writing a gltf parser instead of an editor.
- **Sub-asset GUID stability.** Easy to accidentally break. Write a test: import gltf, capture sub-asset GUIDs, modify the gltf file (add unrelated mesh), re-import, confirm existing sub-asset GUIDs unchanged. If this test doesn't exist, it will break silently.
- **Thumbnail render device contention.** If thumbnails and main viewport share a `wgpu::Device`, synchronization is fiddly. Separate device per thread is simpler but costs VRAM. Start shared with a mutex; profile if it hurts.
- **Very large projects.** 10k assets × thumbnail load × registry rebuild can bog startup. Keep thumbnails lazy (generate on view, not on startup). Registry can be cached to `.rustforge/registry.bin` and validated incrementally.
- **Meta file git conflicts.** Documented in Phase 4; resurface in Phase 5 README. Consider a "reconcile .meta" tool: detect duplicate GUIDs across files, offer to assign new GUIDs to one.
- **Case sensitivity.** macOS default HFS+ is case-insensitive; Linux ext4 is case-sensitive; Windows NTFS is case-insensitive-but-preserving. A project authored on macOS with `Player.gltf` and `player.gltf` will break on Linux. Detect and warn on project load.
- **Unicode filenames.** Someone will test this. `notify` and `std::fs` handle it, but some older image decoders don't. Add a test project with a non-ASCII filename.
- **Dropping files during import.** User drops gltf while previous import still in progress. Need a queue, not a flag. Easy to get wrong.

## 12. Exit criteria

Phase 5 is done when all of these are true:

- [ ] Content Browser panel shows the project's assets with folder tree and grid view.
- [ ] Textures and meshes have thumbnails that generate on first view and persist in `.rustforge/thumbnails/`.
- [ ] Dropping a file from OS explorer into the browser copies it to the project and imports it.
- [ ] Dragging a mesh from browser to viewport spawns it at the cursor raycast hit point.
- [ ] Dragging an asset onto an `AssetRef<T>` field in Inspector assigns it; type mismatches are rejected.
- [ ] Selecting an asset in the browser shows its import settings in the Inspector; changing settings and reimporting updates the scene.
- [ ] External file changes (edit PNG in Photoshop, save) trigger re-import and hot-reload in the viewport within 1 second.
- [ ] Renaming an asset externally preserves its GUID; existing scene references still resolve.
- [ ] Deleting an asset externally produces a broken-reference warning in scenes that reference it, but doesn't crash.
- [ ] GLTF import produces stable sub-asset GUIDs across re-imports.
- [ ] Search / filter in browser works on name and type.
- [ ] `rustforge-core` still builds without the `editor` feature.
