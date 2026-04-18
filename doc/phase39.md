# Phase 39 — ML & AI Integration

Phase 26 gave RustForge a classical AI stack — nav meshes, behavior trees, blackboards, perception, EQS — the deterministic, debuggable foundation every shipped game needs. Phase 35 landed motion matching; Phase 36 opened the door to neural rendering with a vendor-upscaler adapter; Phase 11 defined how untrusted code plugs in safely. Phase 39 is where RustForge stops being a spectator to the 2024-2026 ML explosion and starts exposing modern machine learning as a first-class engine capability — LLMs for dialogue, diffusion for asset generation, and neural techniques woven into animation and rendering.

This is the phase where RustForge can genuinely lead Unreal. Epic ships a mature classical stack and some ML-flavored research demos; nobody — yet — ships a production engine with a clean pluggable ML adapter layer, declared cost/privacy policy, sandboxed inference out-of-process, and an editor UX that treats "generate a PBR material from a prompt" as the same kind of operation as "import a PNG". Phase 39 is explicitly additive: nothing here replaces Phase 26, Phase 35, or Phase 36. Classical AI still ticks. Motion matching still blends clips. DLSS still upscales. ML sits beside each of those with its own trait, its own budget, and its own kill switch.

## Goals

By end of Phase 39:

1. **`MlProvider` trait** — the single extension seam for every ML feature. Local and remote backends implement it; the editor and runtime never see a raw `llama.cpp` or `openai` call.
2. **Local LLM backend** — `llama.cpp` adapter as the reference local provider; `mlx` adapter for Apple Silicon; `ollama` adapter for users who already run a local daemon.
3. **Remote LLM backend** — OpenAI, Anthropic, and a generic `custom` HTTP/JSON adapter behind the same trait.
4. **Streaming dialogue hook** — an `LlmDialogue` runtime that streams tokens into the Phase 44 dialogue UI (forward-referenced) and into BT tasks as blackboard writes.
5. **Prompt template registry** — named, versioned templates as first-class assets; parameter typing; test harness.
6. **Response caching** — content-addressed cache keyed by `(provider_id, model_id, prompt_hash, params_hash)` to prevent repeated remote calls during iteration.
7. **Cost & privacy gating** — per-project policy declared up front; editor refuses remote calls until the user consents; runtime enforces the policy on shipped builds.
8. **Diffusion asset generation** — in-editor "Generate Texture" and "Generate Material" panels. Local via `candle-rs` or `diffusers-rs`; remote via Dall-E / Midjourney adapters. Output routes through the Phase 5 importer.
9. **ML animation synthesis (experimental)** — text-to-motion diffusion models feed generated clips into the Phase 35 motion matching database. Gated behind an experimental flag.
10. **Neural rendering extensions** — DLSS Frame Generation, ReSTIR for RT variance reduction, Neural Radiance Caching (NRC). Each extends the Phase 36 neural adapter layer; each is tier-gated and optional.
11. **Out-of-process inference** — remote providers run in a sandboxed worker; local providers can opt into out-of-process isolation. A crashing model never takes the editor with it.
12. **Editor ML panel** — single pane listing providers, model sizes, offline mode toggle, session cost counter, cache stats.
13. **Offline-first default** — a fresh project boots with remote providers disabled. Users opt in explicitly.

## 1. The `MlProvider` trait

Every ML capability in Phase 39 goes through one trait. Local llama.cpp, remote Anthropic, diffusion pipelines, motion diffusion models — all implement the same surface so the editor, runtime, and plugin code above the provider layer are interchangeable.

```rust
// crates/rustforge-ml-api/src/provider.rs
pub trait MlProvider: Send + Sync + 'static {
    /// Stable identifier: "llama-cpp", "anthropic", "openai", "candle-sdxl".
    fn id(&self) -> &'static str;

    /// Declared capabilities this provider supports.
    fn capabilities(&self) -> MlCapabilities;

    /// Locality — drives cost/privacy gating.
    fn locality(&self) -> MlLocality;

    /// Called once at registration. May load models, open sockets, spawn worker.
    fn init(&mut self, ctx: &MlContext) -> Result<(), MlError>;

    /// Text completion / chat. Streaming is the default shape.
    fn text(&self, req: TextRequest) -> BoxStream<'static, Result<TextChunk, MlError>>;

    /// Image generation. One-shot; no streaming.
    fn image(&self, req: ImageRequest) -> BoxFuture<'static, Result<ImageResponse, MlError>>;

    /// Motion generation. Experimental; gated.
    fn motion(&self, req: MotionRequest) -> BoxFuture<'static, Result<MotionResponse, MlError>>;

    /// Cost estimate before the call. Used by the consent prompt.
    fn estimate_cost(&self, req: &AnyRequest) -> CostEstimate;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MlLocality {
    Local,                  // runs on this machine, no network
    LocalDaemon,            // localhost socket (ollama, mlx-serve)
    Remote { endpoint: &'static str },
}

bitflags! {
    pub struct MlCapabilities: u32 {
        const TEXT_COMPLETION = 1 << 0;
        const CHAT            = 1 << 1;
        const EMBEDDING       = 1 << 2;
        const IMAGE_GEN       = 1 << 3;
        const MOTION_GEN      = 1 << 4;
        const TOOL_USE        = 1 << 5;
        const VISION_INPUT    = 1 << 6;
    }
}
```

Rules that keep the trait honest: providers never touch filesystem, network, or GPU directly — `MlContext` hands them a scoped sandbox matching their manifest; `text()` is streaming-by-default (non-streaming callers `.collect()`; no second method); `estimate_cost()` runs locally without a round-trip and may be pessimistic; registration goes through the Phase 11 `PluginRegistry`; unknown request shapes return `MlError::Unsupported`, never a panic or silent empty response.

## 2. Local LLM backend

`llama.cpp` is the reference; three more adapters ship alongside to cover the common deployment shapes.

```
  crates/rustforge-ml-llama/
  ├── Cargo.toml                # llama-cpp-2 bindings, pinned
  ├── src/
  │   ├── lib.rs                # LlamaProvider: MlProvider
  │   ├── worker.rs             # out-of-process worker (§7)
  │   ├── gguf.rs               # model discovery under $RUSTFORGE_MODELS
  │   └── tokenize.rs
  └── models/                   # .gitignored; user-managed
```

Provider lineup at the end of Phase 39: `llama-cpp` (Local, FFI; Win/mac/Linux), `mlx` (Local; Apple Silicon), `ollama` (LocalDaemon; HTTP to `localhost:11434`), `openai` / `anthropic` (Remote), `custom-http` (Remote; self-hosted JSON).

Opinion: ship the wrapper, not the model. GGUF sniffing of a `models/` directory is 500 lines; bundling a 4 GB checkpoint into the installer slows every release. Users run `rustforge models pull llama-3.1-8b-instruct-q4` and the CLI drops the file where the provider expects it.

## 3. Remote providers and the request flow

Every remote call follows the same path. The editor or the game asks the provider registry for a capability; the registry returns a handle; the handle's call goes through the cost/privacy gate, then through the cache, then across the IPC boundary to the worker, then out to the network.

```
  caller (editor / game / plugin)
        │
        ▼
  ┌──────────────────────────┐
  │ MlProviderRegistry       │ ── capability + policy dispatch
  └──────────────────────────┘
        │
        ▼
  ┌──────────────────────────┐
  │ CostPrivacyGate          │ ── reads project policy, may prompt user
  └──────────────────────────┘
        │                 consent denied ──▶ MlError::PolicyDenied
        ▼
  ┌──────────────────────────┐
  │ ResponseCache            │ ── hash(prompt, params) → hit? return cached
  └──────────────────────────┘
        │                 cache hit ──▶ stream cached tokens
        ▼
  ┌──────────────────────────┐
  │ IPC boundary (bincode)   │
  └──────────────────────────┘
        │
        ▼
  ┌──────────────────────────┐
  │ worker process           │ ── owns the provider, the model, the socket
  └──────────────────────────┘
        │
        ▼
    local model  |  remote API
```

The boundary between the editor / game process and the worker is bincode over a pair of pipes (Unix domain sockets on posix, named pipes on Windows). Streaming tokens come back as framed `MlEvent` messages. A worker crash closes the pipes; the registry observes the hang-up, surfaces `MlError::WorkerCrashed`, and the caller handles it like any other error.

## 4. Prompt template registry

Prompts are assets, not string literals scattered across gameplay code. The registry supports versioning, typed parameters, and a test harness.

```rust
// crates/rustforge-ml-prompt/src/template.rs
#[derive(Asset, Reflect, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub name: SmolStr,
    pub version: u32,
    pub provider_hint: Option<SmolStr>,    // "anthropic", "llama-cpp", or None
    pub system: String,
    pub user: String,                      // {{param}} placeholders
    pub params: Vec<PromptParam>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stop: Vec<String>,
}

#[derive(Reflect, Serialize, Deserialize, Clone)]
pub struct PromptParam {
    pub name: SmolStr,
    pub ty: PromptParamType,               // String, Int, Float, BbKey, AssetRef
    pub required: bool,
}
```

Templates are `.rpt` RON files that round-trip through an editor that reuses the Phase 20 asset editor widget. Each template ships with a snapshot test — a frozen (seed, prompt, expected) triple so prompt edits that drift the output surface in CI, not on a playtest two weeks later.

Render is strict: an undeclared `{{foo}}` in the template or a missing required param fails loudly at authoring time. No silent empty strings.

## 5. Cost & privacy gating

The hardest part of shipping ML in an engine is not the model — it is preventing a designer from accidentally spending $4000 on tokens during an overnight test build, or leaking a studio's unreleased dialogue to a third-party API. Phase 39 treats this as a first-class constraint.

```toml
# project.toml — ml section
[ml.policy]
mode = "local-only"                              # local-only | allowlist | unrestricted
allowed_remote_providers = []                   # empty unless mode = "allowlist"
require_consent_per_call = true                  # prompt on each remote call
require_consent_per_session = false              # or once per session
session_budget_usd = 2.00                        # hard cap; further calls fail
per_call_token_limit = 4096
offline_override = false                         # true = force local-only regardless

[ml.declared_endpoints]
# Every remote endpoint a shipped build may ever reach must appear here.
# The editor warns on first call; the game build refuses any endpoint not listed.
"anthropic.com" = { purpose = "dialogue", data_class = "npc-dialogue" }
"api.openai.com" = { purpose = "texture-gen", data_class = "asset-authoring" }
```

Enforcement:

- **Editor**: first remote call per session with `require_consent_per_call` fires a modal showing provider, endpoint, estimated cost, and data class. Denials are sticky per template.
- **Game build**: policy compiles into the shipped binary. A call to an endpoint not in `declared_endpoints` is a hard `MlError::UndeclaredEndpoint` — the game does not phone home to undeclared domains.
- **Session budget**: cumulative estimated cost over the cap fails calls with `MlError::BudgetExceeded`; editor surfaces a red banner, not a crash.
- **Offline override**: single title-bar toggle forces `local-only` regardless of project policy. For flights, NDA demos, untrusted networks.

Opinion: this is the feature that makes RustForge usable inside a studio with a legal department. Unreal asks studios to layer policy on top of half a dozen plugins; Phase 39 bakes it in.

## 6. Response caching

Iteration on prompts is the 90% use case during authoring. Re-running the same template with the same inputs three hundred times should hit a local cache, not the network.

```
  key = blake3(provider_id || model_id || rendered_prompt || params_serialized)
  value = (stream of tokens, finish_reason, usage)
  storage = project_dir/.rustforge/ml-cache/<first-2-hex>/<rest>.bin
```

- Cache is content-addressed; identical inputs across branches hit the same entry.
- Cache is opt-out per template (`cacheable = false` for anything that must be fresh every call, like random flavor text seeded by `time()`).
- `rustforge cache prune` drops entries older than N days; `rustforge cache stats` reports hit rate per template.
- Cached entries are replayed through the same streaming interface — the caller's UI animation still ticks token-by-token so the designer can see how the final scene paces, even when the tokens came from disk.

## 7. Sandboxing and out-of-process inference

Shared rule: the editor and the game loop never run an LLM inside their own process. An LLM can OOM, assert, deadlock on a bad GGUF, or hit a pathological prompt that runs for an hour. None of that should take down the editor or freeze a shipped game.

```
  editor / game process                 worker process
  ┌─────────────────────┐              ┌─────────────────────┐
  │ MlProviderRegistry  │              │ model / adapter     │
  │                     │ ◀──── pipe ─▶│ llama.cpp / curl /  │
  │ cache + policy      │              │ candle / ollama cli │
  └─────────────────────┘              └─────────────────────┘
          │ supervises, restarts on crash
          ▼
  exit code ──▶ backoff ──▶ respawn or disable
```

- Remote providers **always** run out-of-process. The boundary doubles as a network sandbox: the editor process does not open HTTPS sockets; the worker does.
- Local providers run out-of-process by default for stability, in-process only via a per-provider `allow_in_process = true` opt-in used by tiny embedding models where IPC overhead dominates.
- Worker spawns through the same supervisor the Phase 11 plugin system uses. A worker that crashes twice in 30 seconds is disabled for the session and a profiler warning fires.
- Diffusion GPU access inside the worker uses its own wgpu instance; no sharing with the editor's renderer. A crashed diffusion pipeline doesn't take the viewport with it.

## 8. LLM-driven NPC dialogue

The runtime side of LLMs plugs into Phase 26 through a new BT task and an ECS streaming component.

```rust
#[derive(BtTask)]
#[bt_task(name = "LlmReply", category = "Dialogue")]
pub struct LlmReply {
    pub template: AssetRef<PromptTemplate>,
    #[bb_key] pub target: BbKey<Entity>,
    #[bb_key] pub last_player_line: BbKey<String>,
    #[bb_key] pub output: BbKey<String>,        // final line written here
    pub stream_to: DialogueStreamTarget,        // UI bubble id for Phase 44
    pub max_latency_ms: u32,                    // fall back to canned line if exceeded
}
```

- Tokens stream into the dialogue UI via a typed channel; the Phase 44 widget subscribes by `DialogueStreamTarget`. Phase 39 lands the streaming plumbing and a placeholder widget; Phase 44 replaces it.
- `max_latency_ms` is the designer's escape hatch: if the first token misses the deadline, a pre-authored fallback line fires and the task returns `Success`. The game never waits on the network mid-combat.
- BT abort cancels cleanly — the worker receives a cancel token, stops generating, partial tokens are discarded.
- Tool use: when `MlCapabilities::TOOL_USE` is present, the task exposes a narrow set of engine tools (`lookup_npc`, `get_location_description`) through a fixed JSON schema. The bridge re-enters the engine on the main thread. Narrow by design — prompt-authored gameplay logic is out of scope (§15).

## 9. Diffusion-based asset generation

The editor grows a **Generate** menu with two first-class entries: **Generate Texture** and **Generate Material**. Both land as panels that look like importers because they are importers — the generated bytes flow through the Phase 5 pipeline so a DALL-E-generated albedo becomes a first-class `TextureAsset` with a GUID, a cache artifact, and a slot in the Content Browser.

```
  ┌──────────────────────────────────────┐
  │  Generate Texture                     │
  ├──────────────────────────────────────┤
  │ Prompt:  "weathered bronze, tiling"   │
  │ Size:    [1024 × 1024 ▼]              │
  │ Provider:[candle-sdxl (local) ▼]      │
  │ Seed:    [42]   Steps: [30]           │
  │ [  Estimate cost: free (local)    ]   │
  │                                       │
  │ [ ▶ Generate ]                        │
  └──────────────────────────────────────┘
                │
                ▼
   raw bytes → Phase 5 importer → TextureAsset
                                   + sidecar metadata
                                     { provider, model, prompt, seed, steps }
```

- Local backend: `candle-rs` for SDXL / SD3, running in the same out-of-process worker kind as §7.
- Remote backend: OpenAI Images, Midjourney (if a public API exists at ship time), generic `custom-http`. The policy gate from §5 applies identically.
- The generation sidecar is metadata only — it records provenance so a project can answer "where did this texture come from" without leaking prompts into the shipped build.
- **Generate Material** does one generation for albedo and a constrained PBR decomposition pass to estimate normal / roughness / metallic. The decomposition is cheap and deterministic; it is not itself an ML model in Phase 39 (that's a latter-phase bet).
- Iteration loop: seed + prompt cached via §6 means stepping through variants is instant for repeat seeds.

## 10. ML animation synthesis (experimental)

Text-to-motion diffusion models (MDM, MotionGPT family) can produce short novel clips from prompts. Phase 35's motion matching database consumes clips as input; Phase 39 adds a pipeline that feeds diffusion output into that database.

```rust
// crates/rustforge-ml-motion/src/lib.rs
pub struct MotionRequest {
    pub prompt: String,             // "a heavy two-handed sword overhead chop"
    pub duration_seconds: f32,      // 0.5 .. 4.0
    pub skeleton: AssetRef<Skeleton>,
    pub style_reference: Option<AssetRef<AnimationClip>>,
}
pub struct MotionResponse {
    pub clip: AnimationClip,
    pub provenance: MotionProvenance,   // model, prompt, seed
    pub confidence: f32,                // model-reported 0..1
}
```

Rules that keep this from shipping garbage: the feature is behind `project.ml.experimental_motion_gen = true` (default off); generated clips land as `provisional` in the asset browser with a yellow badge and only enter motion matching after explicit designer approval; output maps through Phase 35's retargeting so obviously broken clips fail review; scope is short action clips only — full locomotion loops, facial, and multi-character interaction are not Phase 39 problems.

Honest opinion: the 2026 state of motion diffusion is "good enough for prototyping, shaky for shipping". Phase 39 ships the plumbing; the quality inflection happens in models outside our control. Keeping this behind a trait means the 2028 version swaps in without touching engine code.

## 11. Neural rendering extensions

Extends Phase 36's `NeuralUpscaler` and denoiser traits. Three additions land in Phase 39; each is independently tier-gated.

```
                 Phase 36 baseline         Phase 39 additions
  Ultra          DLSS/FSR/XeSS upscale  ─▶ + DLSS Frame Generation
                 spatiotemporal denoise ─▶ + ReSTIR-DI/GI variance reduction
                 RT GI (two bounce)     ─▶ + Neural Radiance Caching (NRC)
  High           TAA-U                     unchanged
  Medium / Low   TAA-U / off               unchanged
```

- **DLSS Frame Generation** — interpolates synthetic frames between rendered ones. Behind a new `FrameInterpolator` trait so FSR 3 Frame Gen and (eventually) XeSS equivalents slot in. Tier-gated to Ultra; disabled in competitive projects via `rendering.neural.frame_gen = false`.
- **ReSTIR-DI / ReSTIR-GI** — spatial and temporal sample reuse for direct and indirect lighting. Compute pass in front of the Phase 36 denoiser stack; cuts sample count for equal quality. Not ML in the neural-network sense — it's the reservoir bridge technique, filed here because it sits next to the ML-denoise path.
- **Neural Radiance Caching (NRC)** — small MLP caches indirect irradiance per-scene, trained online. `rendering.neural.nrc = experimental`; default off. Quality wins on the Cornell-analog test scene; production stability is the open question.

All three reuse Phase 36's exported-texture interop; the wgpu-version pin extends to cover these vendor SDKs.

## 12. Editor ML panel

One panel, accessible from `Window → ML`. Reuses the Phase 3 dockable panel shell.

```
  ┌──────────────────────────────────────────────────────┐
  │ ML                                                    │
  ├──────────────────────────────────────────────────────┤
  │ Mode:  ( ) Offline  (●) Local+Remote   [policy…]     │
  │                                                       │
  │ Providers                                             │
  │  ● llama-cpp        local    llama-3.1-8b-q4   ok    │
  │  ● candle-sdxl      local    SDXL-base-1.0    ok    │
  │  ● anthropic        remote   claude-*          auth? │
  │  ○ openai           remote   gpt-4*            dis   │
  │                                                       │
  │ Session: 14 calls  9 hits  cost $0.021 / $2.00       │
  │ [ Clear cache ]  [ Test provider ]  [ View log ]     │
  └──────────────────────────────────────────────────────┘
```

- Offline is instant and sticky across editor restarts until cleared; forces `MlLocality::Remote` calls to fail closed.
- `Test provider` runs a canned prompt to validate auth without writing gameplay code; `View log` exposes the per-call forensic record.
- Panel is gated behind Phase 2's `editor` feature; shipped games see only the runtime `ml` module with no UI.

## 13. Build order within Phase 39

1. **`MlProvider` trait & registry (§1)** — empty, with a mock provider. Merge first; every other piece hangs off this.
2. **Out-of-process worker harness (§7)** — IPC, supervisor, mock provider in the worker. Proves crash isolation before real models land.
3. **Local llama.cpp adapter (§2)** — first real completion from a real model.
4. **Remote adapter — Anthropic (§3)** — first remote; exercises the gate and cache. OpenAI and `custom-http` follow as serializer swaps.
5. **Prompt template registry (§4)** — RON editor and snapshot tests; unlocks real dialogue authoring.
6. **Cost & privacy gate (§5)** — hard-fails remote calls until project policy allows them. Lands with the first remote provider.
7. **Response cache (§6)** — sits between gate and worker.
8. **LLM dialogue BT task (§8)** — streams into a placeholder UI; Phase 44 takes over later.
9. **Diffusion texture & material generators (§9)** — first asset-generation features; validate the importer hook.
10. **ML motion preview (§10)** — experimental flag; warning badge.
11. **Neural rendering: ReSTIR → DLSS Frame Gen → NRC (§11)** — in that order; ReSTIR is least risky, NRC is experimental.
12. **Editor ML panel (§12)** — consolidates the UX after the pieces above exist. Polish step.

## 14. Scope ❌ — what's NOT in Phase 39

- ❌ **Training models in-editor.** No gradient descent, no LoRA UI, no dataset curation. The engine consumes trained models; it does not produce them.
- ❌ **Fine-tuning UI.** Users fine-tune outside the editor and point a provider at the resulting checkpoint.
- ❌ **Hosted ML SaaS.** RustForge does not run a server that hosts models for users. Remote calls go to user-authorized third parties, not us.
- ❌ **AI-authored game logic.** No prompt-to-BT, no code generation from natural language, no runtime gameplay scripted by an LLM. Deterministic gameplay is Phase 26's job.
- ❌ **Text-to-3D mesh generation.** 2026 state of the art produces meshes that require heavy cleanup before use. Not a stable tool yet.
- ❌ **Voice synthesis and speech-to-text.** Valuable but a separate phase; the `MlProvider` trait hosts them later without engine changes.
- ❌ **Agentic loops with autonomous tool use.** Tool use (§8) is narrow and whitelisted; open-ended agent loops are a security and stability disaster.
- ❌ **Open-world "AI director" that mutates scenes.** Unreal's experiments point the way; we deliberately decline.
- ❌ **Custom model formats.** GGUF, safetensors, ONNX — the ones the backends consume. No RustForge-specific container.
- ❌ **In-house neural denoiser training.** Phase 36 already said no; Phase 39 repeats it.
- ❌ **NeRF / Gaussian splatting scene capture.** Interesting, adjacent, separate phase.
- ❌ **Prompt marketplace.** The plugin marketplace covers this if demand materializes.

## 15. Risks

- **Remote API drift.** OpenAI / Anthropic endpoints change shape on a timescale of months. Pin the JSON schemas per provider, version the adapter, and gate upgrades on the provider-test harness (§12 generalized to CI).
- **Cost overruns despite the gate.** Designers disable consent prompts after the fifth one of the day. Mitigation: session budget is a hard cap, not a warning — hitting it fails calls. Editor red-banners the cap.
- **Model licensing surprise.** Llama 3.x, Claude, and GPT-4 have distinct license constraints. Provider manifests surface the license string; CI rejects a ship build whose configured providers mix a non-commercial license with `build.profile = release`.
- **Latency spikes from remote providers.** Network jitter produces 5-second dialogue pauses. The `max_latency_ms` fallback (§8) is mandatory for any in-combat dialogue; omission surfaces a linter warning.
- **Sandbox escape via tool use.** An LLM coerced into calling a dangerous tool bypasses intent. Tool exposure is whitelist-only per template; the default tool set is empty.
- **Cache poisoning across branches.** Prompt-hash cache entries can hide an upstream model update. Mitigation: cache key includes the provider-reported `model_version`, not just the model name.
- **Motion diffusion artifacts.** The experimental gate is the primary mitigation. Document the failure modes — feet penetration, wrong scale, flipped chirality — and reject clips in review.
- **NRC training instability.** Online-trained neural caches oscillate in scenes with rapid lighting changes. Keep the Phase 36 classical path available; NRC is decoration, not a correctness replacement.
- **Frame Generation latency.** Interpolated frames add input lag. Disable by default in projects with `project.profile = "competitive"`; document the knob loudly.
- **Worker process count.** Three local providers plus a diffusion worker plus a motion worker is five extra processes. Document the RAM cost; let users cap concurrent workers.
- **Data class leakage in telemetry.** The ML call log records prompts. Excluded from the default telemetry manifest; override requires a loud opt-in.
- **Plugin-API churn.** The `MlProvider` trait will grow — audio, video, new modalities. Same 0.x semver policy as Phase 11; minor-version breaks are flagged in CHANGELOG and pre-announced.

## 16. Exit criteria

Phase 39 is done when all of these are true:

- [ ] `MlProvider` trait is published in `rustforge-ml-api`, with a mock provider, three local adapters (`llama-cpp`, `mlx`, `ollama`), and three remote adapters (`anthropic`, `openai`, `custom-http`). Integration tests exercise each against a canned prompt.
- [ ] Out-of-process worker supervises all six; a deliberately-crashed worker recovers inside 500 ms without affecting the editor's frame loop.
- [ ] Prompt template registry round-trips `.rpt` RON through an editor with zero key reordering; snapshot tests cover 10 authored templates.
- [ ] Cost & privacy policy compiles into shipped builds. A game build that tries to reach an undeclared endpoint fails with `MlError::UndeclaredEndpoint` and does not open the socket.
- [ ] Editor consent modal is sticky per template per session; session budget cap fails remote calls on overrun while local calls continue.
- [ ] Response cache hits on repeated iteration measured at ≥ 95% in the reference authoring workflow; cache invalidates on `model_version` change.
- [ ] LLM dialogue BT task streams tokens into a placeholder UI, honors `max_latency_ms` fallback on a simulated 10-second stall, and cancels cleanly on BT abort.
- [ ] Diffusion texture generator produces a 1024×1024 albedo from a local SDXL model; output appears in the Content Browser as a first-class `TextureAsset` with provenance sidecar.
- [ ] Diffusion material generator produces a complete PBR set from one prompt; round-trip into the material graph editor produces a lit preview.
- [ ] Experimental motion generator produces a rig-retargeted clip; clip participates in Phase 35 motion matching only after explicit designer approval.
- [ ] DLSS Frame Generation wired through the adapter layer on supported NVIDIA hardware; disables cleanly on unsupported devices; documented latency impact.
- [ ] ReSTIR-DI/GI lands as a compute pass in front of the Phase 36 denoiser stack; reduces required sample count by ≥ 30% for equal SSIM on the reference scene.
- [ ] NRC is behind `experimental` flag, default off; documented quality delta on the Cornell-analog test scene.
- [ ] Editor ML panel shows provider list, session cost, cache stats, offline toggle; Offline forces all remote calls to fail closed within one frame.
- [ ] Phase 2 `editor` feature gate excludes the ML panel from shipped game builds; runtime ML code works without the editor crate.
- [ ] Phase 26 classical AI runs unchanged on projects with `ml.policy.mode = "local-only"`; Phase 36 paths regress zero ΔE after the Phase 39 neural-rendering additions land.
- [ ] Performance budget: a BT tick that fires `LlmReply` against a local 8B Q4 model returns the first token in ≤ 400 ms on the reference CPU; remote providers document their own latency envelopes.
- [ ] Example project `examples/talking-npc/` ships an NPC with a policy-gated local + remote dialogue path, a generated texture set, and an experimental-gated generated animation clip — authored entirely through the editor with no gameplay Rust beyond one custom BT task.
