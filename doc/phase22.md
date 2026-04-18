# Phase 22 — Mobile & Web Platform Expansion

RustForge 1.0 shipped three desktop triples: `windows-x86_64-msvc`, `linux-x86_64-gnu`, `macos-aarch64`. The choice was deliberate — mobile and web each bring a thicket of platform-specific lifecycle, input, memory, and API constraints that would have stalled the editor work for a year. Post-1.0, with the editor stable and the renderer proven, the cost is finally justified.

Phase 22 closes the platform gap. The bet is that wgpu and winit already did most of the heavy lifting: wgpu speaks WebGPU, WebGL2, Metal, and Vulkan from one surface API; winit has first-class iOS, Android, and web backends. What RustForge needs to add is the *stuff around* the renderer and window — filesystem shape, threading model, asset packaging, lifecycle, input extensions, build glue, per-platform cooked textures. That "stuff around" is substantial but bounded.

Scope is deliberately narrow: **games run on mobile and in browsers**. The editor stays desktop-only. Nobody is laying out a Content Browser on a phone in this phase. Console ports remain separate work (see Phase 18). This phase is about getting a game that builds for Windows today to also build for iOS, Android, and the browser with the same project file, the same scenes, and recognizably the same runtime semantics.

## Goals

By end of Phase 22:

1. **Platform abstraction seam.** A new `rustforge-platform` crate that wraps filesystem, windowing, threading, timers, IME, and lifecycle events behind one API. Every other crate depends on it, not on `std::fs` or raw winit, for any call that differs across targets.
2. **Three new target triples.** `aarch64-apple-ios`, `aarch64-linux-android`, `wasm32-unknown-unknown`. Each one cooks, links, packages, and boots a sample game.
3. **Browser runtime.** WebGPU where the browser exposes it; WebGL2 fallback with documented feature degradation. One build, one `.wasm`, runtime picks.
4. **Mobile runtime.** Android via `cargo-apk`, iOS via `cargo-lipo` + `cargo-bundle-ios`, with generated Xcode and Gradle project scaffolds.
5. **Touch and gesture input** extending Phase 16's input map — touch points, pinch, pan, pen pressure, tilt — as a first-class device type, not a mouse emulator.
6. **Per-platform cooked assets.** BC7 for desktop, ASTC for mobile, KTX2+WebP for web; cook step picks the right encoder per target.
7. **Lifecycle resilience.** Suspend/resume, browser `visibilitychange`, iOS "kill while backgrounded" all serialize and restore state.
8. **CLI build support.** `rustforge-cli build --target ios|android|wasm32-web` extends Phase 9 without forking the pipeline.
9. **60 fps on mid-range mobile and Chrome WebGPU** for the sample game.
10. **Documented store submission.** No automation. Written flows for App Store and Play Store, plus a one-page `itch.io` web upload note.

## 1. Platform abstraction: why it's its own crate

Up to Phase 21, RustForge leaned on `std::fs`, `std::thread`, `std::time::Instant`, and `winit` directly. Every crate that touched any of those is implicitly desktop-assuming. On wasm there is no `std::fs`; on iOS and Android the filesystem is sandboxed and assets live inside the app bundle or APK; on web `std::thread::spawn` fails without cross-origin isolation headers. The right moment to paper over those differences is *before* you discover them scattered across fifty files.

```
crates/rustforge-platform/src/
├── lib.rs               # re-exports, feature gating per target
├── fs.rs                # read_asset_bytes, read_save, write_save
├── window.rs            # WindowHandle, surface creation (thin winit wrap)
├── thread.rs            # spawn, JoinHandle, try_spawn (may fail on web)
├── time.rs              # Instant, now_ms, monotonic_since_launch
├── ime.rs               # soft keyboard show/hide, IME composition events
├── lifecycle.rs         # AppEvent { Suspend, Resume, LowMemory, VisibilityHidden, ... }
├── storage.rs           # key-value save (files / sandbox / IndexedDB)
└── net.rs               # fetch (HTTP GET only; sufficient for asset streaming)
```

The crate is target-gated internally. A caller writes:

```rust
use rustforge_platform::{fs, lifecycle, thread};

let bytes = fs::read_asset_bytes("scenes/main.scene.bin").await?;
let Ok(worker) = thread::try_spawn(move || heavy_work()) else {
    // Fall back to single-threaded path on web without COOP/COEP.
    heavy_work();
    return Ok(());
};
```

Every `await` is intentional. Even on desktop where `read_asset_bytes` could be blocking, the signature is async so the web implementation — which has no choice but to go through `fetch()` — fits without a parallel universe of APIs. Async-on-desktop costs nothing when the future is already ready.

The crate exports *exactly one* shape per concept. No conditional types leaking into caller code. `thread::JoinHandle<T>` on web is an opaque type that always resolves synchronously or errors — callers don't branch on target triple.

This crate gets built first. Nothing else in Phase 22 compiles until it exists.

## 2. Browser target — WebGPU first, WebGL2 fallback

The browser is the most demanding of the three new platforms because the capability floor is a moving target. Safari got WebGPU in 17.4; Firefox is partial; Chromium has it stable. Design assumes detect-and-fall-back at runtime, not build-time.

```
boot on web:
    wgpu::Instance::new()
        .request_adapter( backends: PRIMARY )         // WebGPU
            │
            ├── Some(adapter) ──▶ WebGPU path, Phase 21 tier High/Ultra eligible
            │
            └── None ──▶ request_adapter( backends: GL ) // WebGL2
                            │
                            ├── Some(adapter) ──▶ WebGL2 path, forced Low tier
                            │
                            └── None ──▶ fatal: "browser not supported"
```

WebGL2 is *not* just "slower WebGPU." It is missing compute, storage buffers, timestamp queries, and indirect draw. Phase 21's tier system already has a Low tier for constrained hardware; web WebGL2 slots into it with extra restrictions documented below.

### 2.1 WebGL2 degradation table

| Feature                  | WebGPU (web)    | WebGL2 (web)      | Where it matters                          |
|--------------------------|-----------------|-------------------|-------------------------------------------|
| Compute shaders          | yes             | **no**            | VFX sim moves to CPU path (Phase 21 tier Low) |
| Storage buffers          | yes             | no (UBO only)     | Skinning uses attribute stream fallback   |
| Timestamp queries        | yes (limited)   | **no**            | CPU-side profiler only (Phase 21 CPU path)|
| Indirect draw            | yes             | no                | Culling issues draws on CPU               |
| 3D textures              | yes             | limited           | Volumetric effects disabled               |
| MSAA                     | yes, up to 4x   | yes, 4x           | Matches                                   |
| Texture compression      | ASTC/BC         | WebP + ETC2       | Cook step produces both                   |
| Max texture size         | 8192            | 4096 (conservative)| UI assets clamp                           |

The table is **authoritative**: the engine queries capabilities on boot and refuses to enable features not in the table for that backend. No "it worked on my machine" silent degradation.

### 2.2 Threading on the web

`std::thread::spawn` requires SharedArrayBuffer, which requires the page to be served with `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp`. Many hosts don't serve those headers by default.

Two paths are supported:

**Path A — Single-threaded** (default). All systems run on the main thread. Async IO via browser `fetch()`. Good enough for small games. The Phase 6 command stack is `Send + !Sync` on this target; the type system prevents accidental cross-thread sharing that wouldn't compile anyway.

**Path B — Multi-threaded, requires COOP/COEP.** Documented as opt-in. `rustforge-cli build --target wasm32-web --threads` emits a build that requires the two headers; the CLI prints the required `nginx` / `caddy` / `.htaccess` snippets at the end of the build. If the site doesn't send the headers, the game boots into single-threaded mode with a one-line console warning, not a crash.

Practical consequence: the command stack on web is always engineered to work `Send + !Sync`. Phase 6 already decided for `Send + Sync`; Phase 22 relaxes the requirement to `Send` only on `target_arch = "wasm32"`, behind a cfg. No other crate has to notice.

## 3. Android target

Android runs a proper NDK native binary, so the engine code is "normal" Rust. The frictions are the build system, the activity lifecycle, and the asset manager.

Build is driven by `cargo-apk`, with a fallback Gradle integration for studios that already have a Gradle project:

```
android build pipeline (cargo-apk path):
    cook --target android → ASTC textures, baked scenes
        │
        ▼
    cargo ndk -t arm64-v8a build --release
        │   produces libgame.so
        ▼
    cargo apk build
        │   packages libgame.so + assets/ into .apk
        ▼
    out/android/game-arm64-v8a.apk
```

Minimum API level is **24 (Android 7.0)**. That covers 99%+ of devices in 2026, drops the need for legacy workarounds around `AAssetManager`, and is what wgpu's Vulkan backend expects anyway. Lower is technically possible, not supported.

`arm64-v8a` only. `armeabi-v7a` is dead weight on any device Google would let into the Play Store by the time this ships.

Asset loading on Android goes through `AAssetManager` via JNI — the APK is a zip, not a filesystem. `rustforge-platform::fs::read_asset_bytes` hides this; the only caller-visible consequence is that asset paths are relative to the assets root and must be forward-slashed.

Gradle path: generated `build.gradle` with `externalNativeBuild` pointing at a thin CMake file that builds the `.so` via a `cargo build` call. Studios wiring RustForge into a bigger app use this; small teams use `cargo-apk`. Both paths ship in the same Phase 22 release.

## 4. iOS target

iOS is the hardest of the three because Apple requires the submitted artifact to be a full Xcode build product (a signed `.ipa`), not a Cargo-produced binary. The build therefore ends at "you have an Xcode project, open it and archive."

```
ios build pipeline:
    cook --target ios → ASTC + ETC2, baked scenes
        │
        ▼
    cargo lipo --release --targets aarch64-apple-ios
        │   produces libgame.a (universal for device + sim if asked)
        ▼
    cargo bundle-ios (generates Xcode project on first run)
        │   links libgame.a into a Swift main-shim target
        │   bundles assets/ into the app's Resources
        ▼
    out/ios/RustForgeGame.xcodeproj
        │   (developer opens in Xcode, signs, archives, submits)
        ▼
    [manual] .ipa → App Store Connect
```

Signing is explicitly **documented, not automated**. Apple's signing flow involves provisioning profiles, team IDs, keychains, and notarization — all of which change yearly and which developers already have strong opinions about. Phase 22 writes the README; it does not try to own the signing config.

The generated Xcode project includes a tiny Swift entry point that calls `rustforge_main()` exported from the static lib, wires up `UIApplicationDelegate`, and routes lifecycle callbacks (`applicationDidEnterBackground`, etc.) into `rustforge_platform::lifecycle` events.

iOS-specific: when the OS kills a backgrounded app to reclaim memory, resumption starts the process cold. The engine must therefore **serialize enough state on `Suspend` to reconstruct the session** — scene GUID, player position, current level, save slot — and restore on next launch if a "was backgrounded, now cold-booted" state file exists. Exit criterion: resume after 30 minutes of backgrounding lands the player in the same spot (not necessarily identical frame, but same scene, same save state).

## 5. Touch, gestures, and pen — extending Phase 16

Phase 16 is being written in parallel (the input-map, action-remapping phase). Phase 22 contributes a device taxonomy extension:

```rust
// in rustforge-input (Phase 16)
pub enum Device {
    Keyboard,
    Mouse,
    Gamepad(GamepadId),
    Touch(TouchId),    // added in Phase 22
    Pen(PenId),        // added in Phase 22
}

pub enum RawEvent {
    // existing variants ...
    TouchBegin { id: TouchId, pos: Vec2, pressure: f32 },
    TouchMove  { id: TouchId, pos: Vec2, pressure: f32 },
    TouchEnd   { id: TouchId },
    PenMove    { id: PenId, pos: Vec2, pressure: f32, tilt: Vec2 },
    // gesture recognition produces HIGHER-LEVEL events:
    Pinch   { center: Vec2, scale: f32, velocity: f32 },
    Pan     { translation: Vec2, velocity: Vec2 },
    TapN    { pos: Vec2, count: u32 },
}
```

Touch is *not* folded into mouse. A game that wants "tap to shoot" maps the `Touch` device in its action config; a game that wants true multi-touch (two-finger camera drag while one finger aims) gets raw touch IDs. Emulating a mouse from touch is a per-game choice, not an engine default — the failure mode of auto-emulation is that multi-touch games silently lose points.

Gesture recognition lives in `rustforge-platform::input` as a separate layer, consuming raw touch events and emitting `Pinch`/`Pan`/`TapN`. Games opt in per-action.

Pen pressure and tilt flow through even on desktop (Wacom on Windows/macOS). The abstraction was cheap to include; not doing so would force re-plumbing later.

## 6. Asset loading per platform

```
desktop:        std::fs::File + mmap where supported
Android:        AAssetManager (JNI) — no mmap, streamed
iOS:            Bundle.main.resourcePath + std::fs — mmap works
browser:        fetch() async + in-memory cache — no filesystem
```

`rustforge-platform::fs::read_asset_bytes` returns `impl Future<Output = io::Result<Bytes>>`. On desktop and iOS the future is ready immediately. On Android the JNI hop is fast but allocates. On web the future genuinely suspends. All four cases compile into the same caller code.

Streaming assets (big textures, level chunks) get a separate API that returns an async reader, not a whole `Bytes`, because holding a 200 MB mesh in a browser tab is a quick way to run out of address space.

## 7. Memory budgets

| Platform         | Target budget | Hard ceiling | Notes                                    |
|------------------|---------------|--------------|------------------------------------------|
| Desktop          | none          | OS           | same as pre-Phase-22                     |
| Mid-range Android| 1.0 GB        | 1.5 GB       | "mid-range" = 4 GB total device RAM      |
| High-end Android | 2.0 GB        | 3.0 GB       |                                          |
| iPhone 13+       | 1.5 GB        | 2.0 GB       | iOS kills over 2 GB on 4 GB devices      |
| Browser tab      | 500 MB        | 1.0 GB       | 32-bit wasm addr space is 4 GB max, fragmented |

The engine tracks allocator stats and fires a `LowMemory` event when the budget is exceeded so games can drop streaming chunks. Going over the hard ceiling is a bug; shipping with asserts on and a telemetry hit is the intended posture for development.

## 8. Save data

```
desktop:  ~/.local/share/<game>/saves/*.sav      (platform-standard paths)
Android:  getFilesDir()/saves/*.sav              (internal app sandbox)
iOS:     NSDocumentDirectory/saves/*.sav         (with NSFileProtection)
browser: IndexedDB, one DB per game, one object store "saves"
```

`rustforge-platform::storage` is a key-value API (`get(key) -> Option<Bytes>`, `put(key, bytes)`, `delete(key)`), not a filesystem. The file-based backends implement it by writing one file per key under a saves directory. IndexedDB maps directly. This makes porting a save system from desktop to web a non-event.

Autosave frequency on mobile should be generous — the OS can kill the app with no warning. Every checkpoint is a full save.

## 9. Lifecycle events

```rust
pub enum AppEvent {
    Suspend,            // about to go background; save NOW
    Resume,             // coming back from background
    VisibilityHidden,   // browser tab hidden; pause render, keep sim or not
    VisibilityShown,
    LowMemory,          // mobile OS is asking nicely before killing
    WillTerminate,      // iOS sometimes gives you this; often doesn't
}
```

On every `Suspend`, the engine:

1. Calls user `on_suspend(&mut Game)` hook.
2. Runs autosave to `rustforge_platform::storage`.
3. Releases GPU resources that can be recreated (transient surfaces, framebuffers).
4. Drops background audio streams (iOS requires this or it'll be killed faster).

On every `Resume`, the reverse. If the process was cold-booted with a "was suspended" flag, the engine loads the autosave and calls `on_resume_from_cold(&mut Game)` instead of the normal `on_start`.

Browser `visibilitychange` maps to `VisibilityHidden`/`VisibilityShown`, *not* `Suspend`/`Resume`. Rationale: a browser tab being hidden is not the same as an iOS app being suspended; the game process is still alive, heap is intact, we just shouldn't be rendering.

## 10. CLI — `rustforge-cli build`

Phase 9 defined `build --target <triple> --config <cfg>`. Phase 22 adds three triples without changing the shape:

```
rustforge-cli build --target ios          --config shipping
rustforge-cli build --target android      --config shipping
rustforge-cli build --target wasm32-web   --config shipping [--threads]
```

Internally each target selects a cook profile (BC7 vs ASTC vs KTX2+WebP), a link step (cargo / cargo-ndk / cargo-lipo / wasm-bindgen), and a package step (pak+stage / apk / xcodeproj / html+wasm). The pipeline shape from Phase 9 is preserved; only the per-step backends change.

Determinism (Phase 9 goal 8) holds across platforms: same inputs → byte-identical cook output for that target. Cross-target reproducibility is not required (different encoders produce different bytes); within-target reproducibility is.

## 11. Per-platform cooked textures

```
source.png ──▶ cook step ──▶ one or more cooked variants
                               ├── BC7 (desktop)
                               ├── ASTC 6x6 (iOS + Android high-end)
                               ├── ASTC 8x8 (Android mid-range; opt-in)
                               ├── KTX2 + BasisU (web WebGPU path)
                               └── WebP (web WebGL2 fallback)
```

The cook step for a given target emits only the variants that target needs. A desktop build has BC7 only, not ASTC. A web build has KTX2 and WebP only. The GUID is shared across variants; the runtime picks by platform at asset-load time.

Cooking two mobile texture formats (ASTC 6x6 and 8x8) is opt-in because it doubles mobile pak size. Default is ASTC 6x6 only.

## 12. Editor remote preview (stretch)

Stretch because it's a neat product differentiator but not required for shipping. The editor opens a TCP/WebSocket server; a companion app on iOS or Android connects and receives compressed frames plus input is streamed back. Developer iterates on a laptop, the phone mirrors the game at 30 fps over Wi-Fi.

Built on the existing PIE loop: the PIE frame output gets a fork that encodes to VP9 (or H.264 if VP9 is too slow) and ships to the client. Touch events come back as input.

Marked stretch because: codec choice, NAT traversal, and a mobile companion app each have their own tails. If two of the three main platforms ship cleanly, stretch can slip to Phase 23.

## Build order

Strict, because each step depends on the prior:

1. **`rustforge-platform` crate** — fs, thread, time, lifecycle, storage. Desktop impls first (they match current std usage), then stubs for other targets. Nothing else starts until this compiles.
2. **Browser / WebGPU path.** Easiest of the new targets: wgpu's WebGPU backend is mature, the sample game boots to a blue triangle, then to the full renderer. Validates `rustforge-platform` under the strictest target.
3. **Browser / WebGL2 fallback.** Exercises the tier-downgrade path; forces the capability-detection logic that Android will reuse.
4. **Android.** Vulkan via wgpu, APK packaging via `cargo-apk`, lifecycle hooks. Reuses all of the above.
5. **iOS.** Metal via wgpu, Xcode project generation, signing docs. Last because the Apple tax is highest and it benefits from lessons in 2–4.
6. **App store submission documentation.** Written after 4 and 5 are working, while the submission pain is fresh.

Each step ships a working sample before the next starts. "Working" means: the sample game boots, renders, takes input, saves, resumes after suspend.

## Scope ❌

- **Editor-on-mobile.** Not happening in Phase 22. RustForge's editor is a desktop tool.
- **Console ports through the mobile toolchain.** Console work is Phase 18. A console is not a phone.
- **Automated store submission.** Apple and Google both require manual signing / review flows that change yearly. Documented, not scripted.
- **In-App Purchase SDKs.** StoreKit, Google Play Billing — neither is integrated. A game that needs IAP integrates the SDK itself and exposes it as a plugin.
- **Achievements / leaderboards SDKs.** Game Center, Google Play Games Services — same reasoning as IAP. Plugin territory.
- **Push notifications.** Not engine business.
- **Ad SDKs.** Emphatically not engine business.
- **PWA installability, service workers.** Web target ships a plain `.html` + `.wasm`. Packaging as a PWA is downstream.
- **Safari < 17.4.** WebGL2 fallback covers older Safari, but new WebGPU features are simply off.
- **32-bit Android (`armeabi-v7a`).** Play Store no longer accepts 32-bit-only uploads anyway.

## Risks

- **wgpu bugs on specific mobile drivers.** Android's GPU driver landscape is famously inconsistent. Mitigation: test matrix covers at least three phones (Pixel, Samsung S-class, one budget MediaTek device). Known-bad drivers get documented; users get a clear fallback message.
- **Safari's WebGPU state.** Mitigation: WebGL2 fallback is not optional; every feature the engine uses on WebGPU is either marked as Tier-High-only (gracefully disabled on Low) or has a WebGL2 equivalent.
- **iOS background kill timing.** Apple gives you "about 30 seconds" on suspend. If autosave exceeds that, state is lost. Mitigation: autosave must be incremental and fast; Phase 22 tests with a 500 MB world to ensure autosave completes in under 5 seconds on iPhone 12-class hardware.
- **Wasm binary size.** Release wasm for a non-trivial game easily exceeds 20 MB. Mitigation: `wasm-opt -Oz`, `wasm-split` for level-based streaming, and shipped shipping-profile builds with LTO fat. Documented target: < 15 MB gzipped initial download.
- **Threading rug-pull.** If a hosting provider stops serving COOP/COEP, a multi-threaded web build silently degrades. Mitigation: boot-time detection, clean single-threaded fallback, console warning. No crash.
- **Touch vs. mouse action-map confusion.** Games that don't explicitly bind touch see "no input." Mitigation: the default `new-project` template binds both in its action map; docs show the pattern.
- **Gradle integration drift.** Android Studio changes Gradle-plugin versions often. Mitigation: the generated `build.gradle` pins versions and `cargo-apk` is the supported default; Gradle is for studios that already have their own build system and can patch.

## Exit criteria

1. Sample game builds and runs on all three new targets via `rustforge-cli build`.
2. **60 fps on a mid-range Android device** (reference: Pixel 6a / Snapdragon 778G / 6 GB RAM) at 1080p-equivalent, Phase 21 tier Medium.
3. **60 fps on Chrome WebGPU** at 1080p in a desktop browser, Phase 21 tier High.
4. **30 fps minimum on Chrome WebGL2 fallback**, Phase 21 tier Low, same sample game.
5. **iOS lifecycle resume** — background the app for 30 minutes, force the OS to reclaim (open 20 heavy apps), cold-boot the game, land in the same scene with the same save state.
6. **Android lifecycle resume** — swipe away, relaunch, land in the same scene. Suspend-resume within the same process likewise.
7. **Browser `visibilitychange`** — hide the tab, 5 minutes later show it, simulation resumes cleanly, no physics explosion.
8. Wasm initial download for the sample game is **under 15 MB gzipped**.
9. Android APK for the sample game is **under 40 MB** uncompressed.
10. iOS `.xcodeproj` opens in Xcode 16+ and archives without manual edits other than signing team selection.
11. The per-platform cooked texture variants exist for every texture in the sample game, and switching targets changes pak contents accordingly (verified by a byte-diff test).
12. Memory budget is enforced: exceeding the mid-range Android ceiling during a 30-minute play session is a test failure.
13. The new `rustforge-platform` crate is the sole user of `std::fs`, `std::thread`, and `winit` across the workspace. Verified by a grep-based CI check.
14. Store submission docs cover: signing (iOS), keystore (Android), `.ipa`/`.aab` generation, screenshot sizes, privacy manifest. One working "hello world" app has been submitted to TestFlight and Play Console internal testing by the team, notes captured.
