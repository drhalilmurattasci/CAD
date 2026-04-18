# Phase 9 — Build & Packaging

Phase 8 closed the authoring-tool arc: specialized editors, profiler, every panel wired through the command stack. You can now sit inside RustForge for hours and make a game. What you can't do is *ship* it — the game binary still reaches into the project folder at runtime, reads `.ron` scenes from `scenes/`, pulls cooked assets from `.rustforge/cache/<guid>.bin`, and assumes `rustforge-project.toml` is one directory up. None of that is true on an end user's machine.

Phase 9 builds the pipeline that takes a project from "runs under the editor" to "a folder you zip and hand to a player." It cooks every asset non-interactively, bundles the results next to a stripped binary compiled without the `editor` feature, and surfaces all of it through a Build dialog and a headless CLI suitable for CI. The `editor` feature flag, the `SceneId` scheme, the GUID registry, and the PIE snapshot invariant are all load-bearing here — nothing in this phase contradicts them.

## Goals

By end of Phase 9:

1. **Shippable artifacts.** One button in the editor produces a folder containing `game.exe` (or equivalent), a pak of cooked assets, baked scenes, and a runtime manifest — nothing else required.
2. **Headless cook.** `rustforge-cli build` runs the entire pipeline without the editor UI, for CI.
3. **Three build configurations** — Debug, Development, Shipping — with explicit, enumerated differences (no `editor` feature in Shipping).
4. **Three target platforms** — `windows-x86_64-msvc`, `linux-x86_64-gnu`, `macos-aarch64`. No web, no consoles.
5. **Incremental cooking.** An unchanged source asset is never re-cooked; decision driven by content hash recorded in `.meta`.
6. **Out-of-process builds.** The editor spawns the cook job and `cargo`; the UI stays responsive with streaming logs.
7. **Runtime manifest.** The shipped game reads a tiny manifest on boot to locate the entry scene and the pak.
8. **Determinism.** Cooking the same inputs twice produces byte-identical cooked outputs (matters for caching and reproducible builds).

## 1. The build pipeline, end to end

```
 project folder
     │
     ├── cook step ──▶ .rustforge/cache/<guid>.bin   (already exists per-file from Phase 5)
     │                 + content hash in .meta
     │
     ├── scene bake ──▶ cooked scenes: .ron → .scene.bin
     │
     ├── pak step ───▶ assets.pak (concat cooked + index)
     │
     ├── cargo build --no-default-features --features=game
     │                 [--release] [--profile shipping]
     │
     └── stage step ─▶ out/<target>/<config>/
                         ├── rustforge-game(.exe)
                         ├── assets.pak
                         ├── scenes.pak             (optional; see §4.2)
                         └── runtime.manifest
```

Five steps, each one callable independently from the CLI. The editor's Build dialog drives them in order, streaming progress. Failure in any step leaves the stage folder partially populated; the next build starts fresh (`stage` rm-rf its target subfolder first).

## 2. Build configurations

```rust
pub enum BuildConfig {
    Debug,       // cargo build              + editor feature OFF + dev-assertions ON
    Development, // cargo build --release    + editor feature OFF + tracing=info + asserts
    Shipping,    // cargo build --profile shipping + editor OFF  + tracing=warn  + panic=abort
}
```

Concrete differences:

| Axis              | Debug            | Development       | Shipping          |
|-------------------|------------------|-------------------|-------------------|
| `editor` feature  | off              | off               | off               |
| `cargo` profile   | `dev`            | `release`         | `shipping`        |
| `panic`           | unwind           | unwind            | abort             |
| `lto`             | off              | thin              | fat               |
| `debug` info      | full             | line tables       | none              |
| tracing level     | debug            | info              | warn              |
| asset compression | none             | zstd-3            | zstd-19           |
| strip             | no               | no                | yes               |

Add a dedicated profile in the workspace `Cargo.toml`:

```toml
[profile.shipping]
inherits = "release"
lto = "fat"
codegen-units = 1
panic = "abort"
strip = "symbols"
```

Don't reuse `release` for shipping — you lose the ability to profile a release-with-symbols build.

The `editor` feature being off in **all three** build configs is intentional. The editor is a separate binary (`rustforge-editor`) that links against the same crates with the feature on. Phase 8's zero-cost-without-editor pattern already covered this; Phase 9 is what actually exercises it at ship time.

## 3. Asset cooking in non-interactive mode

Phase 5's importer pipeline already takes source → cooked for one asset at a time, triggered by the file watcher. Phase 9 wraps it in a batch driver:

```
crates/rustforge-cli/src/cook.rs
crates/rustforge-core/src/asset/cook_all.rs
```

```rust
pub struct CookPlan {
    pub project_root: PathBuf,
    pub out_cache:    PathBuf,       // default: <project>/.rustforge/cache
    pub force:        bool,          // skip incremental check
    pub parallelism:  usize,         // default: num_cpus
}

pub fn cook_all(plan: &CookPlan, progress: &mut dyn CookProgress) -> Result<CookReport>;
```

`CookProgress` is a trait with `on_start`, `on_asset_start(guid)`, `on_asset_done(guid, status)`, `on_log(level, msg)`. The CLI implements it as stderr lines; the editor implements it as a channel into the Build dialog log view.

### 3.1 Incremental cooking

`.meta` files already exist per Phase 4 §3. Extend the schema with a content-hash field written by the importer:

```toml
# assets/meshes/player.gltf.meta
guid = "0x8f2a9c..."
importer = "gltf"
importer_version = 3
source_hash  = "blake3:a1b2c3..."   # NEW: hash of the *source* file bytes
cooked_hash  = "blake3:f4e5d6..."   # NEW: hash of the cooked output bytes
settings = { ... }
```

Decision on `cook_all`:
1. Read `.meta`. If `source_hash` matches the current file's blake3 and `importer_version` matches the running importer, **skip**.
2. Otherwise re-cook, rewrite both hashes.

Recommend **blake3** over SHA-256 — roughly 5× faster on warm caches and that matters when you're hashing every mesh source on every build.

`force: true` bypasses the skip. Use it when the importer changed but its version bump was forgotten; plumb it through CLI as `--force-cook`.

### 3.2 Parallelism

Cooking is embarrassingly parallel per-asset. Use `rayon::par_iter` over the asset list. Cap at `num_cpus`; a `--jobs N` CLI flag lets CI tune it. Cross-asset dependencies (Phase 5's `ImportResult::dependencies`) force a topological order — do a Kahn sort first, then parallelize within each layer.

### 3.3 Determinism

Two cooks of the same source must produce byte-identical `.bin` files, or the content-hash skip logic will thrash. Pitfalls:

- `HashMap` iteration order in serializers — use `BTreeMap` anywhere the output is bytes.
- Embedded timestamps — none. If a library you wrap inserts them, patch or wrap.
- Float NaN bit patterns — normalize to a canonical NaN on write (or reject NaN values during import).

Add a CI job that cooks the sample project twice and diffs `cache/`. Any drift fails the job.

## 4. Bundling — pak vs. loose files

### 4.1 Recommendation: single `assets.pak`

Two viable layouts:

**A) Loose files.** Ship `cache/<guid>.bin` verbatim. Runtime reads them by path.
**B) Single `assets.pak`.** Concatenate all cooked blobs with an index at the head or tail.

**Recommend B, for shipping builds.** Reasons:

- **Install integrity.** One file either arrives intact or doesn't. Loose files get partial-download or antivirus-quarantine edge cases.
- **Open-file count.** On Windows, cold-opening 10k small files during level load is 2–3× slower than one mmap'd pak.
- **Compression.** A global zstd frame per entry (or one big dictionary-trained frame) compresses better than gzip-per-file.
- **Tamper resistance.** Not security, just "users don't accidentally edit `player_mesh.bin` and wonder why their install is broken."

The editor and Development config keep loose files for iteration — Shipping builds the pak. Don't do pak-in-debug: it makes hot-reload pointless.

### 4.2 Pak layout

```
assets.pak:
  [ magic   "RFPAK1\0\0"    (8 bytes)                          ]
  [ header  { entry_count, index_offset, index_len, flags } 32B ]
  [ blob 0 ]
  [ blob 1 ]
  ...
  [ blob N-1 ]
  [ index:
      for each entry:
        guid:        u128
        offset:      u64
        uncompressed_len: u64
        compressed_len:   u64
        compression:      u8   (0=none, 1=zstd)
        kind:             u8   (mesh, texture, audio, scene, ...)
    ]
```

Index at the tail, not the head — the pak writer can stream blobs without seeking and then append the index. Mmap at load, read index once, resolve `AssetGuid` → `(offset, len, compression)` directly.

Scenes: treat cooked scenes (`.scene.bin`, see §5) as just another entry kind. Don't split into `scenes.pak` — the extra file buys nothing. The table above mentioned it as an option; the recommendation is a single `assets.pak`.

### 4.3 Runtime loader changes

Phase 5's runtime asset cache currently maps `AssetGuid` → `PathBuf`. Wrap it in an enum:

```rust
pub enum AssetSource {
    Loose { root: PathBuf },         // editor, development
    Pak   { mmap: Arc<Mmap>, index: PakIndex },   // shipping
}
```

One dispatch at the `load_bytes(guid)` level, zero changes to callers. Gate pak code behind `#[cfg(not(feature = "editor"))]` — the editor never reads a pak (always loose files via the project folder).

## 5. Scene baking

Phase 4's `.ron` scene files are editor-friendly but slow to parse on a cold start (3–5× slower than a binary round-trip at realistic scene sizes, measured on other engines). Shipping bakes them to a binary form:

```rust
pub fn bake_scene(src: &Path, dst: &Path) -> Result<()> {
    let scene = SceneFile::load_ron(src)?;        // existing Phase 4 loader
    let bytes = bincode::serialize(&scene)?;       // or postcard; see below
    fs::write(dst, bytes)
}
```

`SceneId` stays a `u64`; `EntityRef` still serializes as `SceneId`. The binary scene is loaded via the *same* `SceneFile::into_world` path Phase 4 built, just with a different deserializer selected by file extension. The snapshot/restore invariant from Phase 7 is untouched — PIE always operates on in-memory `SceneFile`, never on disk bytes.

Prefer **postcard** over bincode — smaller, schema-evolution friendly, and the cooked size difference matters when a project has 500 scene files.

Cooked scenes go into `assets.pak` as entries with `kind = Scene`, keyed by `SceneGuid` (which is an `AssetGuid` — scenes have `.meta` like everything else in Phase 4 §4).

## 6. Runtime manifest

On boot, the shipped game needs to know one thing: *which scene do I load first?* Plus a handful of config the editor knows and the game doesn't.

```
runtime.manifest   (TOML, ~40 lines max)
```

```toml
schema = 1
engine_version = "0.9.0"
project_name   = "Knight Adventure"

[boot]
entry_scene = "0x8f2a9c..."      # SceneGuid
default_window = { width = 1280, height = 720, title = "Knight Adventure" }

[assets]
pak  = "assets.pak"
mode = "pak"                      # or "loose" for dev builds

[log]
level = "warn"                    # matches BuildConfig
file  = "game.log"
```

Keep it TOML and human-readable. The file is tiny, read once, and shipping binary size is not the bottleneck. A binary manifest saves <1 ms of startup; not worth the opacity.

Shipped `main.rs` reduces to:

```rust
fn main() -> Result<()> {
    let manifest = RuntimeManifest::load("runtime.manifest")?;
    let mut engine = Engine::boot(&manifest)?;
    engine.load_scene(manifest.boot.entry_scene)?;
    engine.run_main_loop();
    Ok(())
}
```

No project path. No `rustforge-project.toml`. No `editor` feature. That's the whole boot path.

## 7. Build UI inside the editor

```
┌─ Build ────────────────────────────────────────────────┐
│  Target          [ windows-x86_64-msvc      ▼ ]         │
│  Configuration   [ Shipping                 ▼ ]         │
│  Output folder   [ C:\dev\knight\dist           ] [...]│
│                                                         │
│  [ ] Force re-cook all assets                           │
│  [x] Open output folder when done                       │
│                                                         │
│  ┌ Log ────────────────────────────────────────────┐   │
│  │ cook      42/317  meshes/knight_helmet.gltf     │   │
│  │ cook      43/317  textures/grass_albedo.png     │   │
│  │ pak       0/317                                  │   │
│  │ cargo     building rustforge-core...             │   │
│  └──────────────────────────────────────────────────┘   │
│                                                         │
│             [▄▄▄▄▄▄▄▄▄▄▄▄░░░░░░]  63%                   │
│                                                         │
│                     [ Cancel ]   [ Build ]              │
└─────────────────────────────────────────────────────────┘
```

Rules:

- **Out of process.** The editor spawns `rustforge-cli build …` with the same arguments. Stdout/stderr stream back over a pipe into the log view. Editor input (viewport, inspector, PIE) stays fully responsive.
- **Cancellation.** Cancel kills the child process, tears down the partial `out/` stage. The cook cache in `.rustforge/cache/` is **not** invalidated — next build continues where this one stopped.
- **One build at a time.** Disable the Build button while a build is running; show a cancel instead.
- **Don't gate PIE.** The user can still press Play while a build runs. They're separate processes; the editor's in-process engine state is untouched.
- **Last settings persist** — remember target/config/output in editor prefs (`.rustforge/editor.toml` from Phase 8 §5.1).

## 8. Headless CLI

```
rustforge-cli build \
    --project ./knight \
    --target windows-x86_64-msvc \
    --config shipping \
    --out ./dist \
    [--force-cook] \
    [--jobs 8] \
    [--no-cargo]        # cook + pak only, skip cargo build
```

Exit codes:

- `0` — success.
- `1` — user error (missing project, bad target).
- `2` — cook failure (asset couldn't be imported).
- `3` — cargo failure.
- `4` — stage failure (disk full, permission).

Stable exit codes matter for CI. Don't collapse everything to `1`.

Also expose `rustforge-cli cook` (step 1+2 only) and `rustforge-cli pak` (step 3 only) — useful for iterating on the pipeline itself without a full cargo rebuild each time.

## 9. Build order within Phase 9

1. **Shipping profile + game binary target.** Add `[profile.shipping]`, carve a new `rustforge-game` bin crate (no default features), verify it compiles without `editor`.
2. **`cook_all` batch driver.** Wrap Phase 5 importers, no incremental check yet. Verify all assets in the sample project cook end-to-end.
3. **Content-hash incremental cook.** Extend `.meta` schema with `source_hash` / `cooked_hash`; skip unchanged. Determinism test (cook twice, diff cache).
4. **Scene bake.** `.ron` → postcard binary. Round-trip test: load ron, bake, load baked, verify identical world.
5. **Pak writer + reader.** Implement the format in §4.2. Round-trip test against loose files.
6. **Runtime `AssetSource::Pak` loader.** Swap based on config; the editor still uses loose.
7. **Runtime manifest.** Define schema, writer in CLI, loader in game boot path.
8. **`rustforge-cli build` end-to-end.** Glue stages 2–7 together. Runs on sample project for `linux-x86_64-gnu` and produces a bootable artifact.
9. **Build dialog in editor.** Dock panel, spawn CLI, stream logs. Cancellation.
10. **Windows + macOS targets.** Cross-target matrix in CI; macOS needs a signed-ish identity to run locally but signing itself is out of scope.
11. **CI reproducibility check.** Run `build --config shipping` twice on the sample project, diff the stage folder; fail on drift.

Each step is independently testable — stop after (5) and you have a reproducible headless cook with loose files, usable for CI long before the UI lands.

## 10. Scope boundaries — what's NOT in Phase 9

- ❌ **Code signing** (Authenticode, Apple notarization). Noted that it exists; solve in a follow-up phase.
- ❌ **Installer generation** (MSI, DMG, deb/rpm). Ship a folder; users zip it.
- ❌ **Auto-update / patching.** Out of scope — a separate delivery-layer phase.
- ❌ **Web / WASM target.** Browser rendering constraints reshape too much.
- ❌ **Console targets** (PS5, Xbox, Switch). Per-vendor SDKs, NDAs — separate.
- ❌ **Mobile targets** (Android, iOS). Touch input, app lifecycle, store packaging — separate.
- ❌ **Differential / delta paks.** Ship a whole pak per build; patching is out of scope.
- ❌ **Asset streaming / on-demand download.** Pak is loaded whole.
- ❌ **Encryption of paks.** Obfuscation is not security; skip.
- ❌ **Per-platform asset variants** (different texture formats for different GPUs). Single cooked form; the runtime transcodes if needed.
- ❌ **Cargo workspace-crate customization per target.** Stick to `cargo build --target X`.

## 11. Risks & gotchas

- **Incremental cache poisoning.** A bug in an importer produces a bad `cooked.bin`, `.meta` records its hash, next cook skips it. Always-valid. Fix: `--force-cook` CLI, and bump `importer_version` whenever importer logic changes (Phase 5 already prescribed this; actually enforce it).
- **Non-deterministic serializers.** `HashMap` in `Serialize` silently destroys reproducibility. Add a test that sorts the cooked-byte hashes and diffs across runs — catches it immediately.
- **`cargo build` reusing an incompatible target dir.** The editor build and the shipping build share `target/` and have different feature sets, which triggers full rebuilds every switch. Use separate target dirs per config: `target-editor/`, `target-ship/` via `CARGO_TARGET_DIR`. Costs disk, saves minutes.
- **Pak index at tail + truncated file.** A crash during pak write produces blobs but no index. On next load the game can't tell it's corrupt — mmap succeeds, index read garbage. Write a magic footer *after* the index, validate on load, bail loudly.
- **File watchers running during a cook.** Phase 5's watcher fires on `.meta` updates; the cook writes `.meta`; watcher re-imports; loop. Phase 5 §3.1 already has the extension filter — verify it actually covers the batch cooker's writes too.
- **Cross-compilation to macOS from Linux.** Realistically requires `osxcross` or a Mac runner. Document: "macOS builds happen on a Mac runner." Don't try to make cross-compilation a supported path.
- **Panic in a script at shipping's `panic=abort`.** The game hard-crashes instead of catching. Acceptable for shipping — players get a crash dialog, not a recoverable error — but be explicit in docs. Development config keeps `panic=unwind` to aid debugging.
- **Manifest schema drift.** A shipped game has manifest schema v1; a user downloads a new build with schema v2. If they extract-over the old folder the two files mismatch. Require the game to assert `schema == EXPECTED_SCHEMA` and bail with a readable error, never try to guess.
- **Pak mmap on Windows + antivirus.** AV scanners sometimes hold mmap'd files. Usually fine, but the failure mode ("game won't launch after I updated") is bad. Have a loose-files fallback: if pak open fails, look for a `loose/` sibling folder and use that source.
- **Cook job uses all cores, editor becomes unusable.** The cook is out-of-process but still competing for CPU. Default `--jobs` to `num_cpus - 1` when launched from the editor, `num_cpus` from the CLI.
- **Stage folder left over from a previous target.** User builds Linux, then Windows, into the same `--out`. Leftover `rustforge-game` (no `.exe`) ships alongside `rustforge-game.exe`. Always rm-rf `out/<target>/<config>/` at the start of stage.
- **Forgetting a dependency.** A scene references a material that references a texture; the texture never gets pulled into the pak because nothing lists it. The Phase 5 `ImportResult::dependencies` graph is the single source of truth — walk it transitively from the entry scene and any referenced scenes, pull the closure, and *error* on unreachable assets rather than silently shipping them (or silently dropping them).
- **"Works in editor, broken when shipped" feature drift.** A system behind `#[cfg(feature = "editor")]` gets accidentally depended on at runtime. Mitigation: the CI matrix must build `rustforge-game` without the feature on every PR — same as Phase 7 §13 and Phase 8 exit criteria already require.

## 12. Exit criteria

Phase 9 is done when all of these are true:

- [ ] `cargo build -p rustforge-game --profile shipping` produces a runnable binary with no `editor` feature linked.
- [ ] `rustforge-cli cook` cooks every asset in the sample project; second invocation re-cooks zero assets.
- [ ] Two consecutive `rustforge-cli build --config shipping` runs produce byte-identical stage folders (reproducibility test green).
- [ ] `.meta` schema includes `source_hash` and `cooked_hash` fields; changing a source file re-cooks only that file.
- [ ] `.ron` scenes bake to a postcard binary; baked scenes round-trip to an identical in-memory world.
- [ ] `assets.pak` format spec'd, writer produces it, loader reads it, tampering with the tail magic is detected.
- [ ] Runtime asset source switches between `Loose` (editor/dev) and `Pak` (shipping) with no call-site changes.
- [ ] `runtime.manifest` is produced in every build and successfully boots the shipped game to the entry scene.
- [ ] `rustforge-cli build` supports `windows-x86_64-msvc`, `linux-x86_64-gnu`, `macos-aarch64` with stable exit codes.
- [ ] Build dialog in the editor spawns the CLI out of process, streams logs, and can be cancelled without leaving the editor wedged.
- [ ] PIE remains fully functional while a build is in progress.
- [ ] A build with `--force-cook` re-cooks every asset regardless of hash; without it, an unchanged project cooks nothing.
- [ ] Unreachable assets (not referenced from the entry scene's transitive closure) produce a build error, not a silent ship.
- [ ] CI runs the full pipeline on every PR for at least `linux-x86_64-gnu`, including the determinism diff.
- [ ] `rustforge-core` still builds and runs without the `editor` feature; no Phase 7 / Phase 8 invariants regressed.
