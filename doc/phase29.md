# Phase 29 — Shared DDC & Team Build Server

Phase 5 gave each workstation a per-project cooked-asset cache under `.rustforge/cache/<guid>.bin`. Phase 9 extended that cache with content-hashed `.meta` entries and made cooking deterministic. Both were designed around a single developer, and both quietly assume the cheapest place to find a cooked artifact is the local disk. That assumption breaks the moment a team grows past three people: every new hire, every fresh clone, every branch switch that bumps an importer version ends in the same humiliating ritual — forty minutes staring at a progress bar while the machine re-cooks assets three other people already cooked this morning.

Unreal solved this a decade ago with a Derived Data Cache: a content-addressable store keyed by `(source_hash, importer_version_hash, platform)` that falls through local → shared → cold re-cook. Phase 29 ports that idea to RustForge, but stays in the engine's spirit — self-host only, boring protocols, no accounts, no SaaS. The shared cache is a small HTTP server teams run themselves, fed by CI and by editors on clean builds. A later sub-phase adds an optional cook farm that dispatches jobs to a worker pool, but that is explicitly scoped down: build-farm operations is its own discipline and we will not try to reinvent it here.

This phase is team-scale infrastructure. It is the equivalent of "your studio has a build server now." It does not replace Git, does not host source, does not ship assets to end users — only engineering-time cooked derivations.

## Goals

By end of Phase 29:

1. **Local DDC** formalized as a separate cache from Phase 5's per-project `.rustforge/cache/`, living at `~/.cache/rustforge/ddc/` and shared across every project on the workstation.
2. **Shared DDC server** — `rustforge-ddc-server` binary, self-hosted, HTTP/S, bearer-token auth, GET/PUT/HEAD on content hashes.
3. **Three-tier fall-through** — local → shared → cold re-cook, transparent to the importer call sites.
4. **Upload policy** — editor uploads to shared cache only on clean-build success; per-project config; contractors get read-only tokens.
5. **CI integration** — every merged PR cooks and uploads, populating the shared cache so fresh clones hit a warm tier.
6. **Optional cook farm** — master + worker binaries that dispatch cook jobs through a simple job queue; workers consult local + shared DDC before doing real work.
7. **Editor surface** — status-bar cache-hit-rate indicator, Project Settings panel for DDC server config with a test-connection button.
8. **Offline-first** — network unavailable never errors; fall through to local cache, log the miss.

## 1. Cache key — what "content-addressed" means here

Phase 5's `.meta` already records an `importer_version: u32` and the SHA-256 of the source file. Phase 29 combines them into a single 32-byte **DDC key**:

```rust
pub struct DdcKey([u8; 32]);

impl DdcKey {
    pub fn compute(
        source_hash: &[u8; 32],
        importer_id: &str,        // "gltf", "png", "wav"
        importer_version: u32,
        platform: Platform,       // Windows / Linux / macOS / Any
        settings_hash: &[u8; 32], // per-asset .meta settings, hashed
    ) -> Self {
        let mut h = blake3::Hasher::new();
        h.update(source_hash);
        h.update(importer_id.as_bytes());
        h.update(&importer_version.to_le_bytes());
        h.update(&(platform as u8).to_le_bytes());
        h.update(settings_hash);
        let out = h.finalize();
        Self(*out.as_bytes())
    }

    pub fn hex(&self) -> String { hex::encode(self.0) }
}
```

Opinion: `blake3` for this layer, not SHA-256. The `.meta` source hash stays SHA-256 because it is a Git-friendly canonical form; the DDC key is internal plumbing that is never committed, and `blake3` is measurably faster on the hot path where the editor hashes a `.meta` settings blob every keystroke in the importer inspector.

Keys are stable across machines and across time as long as the five inputs are stable. They are **not** stable across importer version bumps — that is the point. Bumping `importer_version` from 7 to 8 invalidates every cached entry for that importer without touching anyone's local disk; the cache simply stops producing hits and the CI warm-fill repopulates it.

## 2. Local DDC — not the same as the project cache

The Phase 5 `.rustforge/cache/` directory remains. It is the project's *output* — the cooked bytes that Phase 9's pak step reads. It lives alongside the project, gets cleaned by `cargo clean`, and is GUID-keyed (one entry per asset, not per-source-version).

The Phase 29 local DDC is additive and orthogonal: `~/.cache/rustforge/ddc/` is keyed by `DdcKey`, shared across every project on the machine, and content-addressed. Two projects that import the same `.gltf` at the same importer version produce the same `DdcKey` and share the same cached bytes.

```
~/.cache/rustforge/ddc/
├── ab/
│   └── cd/
│       └── abcd...ef.blob        # raw cooked bytes
│       └── abcd...ef.meta        # { size, mtime, importer, created_at }
├── index.sqlite                  # LRU bookkeeping
└── tombstones/                   # evicted keys, metadata-only, 30-day audit
```

Two-level sharding (first two bytes, then next two) keeps any directory under ~65k entries on reasonable fill rates. The `index.sqlite` tracks LRU order and total size for eviction; we never scan the filesystem to make policy decisions.

```rust
pub struct LocalDdc {
    root: PathBuf,
    index: SqliteIndex,
    cap_bytes: u64,
}

impl LocalDdc {
    pub fn get(&self, key: &DdcKey) -> Option<Bytes> { /* ... */ }
    pub fn put(&self, key: &DdcKey, bytes: &[u8], meta: &EntryMeta) { /* ... */ }
    pub fn evict_lru(&self) { /* honor cap_bytes */ }
}
```

Eviction is LRU on access time, capped by `cap_bytes` (default 20 GB, user-configurable). Tombstones are kept — 30 bytes of `{key, evicted_at, reason}` — purely so `rustforge-cli ddc audit` can answer "why did my build cold-cook this asset yesterday." Tombstones expire after 30 days.

## 3. Shared DDC server — protocol

The wire protocol is deliberately small. Four endpoints, all keyed by hex-encoded `DdcKey`:

```
GET    /v1/cache/<hex_key>              → 200 bytes | 404
HEAD   /v1/cache/<hex_key>              → 200 headers | 404
PUT    /v1/cache/<hex_key>              ← bytes; 201 Created | 409 Exists
GET    /v1/health                       → 200 { version, uptime, entries, bytes }
```

Headers on GET/HEAD:

```
X-Rustforge-Importer: gltf
X-Rustforge-Importer-Version: 7
X-Rustforge-Platform: windows-x86_64-msvc
X-Rustforge-Created: 2026-04-12T09:14:22Z
X-Rustforge-Size: 1483220
Content-Type: application/octet-stream
```

Request flow for the common case:

```
editor                 local DDC          shared DDC           importer
  │                       │                   │                    │
  │──lookup(key)─────────▶│                   │                    │
  │                       │──miss─────────────▶                    │
  │                       │                   │                    │
  │                       │◀──────GET /v1/cache/<key>──────────────│
  │                       │                   │                    │
  │                       │◀──200 bytes───────│                    │
  │◀──bytes───────────────│  (populated)      │                    │
  │                                                                │
  │ (cache warm; importer never runs)                              │
```

Cold path, on the machine that actually does the cook:

```
editor                 local DDC          shared DDC           importer
  │                       │                   │                    │
  │──lookup(key)─────────▶│                   │                    │
  │                       │──miss────────────▶│                    │
  │                       │◀────404───────────│                    │
  │◀──miss────────────────│                   │                    │
  │                                                                │
  │──cook(source)─────────────────────────────────────────────────▶│
  │◀──bytes────────────────────────────────────────────────────────│
  │                                                                │
  │──put(key, bytes)─────▶│                   │                    │
  │                       │──PUT /v1/cache/<key>──────────────────▶│
  │                       │◀──201─────────────│                    │
```

### 3.1 Server implementation

`rustforge-ddc-server` is a single binary, ~1k lines of code, built on `axum` + `tokio`. Storage backends behind a trait:

```rust
#[async_trait]
pub trait Storage: Send + Sync {
    async fn get(&self, key: &DdcKey) -> Result<Option<Bytes>>;
    async fn head(&self, key: &DdcKey) -> Result<Option<EntryMeta>>;
    async fn put(&self, key: &DdcKey, bytes: Bytes, meta: EntryMeta) -> Result<PutOutcome>;
    async fn stats(&self) -> Result<Stats>;
}

pub enum PutOutcome { Created, AlreadyExists }
```

Two backends ship: `FilesystemStorage` (the same two-level sharding as local) and `S3Storage` for teams that already run an object store. No database for the blob itself — the key *is* the filename.

Config:

```toml
# /etc/rustforge-ddc/config.toml
bind = "0.0.0.0:8410"
storage = { kind = "filesystem", root = "/var/lib/rustforge-ddc" }
max_bytes = "500GB"
eviction = "lru"

[auth]
tokens_file = "/etc/rustforge-ddc/tokens.toml"

[tls]
cert = "/etc/rustforge-ddc/fullchain.pem"
key  = "/etc/rustforge-ddc/privkey.pem"
```

Opinion: ship TLS as first-class. Bearer tokens over plain HTTP are indefensible for anything that crosses a LAN boundary, and every studio that looks at this as "just a cache" will put it on a public VPS within a month. Make the unsafe path require a deliberate `--insecure-http` flag.

## 4. Authentication — tokens, not identity

```toml
# tokens.toml
[[tokens]]
name   = "ci-writer"
hash   = "blake3:a3f4..."
scope  = "read_write"

[[tokens]]
name   = "dev-readwrite"
hash   = "blake3:22cc..."
scope  = "read_write"

[[tokens]]
name   = "contractor-readonly"
hash   = "blake3:91fe..."
scope  = "read"
```

Clients send `Authorization: Bearer <token>`. The server hashes and compares; tokens are never stored in cleartext. There is no user model, no login flow, no refresh. Teams that need SSO put the server behind their existing reverse proxy (Cloudflare Access, oauth2-proxy, Tailscale ACLs) and disable the built-in bearer check with `auth.mode = "trust_proxy"`.

Opinion: resist every request to grow this. Per-asset ACLs, ownership, quotas, audit log dashboards — these are the features that turn a 1,000-line cache server into a 50,000-line access-control system that nobody wants to maintain. If a team genuinely needs those, they have already outgrown self-host and should run the server behind enterprise SSO.

## 5. Tiered fall-through — the call site

The importer call site after Phase 29 looks identical to Phase 5's, wrapped by one helper:

```rust
pub async fn ensure_cooked(
    ddc: &DdcService,
    source: &SourceAsset,
    importer: &dyn Importer,
) -> Result<CookedAsset> {
    let key = DdcKey::compute(
        &source.content_hash,
        importer.id(),
        importer.version(),
        Platform::current(),
        &source.settings_hash(),
    );

    // Tier 1: local.
    if let Some(bytes) = ddc.local.get(&key) {
        metrics::cache_hit(Tier::Local);
        return Ok(CookedAsset::from_bytes(bytes));
    }

    // Tier 2: shared. Never blocks on network longer than 500ms before falling through.
    match timeout(Duration::from_millis(500), ddc.shared.get(&key)).await {
        Ok(Ok(Some(bytes))) => {
            ddc.local.put(&key, &bytes, &EntryMeta::now(importer));
            metrics::cache_hit(Tier::Shared);
            return Ok(CookedAsset::from_bytes(bytes));
        }
        Ok(Ok(None)) => metrics::cache_miss(),
        Ok(Err(e)) => tracing::warn!(?e, "shared DDC error, falling through"),
        Err(_)     => tracing::warn!("shared DDC timeout, falling through"),
    }

    // Tier 3: cold cook.
    let cooked = importer.cook(source).await?;
    let bytes  = cooked.serialize();
    ddc.local.put(&key, &bytes, &EntryMeta::now(importer));

    // Upload policy gates the shared push.
    if ddc.policy.should_upload(&cooked, importer) {
        let _ = ddc.shared.put(&key, bytes.clone()).await; // fire-and-forget
    }
    metrics::cache_hit(Tier::Cold);
    Ok(cooked)
}
```

The 500 ms timeout is not arbitrary — it is the point past which a developer on a slow VPN is better off re-cooking than waiting. Editor preferences expose this as `shared_ddc.timeout_ms`.

## 6. Upload policy

```rust
pub struct UploadPolicy {
    pub enabled: bool,
    pub on_editor: UploadTrigger,  // Never | OnSave | OnCleanBuild
    pub on_ci:     UploadTrigger,  // Always | OnlyMainBranch | Never
    pub skip_if_failed_import: bool,
}

pub enum UploadTrigger { Never, OnSave, OnCleanBuild, Always }
```

Defaults matter:

- `on_editor = OnCleanBuild` — a developer running Play-in-Editor does not push every cook to the shared cache. They push only when they successfully complete a full build, which is the point at which the artifacts are known-good.
- `on_ci = OnlyMainBranch` — branch builds are ephemeral; main-line builds are the canonical warm fill.
- `skip_if_failed_import = true` — if any importer in the graph errored, nothing from that build uploads. Poisoning the shared cache with a broken asset from one bad workstation is the single most costly failure mode of a system like this.

Project manifest entry:

```toml
# rustforge-project.toml
[ddc]
shared_url = "https://ddc.internal.studio/"
token_env  = "RUSTFORGE_DDC_TOKEN"   # never the token itself in source control
on_editor  = "clean_build"
on_ci      = "main_only"
offline    = false
```

The token **never** goes in `rustforge-project.toml`. Always an env var reference; Phase 12 added a `.gitignore` template that already excludes `*.secret.toml` but the convention here is stricter: only env-var indirection is permitted in the manifest parser. A literal `token = "..."` is a hard parse error with a suggested fix.

## 7. CI integration — the warm-fill loop

The shared cache is only useful if it is full. The model is:

```
┌──────────────────────────────────────────────────────────────┐
│ PR merged to main                                            │
│   │                                                          │
│   ▼                                                          │
│ CI runner: rustforge-cli build --config=development          │
│                                \ --ddc-upload                │
│   │                                                          │
│   ├── Phase 9 cook step ──▶ produces cooked assets           │
│   ├── ensure_cooked() caches each in local + shared          │
│   └── exits 0                                                │
│   │                                                          │
│   ▼                                                          │
│ shared DDC now holds every cooked artifact for main@HEAD     │
│                                                              │
│ next developer runs `git pull && rustforge-editor`           │
│   │                                                          │
│   ▼                                                          │
│ first open — every asset hits Tier 2 (shared)                │
│ no cold cook, no local waiting                               │
└──────────────────────────────────────────────────────────────┘
```

The CLI flag `--ddc-upload` forces `on_ci = Always` for that invocation, regardless of project config. The CI job sets `RUSTFORGE_DDC_TOKEN` from its secret store. Nothing about this requires a dedicated CI plugin — a 10-line GitHub Actions step is sufficient.

Opinion: resist the urge to write first-party CI plugins. GitHub Actions, GitLab CI, Buildkite, Jenkins — each has a plugin ecosystem that immediately goes stale. Ship the CLI; let studios write the 10 lines themselves.

## 8. Cook farm — scoped carefully

The cook farm is the optional, ambitious half of this phase. It is scoped carefully because build-farm operations (queue semantics, worker lifecycle, failure recovery, bisection under flaky workers) is an entire discipline that we refuse to rebuild badly.

Minimal design: one `rustforge-cook-master` binary, N `rustforge-cook-worker` binaries.

```
  client                master                    workers
    │                     │                          │
    │──submit(job)───────▶│                          │
    │                     │──dispatch(task_1)───────▶│ (worker A)
    │                     │──dispatch(task_2)───────▶│ (worker B)
    │                     │                          │
    │                     │◀──result(task_1)─────────│
    │                     │◀──result(task_2)─────────│
    │◀──complete──────────│                          │
```

Protocol choice: gRPC with streaming, because we already pull it in for the profiler's trace endpoint in Phase 10 — no new dependency.

Worker loop:

```rust
async fn worker_loop(master: MasterClient, ddc: DdcService) {
    let mut stream = master.subscribe_tasks().await?;
    while let Some(task) = stream.next().await {
        // Consult DDC first — the whole point.
        let key = DdcKey::compute(/* ... */);
        if ddc.local.get(&key).is_some() || ddc.shared.head(&key).await.ok().flatten().is_some() {
            master.complete(task.id, CookResult::CacheHit).await?;
            continue;
        }
        let cooked = task.cook().await;
        ddc.local.put(&key, &cooked.bytes, /* ... */);
        ddc.shared.put(&key, cooked.bytes.clone()).await?;
        master.complete(task.id, CookResult::Cooked).await?;
    }
}
```

Scope ❌ for the farm:

- No bisection-on-failure.
- No worker auto-scaling.
- No cross-worker dependency resolution — tasks are embarrassingly parallel by the time they reach the queue.
- No priority scheduling beyond "oldest first."
- No per-user quotas.

If a studio needs any of those, they can drive the same workers from Nomad, Kubernetes, or whatever orchestrator they already run. The farm is a convenience for teams without that infrastructure; the day it stops being convenient, they graduate off it.

## 9. Editor UI

Two touchpoints, both minimal.

**Status bar, right-hand side**, always visible when a project is open:

```
  [main] ● 3 dirty   |   DDC: 94% hit (local 62, shared 32)   |   CPU: 12%
```

The rate is over a rolling 1000-cook window; clicking it opens a histogram panel with the tier breakdown and recent miss reasons. Reasons are short strings: `importer_bump`, `source_changed`, `offline`, `shared_404`.

**Project Settings → DDC panel:**

```
┌─ Derived Data Cache ────────────────────────────────────────┐
│                                                             │
│ Shared server URL:  [https://ddc.internal.studio/       ]   │
│ Auth token env var: [RUSTFORGE_DDC_TOKEN                ]   │
│                                                             │
│ Upload on editor:   ( ) Never  (•) Clean build  ( ) On save │
│ Upload on CI:       ( ) Never  (•) Main branch  ( ) Always  │
│                                                             │
│ Offline mode:       [ ] Force offline (skip shared tier)    │
│                                                             │
│             [ Test connection ]   [ Clear local cache ]     │
│                                                             │
│ Connection:  ✓ healthy  ·  server 0.4.2  ·  RTT 18ms        │
└─────────────────────────────────────────────────────────────┘
```

"Test connection" does a `GET /v1/health` with the configured token and reports latency and version. Anything else in this panel is reading and writing the project manifest's `[ddc]` section; no hidden state.

## 10. Security posture

Treat the shared DDC as **semi-trusted**. Even a studio's own cache can be compromised (leaked token, stale backup restored, disgruntled employee). Defenses in depth:

1. **Hash-validate on fetch.** The DDC key is a hash of inputs, not a hash of the cooked bytes, so fetching a bad blob at a good key is theoretically possible if the cook was non-deterministic. Phase 9 guarantees determinism, so after fetch we optionally re-hash the bytes and compare against a sidecar `expected_output_hash` stored in the entry metadata. Mismatch = evict + refuse to use + log a loud warning.
2. **Never execute fetched binaries.** Cooked assets are data: meshes, textures, audio, scene BLOBs. The importers that produce them are trusted Rust code. The DDC tier never loads a `.dll`, `.so`, or `.dylib` from the cache — shader bytecode, yes, but passed through the GPU driver's own validation, not executed on the CPU.
3. **Size caps at every layer.** Server caps total bytes; per-entry cap (default 512 MB) rejects absurd uploads; client cap on download buffer.
4. **No source upload, ever.** The DDC is for cooked derivations only. Source `.gltf`, `.png`, `.wav` live in Git (or Git-LFS from Phase 12). The protocol has no endpoint that accepts a source file; the CLI has no flag that would put one. Uploading source would mean forking the source-of-truth off Git, which is precisely the anti-pattern Phase 12 rejected.

## 11. Privacy and the non-goals

- The DDC is not a general file server. No arbitrary-key PUT; all keys are `DdcKey` hex. Reject anything else with 400.
- It is not a source-control replacement. Git is source of truth; DDC is a regeneratable cache and can be wiped at any time with zero data loss.
- It is not a hosted service. We will not run "RustForge Cloud." Self-host only. Ship the binary, publish a reference Docker image, stop there.
- It is not an asset CDN for shipped games. Shipped games read from `assets.pak` per Phase 9. The DDC never appears in a shipped runtime; the `editor` feature gates the client library out of game builds entirely.

## 12. Offline behavior

The single most common failure mode is "developer on a plane." The fall-through from §5 handles this, but the surface details matter:

- A 500 ms timeout on the first shared GET of a session, 150 ms on subsequent GETs after a successful response. A single failure flips the service into a 5-minute **offline window** during which every shared tier call short-circuits to `None` without touching the network.
- Offline window is logged once, not per miss. Status bar shows `DDC: offline (retry in 4m)`.
- Exit offline window on either the timer expiring or the user clicking "Test connection."
- `--offline` CLI flag forces offline mode regardless of reachability; useful for CI reproducibility tests.

## 13. Versioning and schema evolution

The DDC key algorithm itself is versioned:

```rust
const DDC_KEY_ALGORITHM_VERSION: u8 = 1;
```

The algorithm version is baked into the key. Bumping it invalidates every entry globally — a nuclear option reserved for actual bugs in the key derivation. Importer version bumps are the normal, non-nuclear invalidation path and cost nothing because they change the key inputs, not the key algorithm.

Server `/v1/health` reports the accepted algorithm versions; a client with a version the server does not accept logs a warning and falls through to local only. This lets us deploy a v2 server before v2 clients exist (accept both), then deprecate v1 after migration.

## Build order

1. **Local DDC crate.** `rustforge-ddc-local` — filesystem sharding, SQLite index, LRU eviction, key computation. Shippable standalone; Phase 5 importers can adopt it without any server involvement.
2. **Shared server protocol.** Wire format, bearer auth, TLS config, filesystem backend. Binary: `rustforge-ddc-server`. Smoke test with `curl`.
3. **Client library + fall-through.** `rustforge-ddc-client` crate that wraps local + shared into the `DdcService` used by importers. The `ensure_cooked` helper from §5.
4. **Upload policy + project manifest.** `[ddc]` section, env-var indirection for tokens, the `on_editor` / `on_ci` defaults.
5. **CI integration.** `rustforge-cli build --ddc-upload`. Publish reference GitHub Actions and GitLab CI snippets in the docs.
6. **Cook farm (optional, may slip to 29.1).** Master + worker binaries, gRPC protocol, worker loop with DDC consultation.
7. **Editor UI.** Status bar indicator, Project Settings panel, test-connection button.

## Scope ❌

- Hosted service offering. Self-host only. There is no `cloud.rustforge.dev`.
- DDC as source-control replacement. Source stays in Git; DDC is regeneratable cache.
- Multi-tenant SaaS. Each server instance serves one team; teams run their own servers.
- P2P cache sharing between workstations. Central server, no mesh, no gossip, no DHT.
- Incremental cook dependency graph analyzer — "if I changed this material, which textures actually need recooking?" This is a future phase; for now, importer version bumps are the coarse-grained invalidation tool.
- General-purpose blob store. DDC keys only; no arbitrary uploads.
- User identity, ACLs per asset, dashboards, quotas. Bearer tokens with `read` / `read_write` scope is the whole authorization model.

## Risks

- **Cache poisoning via non-determinism.** Phase 9 requires deterministic cooks but a regression in an importer (e.g. iteration order over a `HashMap`) would produce varying bytes for the same key. Mitigation: the optional `expected_output_hash` check in §10; a CI lint that re-cooks every asset twice and fails the build on diff.
- **Token leakage.** Bearer tokens in env vars leak to CI logs surprisingly often. Mitigation: documented `::set-secret::` pattern for the common CI systems; server logs never echo tokens; a `--redact` mode on the CLI that scrubs the token from its own output.
- **Cost-of-a-bad-push.** One developer's broken laptop uploading garbage poisons the cache for the whole team. Mitigation: `skip_if_failed_import = true` default; `expected_output_hash` mismatch on fetch; a `rustforge-cli ddc invalidate <importer>` admin command wired to the server's DELETE endpoint (which is intentionally admin-only, not part of the public protocol).
- **Working-copy divergence.** A dev cooks on importer v7, a CI job subsequently cooks the same source at v7, both upload — race on PUT. Mitigation: 409 on existing key, client treats as success. Entries are immutable once written; updating a key requires a new key (new importer version).
- **Cook farm sprawl.** Worker pool becomes pet infrastructure, on-call, Slack channels about "the farm is down." Mitigation: documented "turn it off" path — every worker's job can be done on a developer's workstation; the farm is an optimization, not a dependency.
- **Offline window too aggressive.** 5 minutes is guesswork. Mitigation: configurable; telemetry counter exposes the actual distribution so we can tune defaults at 30.x.

## Exit criteria

- A fresh clone of a project with a populated shared DDC opens in under 30 seconds on a 100 Mbit link with zero cold cooks.
- `rustforge-cli build --ddc-upload` on CI populates the shared cache such that a second CI run on the same commit reports 100% Tier 2 hits.
- Status-bar cache-hit rate is visible and updates live during editor use.
- Network outage mid-session does not produce any error dialog; status bar shows offline; editor continues to cook locally.
- `rustforge-ddc-server` binary runs behind an nginx reverse proxy with TLS termination; a read-only contractor token is rejected on PUT with 403 and accepted on GET with 200.
- Importer version bump on one importer invalidates only that importer's cache; every other importer's entries remain hot.
- An intentionally corrupted entry in the shared cache is detected on fetch, evicted locally, logged, and re-cooked — without user intervention.
- Cook farm (if shipped in 29.0) processes a 500-asset project in under half the single-machine wall-clock time with 4 workers, and with no worker hitting the cold path for any asset previously cooked on main.
