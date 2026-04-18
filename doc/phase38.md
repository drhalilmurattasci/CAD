# Phase 38 — Modding Runtime & Live Ops

Phase 11 opened the *editor* to third-party code: plugins add panels, importers, commands, and custom asset editors, running inside the author's RustForge instance. Phase 38 is the shipped-game mirror image. A game built on RustForge can boot on a player's machine, find a `mods/` folder, layer user content on top of the base game, hand scripted mods a sandboxed WASM runtime with a capability envelope the game author controls, and — orthogonally — pull down a patch the publisher cut yesterday without redownloading 40 GB of unchanged art.

Phase 38 reuses load-bearing primitives already in the engine: Phase 9's cook pipeline produces mod archives, Phase 9's pak format layers, Phase 7's WASM host runs scripts, Phase 11's capability system gates what mods touch, and the Phase 5 GUID registry makes asset overrides unambiguous. Nothing new is invented that didn't have to be.

## Goals

By end of Phase 38:

1. **`.rmod` archive format** — a cooked subset of a project packaged by the same Phase 9 pipeline, loadable at runtime by a shipped game.
2. **Runtime mod loader** — games discover `mods/` at a platform-appropriate path, parse manifests, resolve load order, and layer assets atop the base pak.
3. **Asset override by GUID** — a mod's asset with the same GUID as a base asset replaces it; base files are never rewritten on disk.
4. **WASM mod scripts with capabilities** — mods can ship `.wasm` modules that run in the Phase 7 host under capability grants the game author defines; default is zero capabilities.
5. **Mod SDK export** — a game shipping mod support can publish a subset of its project (scene templates, reflection metadata, sample mod) that players open in the full RustForge editor.
6. **Live content patching** — a tool diffs two cooked builds and produces a delta pak containing only changed assets, applied atomically at install-time.
7. **Rollback safety** — failed patch apply leaves the install in the pre-patch state, never partially updated.
8. **DLC as signed `.rmod`** — DLC is a bundled mod gated on platform entitlement check.
9. **UGC moderation hooks** — games with user-submitted content can register scan-before-load callbacks that veto or quarantine mods.

Phase 38 is not a store, not an anti-cheat, not a DRM scheme. It is the plumbing that makes mods, DLC, and patches *possible* and *safe by default*; distribution is delegated to whatever platform the game ships on.

## 1. `.rmod` archive format

A `.rmod` is a Phase 9 pak with a mod manifest glued to the front. A cooked mesh inside a mod uses the same on-disk representation as a cooked mesh in the base game — same cooker, same format.

```
.rmod layout:
  [ magic     "RFMOD1\0\0"              (8 bytes)              ]
  [ manifest  length-prefixed TOML      (u32 len + bytes)      ]
  [ pak       embedded Phase 9 pak      (magic RFPAK1...)      ]
  [ signature optional Ed25519 sig over (manifest || pak)  64B ]
```

Manifest:

```toml
schema       = 1
id           = "com.player.extra-swords"
name         = "Extra Swords"
version      = "0.3.1"
game_id      = "com.studio.knight-adventure"
game_version = "^1.2"                   # semver range the mod targets
api_version  = "0.3"                    # rustforge-mod-api minor

[author]
name = "someplayer"
url  = "https://github.com/someplayer/extra-swords"

[capabilities]
# Empty by default. Game declares what's grantable; player grants per-mod.
scripts    = false
scene_read = false
network    = false
fs_read    = []

[entitlement]           # Only set for DLC
required_id = ""        # platform entitlement id
signer      = ""        # base64 Ed25519 public key

[overrides]             # Advisory; real answer from pak index at load
assets = ["0x8f2a9c...", "0x1d4e7b..."]
```

Manifest stays TOML for the same reason `runtime.manifest` is: the file is tiny, opened once, opacity costs more than bytes save. `game_id` / `game_version` let a shipped game reject a mod built for a different title or incompatible version; semver compatibility rules are the game author's call.

## 2. Runtime mod loader

### 2.1 Discovery

Platform-appropriate default paths:

```
Windows:  %APPDATA%\<project_name>\mods\
Linux:    $XDG_DATA_HOME/<project_name>/mods/  (fallback ~/.local/share/...)
macOS:    ~/Library/Application Support/<project_name>/mods/
```

Override via `runtime.manifest`:

```toml
[mods]
enabled      = true
search_paths = ["user://mods", "game://builtin_mods"]
max_mods     = 64
```

`user://` is the user data dir, `game://` the install dir. Mod support is a runtime boolean — loader code is always linked; `enabled = false` short-circuits discovery. No conditional compilation, keeps the shipping binary monomorphic.

### 2.2 Load order

```toml
# <user_data>/mods/load_order.toml
order = [
    "com.studio.official-extras",
    "com.player.extra-swords",
    "com.player.ui-tweaks",
]
disabled = ["com.player.broken-experiment"]
```

Order is **first-wins for non-overrides, last-wins for overrides**: a new asset added by a mod appears once regardless of position; an override of an existing GUID walks the order tail-first. Last listed = highest priority. Missing entries append in discovery order (alphabetical by `id`). An in-game Mod Manager UI is a game-author concern; the engine ships a sample, not a mandate.

### 2.3 Loader flow

```
boot
  ├── load base pak (Phase 9)
  ├── discover mods/*.rmod
  │     verify magic; parse manifest; check game_id/version/api_version
  │     if signed: verify; if DLC: query entitlement
  │     reject? log, skip, surface in Mod Manager
  ├── resolve load order (load_order.toml + discovery default)
  ├── build override table: GUID -> (Base|Mod(id), offset, len, compression)
  │     walk mods in priority order; last write wins for override slot
  ├── for each scripted mod: instantiate WASM under grants, fire on_load
  └── boot entry scene (via override table → base if no override)
```

The override table sits in front of the Phase 9 `AssetSource::Pak` resolver:

```rust
pub enum AssetSource {
    Loose { root: PathBuf },
    Pak   { base: PakIndex, mods: Vec<ModPak>, overrides: OverrideTable },
}

impl AssetSource {
    pub fn load_bytes(&self, guid: AssetGuid) -> Result<Bytes> {
        if let AssetSource::Pak { base, mods, overrides } = self {
            if let Some(OverrideEntry { mod_idx, pak_offset, .. }) = overrides.get(&guid) {
                return mods[*mod_idx].read_blob(*pak_offset);
            }
            return base.read_blob(guid);
        }
        // loose-file path unchanged
    }
}
```

One branch in the hot path; the override lookup is a hashmap read.

## 3. Asset override by GUID

Non-destructive is the invariant: the base pak's bytes on disk never change. A mod that "replaces the hero mesh" cooks its own mesh with the same `AssetGuid` and the override table routes reads to the mod's blob.

Authoring flow (in the editor pointed at a Mod SDK project):

1. Player opens the SDK project — base assets appear read-only with GUIDs already assigned.
2. Player drags `hero.gltf` in; by default it gets a fresh GUID.
3. To *override*, right-click the base asset and pick "Create Override" — the editor creates a mod-side asset whose `.meta` carries the base asset's GUID plus `override_of = "<base_guid>"`.
4. Cook produces a cooked blob under the same GUID; the `.rmod` pak contains it.

GUIDs are `u128`; collision between a mod's new asset and a base asset is astronomically unlikely. Collision between two *mods* both overriding the same GUID is expected — that's the whole point; load order resolves it. Non-override collisions (two mods adding *new* assets under the same GUID) are a bug: log a warning naming both mods and take the higher-priority one.

### 3.1 What cannot be overridden

- **Code.** Rust is linked into `rustforge-game` at build time (Phase 9). Scripted mods (§4) extend behavior; they don't replace it.
- **`runtime.manifest`.** The base game's boot manifest is authoritative; a mod cannot redirect the entry scene.
- **Capability declarations.** The game's grant policy is in its own manifest, not modifiable.

Everything else — meshes, textures, materials, audio, scenes, prefabs, localization — is fair game.

## 4. Scripted mods & capability gating

Scripted mods reuse the Phase 7 WASM host verbatim. A `.rmod` whose manifest declares `scripts = true` and whose pak contains `.wasm` blobs registers those modules as mod-owned scripts when the mod loads.

### 4.1 Capability model

Mods do not choose their capabilities — the game author chooses what capabilities *exist as grantable* and what each one actually does. Phase 11's capability enum becomes a game-author-defined vocabulary in Phase 38:

```rust
// Game author declares in the game's mod support config.
pub struct ModCapabilityPolicy {
    pub grants: Vec<CapabilityGrant>,
}

pub struct CapabilityGrant {
    pub id:          String,           // "spawn_enemy"
    pub description: String,           // shown to player
    pub host_fns:    Vec<HostFnId>,    // what this capability unlocks
    pub risk:        RiskLevel,        // Low / Medium / High
}
```

A mod requests capabilities by id:

```toml
[capabilities]
scripts   = true
requested = ["spawn_enemy", "read_inventory"]
```

First load of a mod with any non-Low capability pops a confirmation listing the requested capabilities, descriptions, and author; the player's choice persists in `load_order.toml`. No silent escalation.

### 4.2 Default grants

Zero. A mod without explicit grants can compute, can't observe, can't mutate, can't reach outside. It can still register systems the game author explicitly exposed as side-effect-free.

### 4.3 Hard denials

Regardless of grants: no direct filesystem, no direct network, no process spawn, no dynamic library load, no raw pointer tricks. The host exposes *no* `fs_read` / `fs_write` / `http_*` functions to mods directly. If a game wants mods to persist state or fetch data, it exposes a scoped host function under a capability, with URL allowlists baked into the game.

### 4.4 High-capability warning

Any grant marked `RiskLevel::High` triggers a non-dismissable toast on first load: "Mod X is requesting *network access* — only load mods from authors you trust."

## 5. Mod SDK export

A game shipping mod support publishes an SDK — a cut-down RustForge project the player opens in the full editor.

```
knight-adventure-sdk/
├── rustforge-project.toml      # marks as SDK-mode
├── .sdk/
│   ├── base_manifest.toml      # GUIDs, kinds, names of base assets
│   ├── reflect.ron             # component type registry snapshot
│   ├── game_id                 # "com.studio.knight-adventure"
│   └── capabilities.toml       # grantable capability vocabulary
├── scene_templates/            # example scenes the player can copy
├── sample_mods/hello-sword/
└── scripts_api/                # WASM host function declarations
```

Player opens the project, sees base assets read-only, authors a mod, and uses `File → Export Mod` — which invokes `rustforge-cli package-mod`. The Phase 9 cooker runs with an SDK-mode flag that restricts outputs to mod-legal kinds. SDK projects carry `[sdk]` in `rustforge-project.toml`; the editor hides the Build button.

## 6. Live content patching

Patches are pak diffs.

### 6.1 Diff tool

```
rustforge-cli patch diff \
    --old ./dist/v1.2.0 \
    --new ./dist/v1.2.1 \
    --out ./patches/v1.2.0-to-v1.2.1.rpatch
```

A `.rpatch` is structurally a `.rmod` with two extra fields in its manifest:

```toml
schema       = 1
kind         = "patch"
from_version = "1.2.0"
to_version   = "1.2.1"

[assets]
changed = ["0x8f2a9c...", "0x1d4e7b..."]
removed = ["0xdeadbe..."]
```

The embedded pak contains only the `changed` blobs. `removed` GUIDs are hidden via a patch-level overlay; the base pak is not rewritten in place.

### 6.2 Hash-addressed skip

Every cooked blob has a `cooked_hash` (Phase 9 §3.1). The diff tool walks `(guid, cooked_hash)` pairs; matching hashes contribute zero bytes. Unchanged textures, audio, meshes all skip.

### 6.3 Apply flow

```
apply ./patches/v1.2.0-to-v1.2.1.rpatch to ./game_install/

 1. verify install version == patch.from_version; mismatch → bail
 2. verify patch magic + optional signature
 3. stage: cp game_install → game_install.staging (hardlinks where possible)
 4. for each changed asset: write new blob, update staging pak index
 5. for each removed asset: remove index entry
 6. write new runtime.manifest with patched version; fsync staging
 7. atomic swap:  mv game_install → .rollback;  mv .staging → game_install
 8. ok:   rm -rf .rollback
    fail: mv game_install → .broken; mv .rollback → game_install; surface error

state machine:
   ready ──(verify ok)──▶ staging ──(write ok)──▶ swapped ──(cleanup)──▶ ready'
     │                      │                       │
     ▼ verify fail           ▼ write fail           ▼ post-swap fail
   abort                 rollback               restore-from-rollback
```

Hardlinks keep cost O(changed). FAT32 / exFAT lack hardlinks; fall back to full copy with a warning.

### 6.4 Content versioning & manifest fetch

```toml
[content]
version      = "1.2.1"
manifest_url = "https://cdn.example.com/knight-adventure/manifest.json"
```

On boot, the launcher fetches the remote manifest, compares versions, downloads the appropriate `.rpatch`, and applies via the flow above. The *engine* ships the diff and apply tools; the *launcher* is the game's problem (Steam, Epic, or a bespoke wrapper).

### 6.5 Hot content reload (dev only)

```toml
[content.dev]
hot_reload_port = 7823
```

The editor pushes a `.rpatch` over localhost to a running game; the game applies it in-memory (not on disk) and re-routes affected asset loads. Shipping builds with `[content.dev]` fail the Phase 9 reproducibility check.

## 7. DLC

DLC is a signed `.rmod` with an entitlement gate:

```toml
# dlc_manifest excerpt
schema = 1
id     = "com.studio.knight-adventure.dlc.dragon-isles"
kind   = "dlc"

[entitlement]
required_id = "studio.knight.dlc.dragon_isles"   # platform-specific id
signer      = "base64:AAECAw..."                 # publisher public key
```

DLC flow on load:

1. Verify signature against `signer`. Mismatch → refuse to load.
2. Query platform entitlement: `ctx.platform.has_entitlement(required_id)`.
3. If no entitlement: refuse to load; optionally show "Own this DLC? [Restore Purchases]" — game-author UX, engine provides the hook.
4. If entitled: load exactly like a normal mod, with capabilities granted as the publisher declared in the DLC manifest (usually permissive — the publisher is trusting themselves).

Platform entitlement abstraction:

```rust
pub trait PlatformEntitlements: Send + Sync {
    fn has_entitlement(&self, id: &str) -> bool;
    fn list(&self) -> Vec<String>;
    fn on_change(&self, cb: Box<dyn Fn() + Send + Sync>);
}
```

Implementations: Steam (`steamworks`), Epic (EOS), generic "license file" for non-store builds. Consoles are out of Phase 38 scope, same as Phase 9; the trait accommodates them later.

## 8. UGC moderation hooks

Games accepting user-submitted content (UGC) can register scan callbacks invoked before a mod loads:

```rust
pub trait ModScanner: Send + Sync {
    fn scan(&self, mod_manifest: &ModManifest, pak: &ModPak) -> ScanVerdict;
}

pub enum ScanVerdict {
    Allow,
    Quarantine { reason: String },   // disables by default, shows reason
    Deny { reason: String },         // refuses to load
}

engine.register_mod_scanner(Box::new(BlocklistScanner::new(...)));
engine.register_mod_scanner(Box::new(PreviewRenderScanner::new(...)));
```

Scanners run in declaration order; first non-Allow wins. Typical uses: blocklist by id or author key, asset-size sniff, sandboxed preview render + classifier for visible-content policies, WASM import audit catching capability lies. Scanning is opt-in — a game registering no scanner gets none. Avoids baking any one moderation policy into the engine.

## 9. Build order within Phase 38

Each step is independently testable.

1. **`.rmod` format** — writer + reader + manifest parser. Hand-built round-trip test, no game integration yet.
2. **`rustforge-cli package-mod`** — Phase 9 cooker in SDK-mode. Produces a real `.rmod` from a sample SDK project.
3. **Runtime loader (no overrides)** — discover `mods/`, parse manifests, build mod list, reject malformed archives. New mod-GUID assets load alongside base.
4. **Override table** — GUID lookup in front of `AssetSource::Pak`. Verify mod replaces hero mesh without touching base pak bytes.
5. **Load order resolution** — `load_order.toml`, priority rules, conflict logging.
6. **WASM capabilities** — game-author vocabulary, `requested`, player grant flow, zero defaults. Refusal test for ungranted `network`.
7. **Mod SDK export** — editor `File → Export Mod`; SDK-mode project flag; read-only base assets.
8. **Patch diff** — `rustforge-cli patch diff`; round-trip test diff v1→v2, apply to v1, byte-compare to v2.
9. **Patch apply** — staging, atomic swap, rollback-on-failure. Fuzz by killing the process at every step boundary.
10. **Content manifest fetch** — optional launcher primitive; Phase 23 opt-in network stack.
11. **Hot content reload** — dev-mode localhost push; Shipping with `[content.dev]` fails reproducibility CI.
12. **DLC signing + entitlement** — Ed25519 verify; `PlatformEntitlements` trait + mock + Steam adapter reference.
13. **UGC moderation hooks** — `ModScanner` trait, quarantine state, registration API.
14. **Docs** — Mod SDK author guide, capability authoring guide, patch publisher guide.

## 10. Scope boundaries — what's NOT in Phase 38

- ❌ **Hosted mod marketplace / workshop.** Games link out to Steam Workshop, mod.io, or GitHub. The engine ships no UI, no backend, no curation. A reference `ModIO` adapter plugin exists but is a plugin, not engine core.
- ❌ **Cheat-proof mod verification.** Signatures prove authorship, not multiplayer fairness. That's anti-cheat, a separate phase if ever.
- ❌ **DRM enforcement.** DLC entitlement is a soft gate through platform APIs; determined users bypass it locally. No obfuscation, no integrity checks on `rustforge-game`, no kernel-mode protection.
- ❌ **Ad SDK integration.** Games wanting ads integrate a third-party SDK like any other native library.
- ❌ **Telemetry SaaS.** Hooks exist (mod-load, patch-apply events), opt-in, delivered to game-registered callbacks. Engine does not phone home. Phase 23's "no first-party telemetry" rule applies.
- ❌ **KYC for modders.** Distribution-layer concern.
- ❌ **Mod conflict auto-resolution.** Last-wins with a warning; no three-way merge UI.
- ❌ **Content re-cook on player machines.** Players consume cooked `.rmod` archives; they don't run the cooker.
- ❌ **Non-WASM scripting runtimes.** WASM only, per Phase 7 / Phase 11.
- ❌ **Sub-blob differential patches.** A changed texture ships as a whole new blob. Revisit for gigantic assets later.
- ❌ **Cross-version savegame migration for mods.** Mod-defined schemas are the game author's problem.

## 11. Risks & gotchas

- **Override collision shipped but hidden.** Two mods claim GUID `X`; load-order winner wins, loser invisible. If the winner is broken the game loads broken assets. Mod Manager surfaces conflicts visibly, not only in logs.
- **Mod lies about capabilities.** `requested = ["score_bonus"]` whose WASM imports `http_request` must fail instantiation on the missing import — the host must not silently bind stubs for ungranted functions.
- **Version ranges advisory only.** `^1.2` against `1.3.0` may or may not work; engine enforces the range but runtime incompatibility is the mod author's problem.
- **Patch apply interrupted.** Check for orphan `.staging` / `.rollback` on boot; resume or clean up with a toast. Never assume atomicity the filesystem doesn't guarantee. Probe hardlink support at staging start; fall back to copy on FAT32 / network drives.
- **DLC signature TOCTOU + offline entitlement.** Verify over mmap'd bytes and load through the same mmap. Default offline policy: 14-day last-known-good cached entitlement, then require online re-check. Document per-platform.
- **Scripted mod panic storm.** Phase 7 host traps and disables on repeated panic; Phase 38 surfaces auto-disabled state in Mod Manager.
- **Mod scenes reference base entity by `SceneId`.** `SceneId`s shift when base is patched. Dangling references log a warning and become `Entity::NULL`. Mods should reference asset GUIDs instead.
- **Mod SDK leaks unshipped base assets.** Export walks the base project; secret assets stored alongside leak. Provide `[sdk.excluded]` in `rustforge-project.toml`.
- **UGC scanner slow on cold install.** Cache verdicts by `(mod_id, mod_version, scanner_version)`.
- **Publisher forgets to bump `importer_version`.** Cooker output changes, `.meta` hash doesn't, patch diff is empty. CI lint cooks the sample project before/after importer edits and fails on bytes-changed-but-version-same.
- **Capability grants across mod upgrades.** Persisted grants are capability-scoped: new capability in new version re-prompts; existing still-requested grants carry over.

## 12. Exit criteria

Phase 38 is done when all of these are true:

- [ ] `.rmod` writer and reader round-trip; malformed archives yield actionable errors, never panics.
- [ ] `rustforge-cli package-mod` produces a `.rmod` from an SDK project using the Phase 9 cooker with no duplicated code.
- [ ] A shipped game with `[mods] enabled = true` discovers mods in the platform user data dir and layers them over base content.
- [ ] A mod asset with the same GUID as a base asset replaces it at runtime; base pak bytes on disk are unchanged.
- [ ] Two mods overriding the same GUID resolve by `load_order.toml` priority with a named-both warning.
- [ ] A WASM mod with zero granted capabilities instantiates and runs; gated host functions return `CapabilityDenied`. A mod requesting `network` without a matching game declaration is refused at load.
- [ ] `RiskLevel::High` capability request on first load shows a user confirmation that persists across sessions.
- [ ] Mod SDK export produces an openable project with read-only base assets and grantable capability vocabulary. `rustforge-cli patch diff` produces a `.rpatch` whose size is proportional to changed content; applying it to version N yields a byte-identical clean build at N+1.
- [ ] Killing patch apply between any two steps leaves the install fully old or fully new; orphan staging/rollback dirs are resolved on next boot.
- [ ] DLC with signature mismatch refuses to load; with valid signature but no entitlement refuses to load; with both loads normally.
- [ ] `ModScanner` callbacks run before every mod load; quarantine and deny verdicts are honored. `[content.dev]` in a Shipping manifest fails the Phase 9 reproducibility CI check.
- [ ] `[mods] enabled = false` ships with zero mod behavior observable; flipping the flag enables full mod support on the same binary.
- [ ] `rustforge-core` still builds without the `editor` feature; no Phase 9 or Phase 11 invariants regressed; shipping binary size delta is measured.
