# Phase 17 — Audio Engine

Phases 2 through 16 built a competent editor, physics, rendering, scripting, networking, and AI. The most conspicuous hole left in RustForge's runtime is audio. Right now the engine can draw a 5000-polygon character, replicate him across the wire, and have his AI navigate a mesh — but he can't make a sound. After networking, the audio engine is the single biggest missing runtime subsystem, and shipping a 1.0 without it would be embarrassing. Phase 17 fixes that.

The shape of the subsystem mirrors what modern engines converge on: a mixer graph with sources feeding through effect chains into busses and a master, spatialization for 3D sources, a streaming/decoded split for memory, and a node-based "audio graph" asset for composable DSP (the MetaSounds-equivalent) that the editor authors the same way it will later author VFX (Phase 18) and materials (Phase 20). Build one reusable node-graph widget; use it three times.

## Goals

By end of Phase 17:

1. **`cpal`-backed audio device** opens on startup, outputs a stable stereo stream at the device's native sample rate, underrun-free under editor load.
2. **Mixer graph** — sources route through effect chains into typed busses (Music / SFX / Dialog / UI), busses into Master. Volume, mute, solo per bus, serialized in scene.
3. **`AudioSource` component** with 2D and 3D modes; 3D sources attenuate by distance curve, optional Doppler, optional HRTF.
4. **Streaming vs. decoded** — music streams from disk, short SFX decode once into memory. Shared decoder pool, no per-source thread.
5. **Cooked format** — `.wav` / `.ogg` / `.flac` source, Opus cooked, `AssetImporter` plugged into Phase 5.
6. **Audio Graph** (`.raudio`) — node DSP graph authored in a dedicated editor panel, instantiable as a source or an effect.
7. **Node-graph widget** — reusable `crates/rustforge-editor-ui-nodegraph`, consumed by Phase 17 (audio), Phase 18 (VFX), Phase 20 (materials).
8. **Scripting** — `audio::play(asset, params)` et al. bound into the WASM host from Phase 11.
9. **PIE integration** — pressing Stop (Phase 7) halts every active voice and resets the mixer to its pre-play state.
10. **Voice cap + priority culling, occlusion low-pass via raycast, visualizer panel** — the polish tier that separates a working audio system from a shippable one.

## 1. Crate layout

Audio is large enough to justify its own crate, gated for editor-only pieces as usual.

```
crates/
├── rustforge-audio/                    # core runtime, no editor deps
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── device.rs                   # cpal stream, callback, ring buffer
│       ├── mixer/
│       │   ├── mod.rs                  # MixerGraph, tick
│       │   ├── bus.rs                  # Bus, BusId
│       │   ├── voice.rs                # Voice, VoiceId, priority
│       │   └── master.rs
│       ├── source/
│       │   ├── mod.rs                  # AudioSource component
│       │   ├── decoded.rs              # in-memory PCM
│       │   └── streaming.rs            # disk-backed streamer
│       ├── spatial/
│       │   ├── mod.rs                  # attenuation curves
│       │   ├── panning.rs              # stereo pan law
│       │   ├── doppler.rs
│       │   └── hrtf.rs                 # #[cfg(feature = "phonon")]
│       ├── dsp/                        # primitive DSP blocks (gain, LPF, delay, reverb)
│       ├── graph/                      # Audio Graph asset runtime
│       │   ├── mod.rs                  # .raudio deserialize, instantiate
│       │   ├── node.rs                 # Node trait, NodeDesc
│       │   └── eval.rs                 # per-block graph evaluation
│       ├── decode/                     # wav/ogg/flac/opus decoders
│       └── script_api.rs               # audio::play bindings (Phase 11 host)
│
├── rustforge-editor-ui-nodegraph/      # NEW reusable widget
│   └── src/                            # pan/zoom canvas, pins, wires, inspector
│
└── rustforge-editor/src/
    ├── panels/
    │   ├── audio_mixer.rs              # bus strip view
    │   ├── audio_visualizer.rs         # waveform / spectrum (extends Phase 10)
    │   └── ...
    └── asset_editors/
        └── audio_graph/                # consumes ui-nodegraph widget
```

Shipped games link `rustforge-audio` directly. The editor panels and the audio-graph asset editor live in `rustforge-editor`.

## 2. The mixer graph

The core runtime abstraction. Everything else routes through this.

```
┌─ Voice ─┐  ┌─ Voice ─┐   ┌─ Voice ─┐   ┌─ Voice ─┐
│ SFX     │  │ SFX     │   │ Music   │   │ UI      │
│ Gunshot │  │ Footstep│   │ Track   │   │ Click   │
└────┬────┘  └────┬────┘   └────┬────┘   └────┬────┘
     │ effect     │ effect      │ effect       │
     ▼ chain      ▼ chain       ▼ chain        ▼
  ┌──────────────────────┐   ┌────────┐    ┌────┐
  │     Bus: SFX         │   │ Music  │    │ UI │
  │  (volume, LPF, rev)  │   │        │    │    │
  └──────────┬───────────┘   └───┬────┘    └──┬─┘
             │                   │            │
             └────────┬──────────┴────────────┘
                      ▼
               ┌──────────────┐     ┌─────────┐
               │    Master    │────▶│ Limiter │──▶ cpal out
               └──────────────┘     └─────────┘
```

```rust
pub struct MixerGraph {
    busses: SlotMap<BusId, Bus>,
    master: BusId,
    voices: SlotMap<VoiceId, Voice>,
    sample_rate: u32,
}

pub struct Bus {
    pub name: String,
    pub parent: Option<BusId>,
    pub volume_db: f32,
    pub muted: bool,
    pub soloed: bool,
    pub effects: Vec<BoxedEffect>,
}

pub struct Voice {
    pub source: VoiceSource,              // Decoded | Streaming | GraphInstance
    pub bus: BusId,
    pub spatial: Option<SpatialState>,
    pub gain: f32,
    pub priority: u8,
    pub playback_state: PlaybackState,    // Playing | Paused | Stopping | Stopped
}
```

Submix defaults on project creation: `Master`, `Music`, `SFX`, `Dialog`, `UI`. These are just the starter set — users can add more in the Audio Mixer panel, parented under Master or any other bus. The serialized mixer layout lives in `rustforge-project.toml` (Phase 4 §2), not per-scene, because your game's audio bus structure is a project-level concern.

### 2.1 Tick & block size

`cpal`'s callback fires on the audio thread and demands samples *now*. Mixing inside the callback is a recipe for underruns the first time someone attaches a debugger. Instead:

- **Audio thread** (cpal callback): pulls pre-rendered blocks out of a lock-free SPSC ring buffer, copies to the output, returns. Zero allocation.
- **Mixer thread**: wakes on a condvar, renders one 512-sample block (≈10 ms at 48 kHz), pushes into the ring. Runs ahead of playback by 2–4 blocks.
- **Main thread**: enqueues commands (spawn voice, set gain, stop) into an MPSC queue that the mixer drains at the top of each block.

```rust
pub enum AudioCmd {
    Play { voice: Voice, id: VoiceId },
    Stop { id: VoiceId, fade: Option<Duration> },
    SetGain { id: VoiceId, gain: f32, ramp: Duration },
    SetBusVolume { id: BusId, db: f32 },
    UpdateListener { pos: Vec3, rot: Quat, vel: Vec3 },
    ResetAll,                             // Phase 7 Stop hits this
}
```

This is the same triple-thread layout every serious audio engine ends up with. Skip it and you will rewrite it.

## 3. `AudioSource` component

```rust
#[derive(Reflect, Clone)]
pub struct AudioSource {
    pub asset: AssetRef<AudioAsset>,      // clip or .raudio graph
    pub bus: BusName,                     // resolved to BusId at play time
    pub autoplay: bool,
    pub looped: bool,
    pub gain_db: f32,
    pub pitch: f32,                       // 1.0 = normal
    pub spatial: SpatialSettings,         // see §4
    pub priority: u8,                     // 0 = cullable, 255 = never cull
    #[reflect(runtime_only)]
    pub voice: Option<VoiceId>,           // live playback handle
}
```

The `voice` field is tagged `runtime_only` so the Phase 4 serializer skips it — it's a runtime handle, not scene data. Scripts reach it via a `Query<&mut AudioSource>` in the WASM host.

## 4. Spatial audio

```rust
pub struct SpatialSettings {
    pub mode: SpatialMode,                // Source2D | Source3D
    pub attenuation: AttenuationCurve,    // Linear | Inverse | InverseSquare | Custom(Curve)
    pub min_distance: f32,                // full-gain radius
    pub max_distance: f32,                // silence beyond this
    pub doppler_factor: f32,              // 0 = off, 1 = physical
    pub spread_deg: f32,                  // 0 = point source, 180 = ambient
    pub hrtf: bool,                       // requires `phonon` feature
}
```

### 4.1 Attenuation

Stick with the three standard curves plus a user curve. Same maths as every other engine; no reinvention.

```
gain = clamp01((max - d) / (max - min))        // Linear
gain = min / max(d, min)                        // Inverse  (Unity default)
gain = (min / max(d, min))^2                    // InverseSquare (physically correct)
```

Sample the curve once per block, not per sample.

### 4.2 Doppler

Per frame, compute `rel_vel = (source.vel - listener.vel) · normalize(source.pos - listener.pos)` and shift playback pitch by `1 - rel_vel * doppler_factor / speed_of_sound`. Clamp to `[0.25, 4.0]` to keep the resampler stable. Disable entirely when `doppler_factor == 0`.

### 4.3 HRTF via `phonon`

Optional. Steam Audio (`phonon`) ships Rust bindings; wire it behind a feature flag:

```toml
[features]
default = []
hrtf = ["phonon"]
```

When the feature is off, 3D panning falls back to equal-power stereo panning. This is non-negotiable — a lot of users will ship without HRTF because of licensing, binary size, or platform reach, and the fallback must sound acceptable.

## 5. Streaming vs. decoded

```
                 ┌───────────────────────────────────┐
                 │ AudioAsset::cook() chooses format │
                 └──────────────┬────────────────────┘
                                │
       ┌────────────────────────┴────────────────────────┐
       │                                                 │
 size ≤ threshold                                 size > threshold
 (default 512 KB decoded)                      or [stream = true] override
       │                                                 │
       ▼                                                 ▼
┌──────────────┐                               ┌────────────────┐
│  Decoded     │                               │   Streaming    │
│  (full PCM   │                               │  (Opus blocks, │
│   in RAM)    │                               │   demand-paged)│
└──────────────┘                               └────────────────┘
```

A **decoder pool** (4–8 worker threads) handles streaming: each active streaming voice grabs a decoder when it needs the next block, returns it when done. Budget for ~4 simultaneous streams (music + ambience + two dialog lines); anything more probably means a design bug. Short SFX never touch the pool — they're already PCM by the time they play.

The threshold lives in importer settings; per-asset override via `.meta`:

```toml
# goblin_grunt.wav.meta
[import]
stream = false
quality = 0.6    # Opus quality (0–1), ignored if stream = false
```

## 6. Asset formats

| Source        | Cooked             | Notes                                        |
|---------------|--------------------|----------------------------------------------|
| `.wav`        | Raw PCM or Opus    | PCM for decoded path, Opus for streamed      |
| `.ogg`        | Opus (transcode)   | Don't ship a second Vorbis decoder           |
| `.flac`       | Opus (transcode)   | Lossless source, lossy ship                  |
| `.raudio`     | `.raudio` (binary) | Node graph, authored in editor               |

One runtime decoder (Opus) plus raw PCM passthrough. Avoid shipping Vorbis, MP3, AAC — every extra codec is a supply-chain liability, a fuzzing target, and binary bloat. Source-side we accept the popular lossless/lossy formats for artist convenience; everything normalizes to Opus at cook time.

## 7. Audio Graph (`.raudio`)

The MetaSounds-equivalent. A node graph that evaluates DSP in block-sized chunks, instantiable as either a source (produces audio) or an effect (consumes + produces audio on a bus).

### 7.1 Node model

```rust
pub trait Node: Send {
    fn inputs(&self) -> &[PinDesc];       // { name, kind: Audio | Control | Trigger }
    fn outputs(&self) -> &[PinDesc];
    fn process(&mut self, ctx: &mut NodeCtx, block: &mut Block);
}
```

Core node set for Phase 17 (keep it tight):

- **Sources**: `WavePlayer`, `Noise`, `Sine`, `Sampler`.
- **Math**: `Add`, `Multiply`, `Gain`, `Clamp`.
- **DSP**: `BiquadLP`, `BiquadHP`, `Delay`, `Reverb` (Schroeder or freeverb, nothing fancier).
- **Control**: `ADSR`, `LFO`, `Mix2`, `Mix4`, `XFade`.
- **IO**: `In`, `Out`, `Parameter` (external float input, e.g. `rpm` from a vehicle script).

Third-party plugins are out of scope — see §11.

### 7.2 Graph evaluation

Topologically sort once at instantiate time, cache the order, evaluate linearly per block. Feedback loops require a one-block unit delay on the feedback edge (and a `Delay` node makes that explicit rather than implicit). Re-sort only when the graph is edited, never at runtime.

### 7.3 Parameters

The graph exposes `#[parameter]` nodes at its boundary. Scripts and the inspector set parameters by name:

```rust
audio::set_param(voice, "rpm", 4200.0);
```

Parameters interpolate over one block to avoid zipper noise. This is the one lesson every audio engine learns the hard way; bake it in from day one.

## 8. Audio Graph editor panel

Asset editor registered via Phase 8's `AssetEditor` trait, opens on double-click of a `.raudio`. Layout:

```
┌─ brick_engine.raudio ─────────────────────────────────┐
│ [Save] [Preview ▶] [Stop ■]         │ Inspector       │
│ ─────────────────────────────────────┤ ─────────────── │
│                                      │ Node: BiquadLP │
│    [RPM]──▶[Mul]──▶[Sine]──▶[LP]─┐   │ Cutoff [---o-] │
│                                  │   │ Q      [-o---] │
│                     [Noise]──────┤   │ Type   [LP ▾]  │
│                                  ▼   │                │
│                                 [Out]│                │
└──────────────────────────────────────┴────────────────┘
```

All of the canvas, pin, wire, and drag logic lives in `rustforge-editor-ui-nodegraph`. The audio editor supplies a node registry and a preview harness; it does not reimplement graph-drawing primitives.

### 8.1 The shared widget's contract

```rust
pub trait GraphSchema {
    type NodeId: Copy + Eq + Hash;
    type PinId: Copy + Eq + Hash;
    type NodeData: Clone;
    fn nodes(&self) -> &SlotMap<Self::NodeId, Node<Self>>;
    fn can_connect(&self, from: Self::PinId, to: Self::PinId) -> bool;
    fn on_edit(&mut self, edit: GraphEdit<Self>);   // widget reports; app commits
}

pub fn show<S: GraphSchema>(ui: &mut egui::Ui, schema: &mut S);
```

This widget must be genuinely reusable. The Phase 18 VFX graph and Phase 20 material graph both implement `GraphSchema`; if the trait turns out to need escape hatches, add them *now* rather than forking the widget three times. The single biggest Phase 17 risk is letting the audio team build a node editor that the VFX and material teams then have to re-implement.

## 9. Scripting bindings (Phase 11)

Thin surface, matching the rest of the script API's style:

```rust
// exported to WASM
pub fn play(asset: AssetRef<AudioAsset>, params: PlayParams) -> VoiceId;
pub fn stop(voice: VoiceId, fade: Option<Duration>);
pub fn set_gain(voice: VoiceId, gain_db: f32, ramp: Duration);
pub fn set_param(voice: VoiceId, name: &str, value: f32);
pub fn is_playing(voice: VoiceId) -> bool;

// on an AudioSource component
source.play();
source.stop();
```

`VoiceId` is a generational index; scripts holding a stale `VoiceId` after the voice ended get `is_playing → false` rather than a panic. Same pattern as `Entity`.

## 10. PIE integration (Phase 7)

Phase 7 §12 already flagged this: "audio continues after Stop." Phase 17 is where we pay that tab.

On `PlayState::Playing → Edit`:

1. Send `AudioCmd::ResetAll` to the mixer. The mixer thread drains its voice slotmap, frees streaming decoders back to the pool, fades out any in-flight block over ~10 ms to avoid a pop.
2. Clear all script-visible `VoiceId`s (they're in the ECS `AudioSource.voice` fields, which get restored by the snapshot anyway).
3. Leave bus volumes / mixer topology alone — those are project-level, not scene-level, and the user probably tweaked them deliberately.

On `Playing → Paused`: pause all voices (stop advancing playback cursors) but keep them allocated. On `Paused → Playing`: resume. No sample-accurate pause — latency is one block.

## 11. Voice cap + priority culling

A fixed voice budget (default 64 simultaneous voices, configurable per-project) prevents a pathological script from summoning 10,000 explosions. When a new `Play` would exceed the cap:

```
1. Compute effective priority = priority * distance_attenuation * (1 - age_factor)
2. Find the lowest-priority currently-playing voice.
3. If new voice's priority ≥ lowest: steal its slot (with a 50 ms fade-out).
4. Else: drop the new voice, increment a debug counter.
```

A counter for "voices stolen" and "voices dropped" surfaces in the Phase 8 profiler and the visualizer panel. If either is nonzero in a shipped build, that's a design smell.

## 12. Occlusion via raycast low-pass

Cheap and convincing. For each active 3D voice, once every N frames (N = 4 or so, staggered so not all voices ray-test on the same frame):

```
cast ray from source.pos to listener.pos through physics world (Phase 9)
if hit:
    target_cutoff = lerp(occluded_hz, open_hz, 1 - occlusion_thickness)
else:
    target_cutoff = open_hz
smoothly ramp the voice's built-in LPF cutoff toward target_cutoff
```

`occluded_hz ≈ 600 Hz`, `open_hz ≈ 22 kHz`. Tune by ear. The ramp is mandatory — snapping cutoffs clicks audibly.

Skip occlusion for 2D sources, UI sounds, and anything on the `Music` bus. An `AudioSource.occlusion: OcclusionMode` field controls participation (`Auto | Off | Force`).

## 13. Audio visualizer panel

Extends Phase 10's (profiler / debug draw) panel family rather than being a standalone. Three views, picked via a tab at the top:

```
┌─ Audio Visualizer ─────────────────────────────────────┐
│ [Meters] [Scope] [Spectrum]                            │
│ ──────────────────────────────────────────────────────  │
│ Master  ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯  -6.2 dB  peak -1.1 dB  │
│ Music   ▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯▯▯▯▯▯  -12.4 dB                │
│ SFX     ▮▮▮▮▮▮▮▮▮▮▮▮▮▮▮▯▯▯▯▯▯  -8.0 dB                 │
│ Dialog  ▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯  silent                  │
│ UI      ▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯▯  silent                  │
│ ──────────────────────────────────────────────────────  │
│ Active voices: 12 / 64   Stolen: 0   Dropped: 0        │
└────────────────────────────────────────────────────────┘
```

Meters use ITU-R BS.1770 integrated loudness if you want to be serious, or plain peak + RMS for Phase 17. Start with peak + RMS, upgrade in a later phase.

## 14. Build order within Phase 17

1. **`cpal` device + silence output** — open stream, write zeros. Confirm no underruns for 60 seconds across Windows/macOS/Linux.
2. **Ring buffer + mixer thread** — pre-render blocks of silence, pipe through ring. Still silent, but the three-thread architecture is live.
3. **Decoded source playback** — one hardcoded WAV, one voice, no bus, straight to master. "It plays" moment.
4. **Mixer graph** — Busses, Master, volume/mute/solo. Route the single voice through `SFX → Master`.
5. **`AudioSource` component + ECS integration** — autoplay, looped, spawn voices from the mixer thread via command queue.
6. **Streaming path + decoder pool** — play a long Ogg file, verify memory is bounded.
7. **Cook pipeline** — `.wav/.ogg/.flac → Opus` importer plugged into Phase 5; `.meta` threshold.
8. **Spatial: attenuation + stereo pan** — 3D sources pan and fade. No Doppler or HRTF yet.
9. **Doppler** — add the pitch shift path; write a test with a source flying past the listener.
10. **HRTF** (feature = `hrtf`) — wire `phonon`, verify the fallback still works when disabled.
11. **Node-graph widget** — `rustforge-editor-ui-nodegraph` crate with a demo integer-math graph in its examples. Land this *standalone* before pointing the audio editor at it.
12. **Audio Graph runtime** — `.raudio` format, toposort, block eval, parameter ramps.
13. **Audio Graph editor panel** — consumes the widget, ships the core node set from §7.1.
14. **Script bindings** — `audio::play` family, `AudioSource` script methods.
15. **PIE Stop integration** — `AudioCmd::ResetAll`, verify no stuck voices after 100 play/stop cycles (Phase 7 exit criterion).
16. **Voice cap + priority culling** — fixed budget, stealing, debug counters.
17. **Occlusion low-pass** — raycast against physics world, smoothed cutoff.
18. **Audio visualizer panel** — meters first, scope and spectrum second.

## 15. Scope boundaries — what's NOT in Phase 17

- ❌ **Voice chat** — that's Phase 14 (networking); Phase 17 is playback only.
- ❌ **Text-to-speech** — useful, large, totally separable; not in a 1.0 cycle.
- ❌ **MIDI authoring, sequencing, piano roll** — RustForge ships games, not DAWs.
- ❌ **Orchestral tools** (adaptive music layers, vertical remixing, stingers) — interesting future phase, out of scope here. Users can approximate with the audio graph and parameters.
- ❌ **DAW-level editing** — clip trimming, non-destructive waveform editors, fade curves on the waveform. Users edit audio in Reaper/Audacity and reimport.
- ❌ **Third-party plugins** — no VST/AU/LV2 host. Security, stability, and cross-platform licensing costs exceed the benefit at this stage.
- ❌ **Ambisonics / higher-order surround beyond stereo + (optional) 5.1 passthrough**. 7.1 and Atmos object-audio are post-1.0.
- ❌ **Per-platform audio backends** beyond what `cpal` covers. Xbox GameCore, PS5 native, etc. come when their porting phases do.
- ❌ **Sample-accurate MIDI-style event scheduling** across voices. Block-accurate (10 ms) is fine for games.

## 16. Risks

- **The audio thread is unforgiving.** One allocation, one lock, one `println!` in the cpal callback and the user hears a click. Enforce `#![deny(unsafe_op_in_unsafe_fn)]` and a CI test that runs a stress scene and asserts zero underruns over 30 seconds. Audio bugs that only reproduce on the user's machine are the worst class of bug this engine can ship.
- **Node-graph widget scope creep.** Every time audio, VFX, or materials says "we need one more tiny feature", the widget's API grows. Pick the MVP (pan/zoom, add/remove nodes, connect/disconnect pins, right-click menu, inspector callback) and resist everything else until Phase 18 and Phase 20 have both used it for real work.
- **HRTF licensing.** `phonon` (Steam Audio) is permissive but comes with Valve's EULA around the HRTF dataset; confirm RustForge's distribution story before making it the default. Shipping the fallback-first design (§4.3) protects us here.
- **Opus transcoding quality.** Dialog at low bitrate sounds bad. Default quality setting (0.6) is a tradeoff; make sure the importer UI surfaces it prominently and let users override per-asset.
- **Voice-cap tuning.** 64 voices is a shot in the dark. Expose it per-project early and collect telemetry from dogfood projects during Phase 17 to inform the default.
- **Occlusion ray-cost on dense scenes.** 64 voices × 1 ray/4 frames = 16 rays/frame. Cheap. But if the voice cap grows, so does this. Cap occlusion rays independently (e.g. only the nearest N voices ray-test).
- **Snapshot/restore of mixer state.** Bus volumes during play — are they scene state or project state? Phase 17 says project; confirm this doesn't surprise users who expect their play-mode tweaks to the SFX bus slider to revert. Add a separate "Session override" layer if needed.
- **Script `VoiceId` leaks.** Scripts that call `audio::play` in a loop without storing or stopping the handle will hit the voice cap fast. The priority culler saves correctness, but diagnostics have to make this obvious. Log-once per script the first time it causes a drop.
- **`cpal` device-change events.** User unplugs headphones mid-play. `cpal` surfaces this; handle it — close the stream, reopen on the new default device, resume. Test by actually unplugging something.

## 17. Exit criteria

Phase 17 is done when all of these are true:

- [ ] Engine opens a `cpal` output stream on startup and can play a decoded WAV through a `Master` bus with zero underruns over a 60-second stress test.
- [ ] `AudioSource` component authors in the inspector, autoplays, loops, and serializes cleanly through Phase 4's scene I/O.
- [ ] Mixer graph has `Master`, `Music`, `SFX`, `Dialog`, `UI` busses by default; per-bus volume / mute / solo work and persist in `rustforge-project.toml`.
- [ ] 3D sources attenuate by at least Linear / Inverse / InverseSquare, pan across stereo, and respond to Doppler when enabled.
- [ ] HRTF path compiles and produces audible spatialization behind the `hrtf` feature; non-`hrtf` builds fall back cleanly to stereo panning.
- [ ] Streaming and decoded paths both work; streaming a 10-minute Opus file keeps audio-thread memory bounded within 1 MB of baseline.
- [ ] Cook pipeline converts `.wav`, `.ogg`, and `.flac` to Opus or raw PCM based on importer settings; reimport via Phase 5's watcher updates live.
- [ ] `.raudio` graph assets can be authored in the Audio Graph editor panel, saved, reloaded, and instantiated at runtime; core node set of §7.1 is implemented.
- [ ] `rustforge-editor-ui-nodegraph` is a standalone crate with its own example and is the *only* graph-rendering code path in the audio editor.
- [ ] WASM scripts can `audio::play`, `audio::stop`, `audio::set_gain`, and `audio::set_param` with parameter interpolation.
- [ ] Pressing Stop (Phase 7) from Playing silences every voice within one block, resets voice state, and leaves bus configuration untouched.
- [ ] Voice cap of 64 (configurable) is enforced; priority-based stealing works; "voices dropped" counter exposed in the visualizer.
- [ ] 3D voices low-pass when a raycast from listener to source hits Phase 9 geometry; cutoff ramps smoothly.
- [ ] Audio visualizer panel shows per-bus peak/RMS meters, live voice count, and stolen/dropped counters.
- [ ] `rustforge-core` still builds and runs without the `editor` feature; `rustforge-audio` builds without the `hrtf` feature.
- [ ] Determinism check: playing the same scene twice from the same script inputs produces byte-identical output on the same sample-rate device.
