# Phase 33 — Replay & Session Recording

QA reproduces a crash three patches later. Marketing needs a clean 4K render of last night's boss fight. An esports team wants to scrub through a tournament final from each player's POV. A speedrunner posts a world record and the community wants to verify it without trusting the video. All of these want the same thing: a **replay system** — a file that, when fed back into the engine, reconstructs the session exactly.

RustForge already has the two halves: Phase 10 gave us a short-window physics rewind ring for gameplay uses (parry windows, lag compensation, undo), and Phase 16 gave us deterministic input record/replay for CI tests. Phase 33 stitches them — plus networked snapshots (Phase 14), RNG seeds (Phase 25), and script events — into a full session recorder with a proper `.rreplay` file format, editor scrubber, and export-to-video sidecar.

The big idea: **we never record pixels**. A replay is a re-simulation. Files stay tiny (MB, not GB), playback resolution is arbitrary, and the viewer can change camera angles the original player never took. The cost is strict determinism — Phase 25's Rapier determinism, fixed timestep, and floating-point discipline are load-bearing. Unreal's Replay System is the reference; we diverge in exposing the raw input stream so replays double as regression tests, and in making export-to-video a first-class workflow.

## Goals

1. Define `.rreplay` binary format: header, keyframe snapshots, delta snapshots, input stream, events.
2. Ship recording in two modes: editor PIE ("record this play session") and shipped-game runtime API.
3. Ship playback with timeline scrubber, speed (0.25x-4x), seek, and multi-POV switching.
4. Integrate with Phase 10 rewind ring — short-window rewind built on top of replay infra, not separately.
5. Support script-emitted events as timeline markers.
6. Export-to-video pipeline via Phase 9 headless + ffmpeg sidecar, with optional high-quality pass.
7. Streamer mode: live replay with ~1s buffer so remote viewers watch in near-real-time.
8. Version-tag every replay; refuse mismatched playback unless user opts into loose mode.

---

## 1. Recording Model

The replay is a **deterministic re-simulation script**. We capture the minimum state needed to reconstruct the session; derived data is regenerated.

```
+-- Captured (in .rreplay) ----+  +-- NOT captured (regenerated) --+
| RNG seeds                    |  | Pixel frames, audio samples    |
| Input stream per player/tick |  | Particle/animation pose        |
| Network snapshots (kf+delta) |  | UI state                       |
| Script events                |  | Physics between snapshots      |
| Engine+game version+hash,cam |  +--------------------------------+
+------------------------------+
```

The snapshot stream makes spectating another player work. Input alone is insufficient in multiplayer — you need remote players' authoritative state. In single-player, inputs + seeds + fixed timestep reconstruct everything; snapshot stream shrinks to a handful of bytes per second.

```rust
pub struct ReplayRecorder {
    header: ReplayHeader,
    keyframe_interval: Duration,    // default 30s
    delta_interval: Duration,       // default 1s
    input_sink: InputStreamWriter,  // Phase 16
    snap_sink: SnapshotWriter,      // Phase 14
    event_sink: EventWriter,
    writer: ZstdStreamWriter<File>,
    last_keyframe_at: Tick,
}

impl ReplayRecorder {
    pub fn on_tick(&mut self, tick: Tick, world: &World, inputs: &InputFrame) {
        self.input_sink.append(tick, inputs);
        if tick - self.last_keyframe_at >= self.keyframe_ticks() {
            self.snap_sink.write_keyframe(tick, world);
            self.last_keyframe_at = tick;
        } else if tick % self.delta_ticks() == 0 {
            self.snap_sink.write_delta(tick, world);
        }
    }

    pub fn emit_event(&mut self, name: &str, payload: &[u8]) {
        self.event_sink.push(ReplayEvent { tick: self.current_tick(), name: name.into(), payload: payload.into() });
    }
}
```

Cost when disabled: one atomic load behind `Recording::is_recording()`.

---

## 2. Keyframes & Deltas

Seek performance is the whole reason for keyframes. Without them, seeking to minute 42 means re-simulating 42 minutes. With a keyframe every 30s, worst case is ~30s of re-simulation; typical is a second or two.

```
Time   0s     30s    60s    90s    120s   150s
       |      |      |      |      |      |
Keys   K------K------K------K------K------K
Delta  .ddddd.ddddd.ddddd.ddddd.ddddd.ddddd
Input  iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii
Events   e        e  e              e
```

**Keyframe** = full snapshot (~50KB small match, ~2MB for 32-player server). **Delta** = diff against prior snapshot, ~1–20KB. **Input frames** record every tick (~32 bytes per player per tick).

| Setting             | Default | Notes                               |
| ------------------- | ------- | ----------------------------------- |
| `keyframe_interval` | 30s     | Seek granularity trade-off          |
| `delta_interval`    | 1s      | Finer = faster scrub, bigger file   |
| `zstd_level`        | 3 / 19  | Live / archive repack               |
| `max_file_mb`       | 500     | Auto-rotate if live recording grows |

We deliberately do **not** reconstruct between-tick state. If the renderer interpolates at 144Hz but sim ticks at 60Hz, playback re-runs 60Hz and lets the renderer interpolate normally. Interpolating snapshots lies.

---

## 3. File Format — `.rreplay`

Binary, little-endian, zstd-framed, seekable. Every chunk has a length prefix so a corrupt tail is recoverable up to the last good chunk.

```
+--- RREPLAY FILE LAYOUT -----------------------------------+
| MAGIC "RRPL"      4 B                                     |
| VERSION           u16                                     |
| ENGINE_VERSION    u32 (semver packed)                     |
| GAME_ID           [u8; 32] (content hash of game manifest)|
| GAME_VERSION      u32                                     |
| TICK_RATE_HZ      u16                                     |
| FLAGS             u32 (single_player, networked, pie, ...)|
| PLAYER_COUNT / PLAYER_TABLE                               |
| INDEX_OFFSET      u64  -> seek table at file end          |
+-----------------------------------------------------------+
| CHUNK STREAM (zstd frames, <= 256KB uncompressed)         |
|  [KEYFRAME   tick=0      payload]                         |
|  [INPUT_RUN  tick=0..60  player=0   payload]              |
|  [INPUT_RUN  tick=0..60  player=1   payload]              |
|  [EVENT      tick=12     name=...]                        |
|  [DELTA      tick=60     against=0  payload]              |
|  ...                                                      |
+-----------------------------------------------------------+
| SEEK INDEX (at INDEX_OFFSET)                              |
|  keyframes: (tick, offset, wall_time)                     |
|  deltas:    (tick, offset)                                |
|  events:    (tick, name_hash, offset)                     |
+-----------------------------------------------------------+
| FOOTER: INDEX_OFFSET repeat, crc64                        |
+-----------------------------------------------------------+
```

Seek index lives at the end so recording doesn't need final size upfront. If the file is truncated, playback rebuilds a partial index by scanning chunks.

```rust
pub enum Chunk {
    Keyframe { tick: Tick, payload: Vec<u8> },
    Delta    { tick: Tick, against: Tick, payload: Vec<u8> },
    InputRun { player: PlayerId, tick_start: Tick, tick_end: Tick, payload: Vec<u8> },
    Event    { tick: Tick, name: FixedStr<32>, payload: Vec<u8> },
}
```

---

## 4. Playback VM

Playback is a small state machine driving the same simulation code path as live play, over a fresh `World` plus a cursor into the replay file.

```rust
pub struct ReplayPlayer {
    file: ReplayFile,
    world: World,
    cursor: Tick,
    target_speed: f32,     // 0.25..=4.0
    paused: bool,
    follow: Follow,
}

pub enum Follow { Player(PlayerId), FreeCam(CameraState), ThirdPersonChase(PlayerId), Tactical(BoundingBox) }

impl ReplayPlayer {
    pub fn tick(&mut self, dt: Duration) {
        if self.paused { return; }
        let target_tick = self.cursor + dt.mul_f32(self.target_speed).as_ticks(self.file.tick_rate);
        while self.cursor < target_tick {
            let inputs = self.file.inputs_at(self.cursor);
            self.world.step(inputs, SimMode::Replay);
            if let Some(snap) = self.file.snapshot_at(self.cursor) {
                self.world.apply_correction(snap); // drift safeguard
            }
            self.cursor += 1;
        }
    }

    pub fn seek(&mut self, target: Tick) {
        let kf = self.file.nearest_keyframe_before(target);
        self.world.restore_keyframe(kf);
        self.cursor = kf.tick;
        while self.cursor < target {
            let inputs = self.file.inputs_at(self.cursor);
            self.world.step(inputs, SimMode::FastReplay); // skip rendering/SFX
            self.cursor += 1;
        }
    }
}
```

Two knobs: **`SimMode::Replay`** suppresses one-shot side effects that don't round-trip (analytics, cloud saves, autosave). Still emits visuals and audio. **`SimMode::FastReplay`** skips rendering, audio, particles for seek — runs at hundreds of x realtime. The `apply_correction` step snaps the world to recorded truth every delta boundary — our insurance against drift.

---

## 5. Playback Controls

```
 <<  [||]  >   >|       [=========O==========]   0.5x
 rew pause play step     timeline (scrub)       speed
```

Speeds 0.25x/0.5x/1x/2x/4x scale `dt`. Audio time-stretches via phase vocoder (rodio) or mutes at >=2x. Rewind is repeated short seeks (sim does not run backwards); granularity matches `delta_interval`. Keyboard matches video editors: `Space` pause, `J/K/L` rewind-pause-play, `,`/`.` step, `Shift+,`/`Shift+.` for 10s jumps.

---

## 6. Multi-Perspective Playback

The recorder captured every player's input stream and the authoritative snapshot stream. POV switch is instant, no seek — sim state is shared, only camera and HUD filter change. Free-cam disables gameplay HUD, enables spectator overlay (nameplates, objectives). Same primitive powers esports review and marketing cuts: record once, angle-shop on playback.

## 7. Script Events & Markers

Games annotate replays through a script API:

```rust
world.replay_mut().emit_event("boss_phase_2", &[]);
world.replay_mut().emit_event_struct("kill", &KillEvent { attacker, victim, weapon });
// Lua binding:
//   replay.event("achievement_unlocked", { id = "first_blood" })
```

Events land in the file as tagged chunks. The editor timeline renders them as colored pips:

```
 |-------|-----|---|---------------|----|---------|
   0:30   1:00 1:08               2:45  3:20
   ^kill  ^kill^boss_phase_2       ^ach  ^kill
```

Clicking jumps the cursor. Right-click opens "export clip from N seconds before to M seconds after" — QA ships crash-adjacent snippets to devs without a whole 40-minute file. Built-in events (`player_joined`, `level_loaded`, `crash`, `error_log`) emit automatically.

---

## 8. Editor Replay Panel

Dockable AssetEditor-style panel (Phase 7 shell). Opens when a `.rreplay` is double-clicked or when PIE stops with recording enabled.

```
+------ Replay: pie_2026-04-16_14-32.rreplay ---------------+
| [||] [>] [>|]  0:42 / 12:18        speed [ 1.0x v ]       |
| POV: [ Alice v ] [ Free ] [ Tactical ]                    |
| Timeline                                                  |
| |====================O==================================| |
|   kill       boss_phase_2       ach_unlocked    kill      |
| +------ Players ------+  +------ Events ------+           |
| | Alice    12 kills   |  | 00:12  kill        |           |
| | Bob       8 kills   |  | 01:08  boss_phase_2|           |
| | Carol     3 kills   |  | 02:45  achievement |           |
| +---------------------+  +--------------------+           |
| [Export Clip]  [Export Video...]  [Open in Debugger]      |
+-----------------------------------------------------------+
```

"Open in Debugger" hands the current tick to Phase 10/25 tooling to inspect ECS state at the frozen moment.

---

## 9. PIE Integration

PIE toolbar (Phase 7) adds `[x] Record session`. On Stop, a dialog offers **Save** (to `<project>/Replays/pie_YYYY-MM-DD_HH-MM.rreplay`), **Discard**, or **Open in Replay Panel**. If PIE crashes, the recorder flushes what it has (zstd frames are chunk-boundary safe) and the next editor launch prompts: "Crash recorded — open the last 30s before the crash?" This is the QA killer feature. Live recording uses `zstd_level=3`; a background repack on save re-encodes at level 19.

---

## 10. Runtime API — Shipped Games

```rust
use rustforge::replay;

replay::start(StartOptions {
    path: user_data_dir().join("replays/auto.rreplay"),
    max_duration: Some(Duration::from_secs(600)),
    ring_mode: true,           // keep only last 10 minutes
});
replay::stop();
replay::event("bomb_planted", &bomb_site_id);
if replay::is_recording() { show_rec_indicator(); }
```

**Ring mode** is the common shipping config: always recording the last N minutes, flushed on crash or player command. Memory bounded because we rotate zstd frames out as new keyframes arrive.

A small red-dot indicator appears while recording. No silent always-on recorder — starts only via game settings opt-in or explicit API. Save location defaults to the platform user data dir (`%APPDATA%/GameName/Replays/` Windows, `~/.local/share/GameName/replays/` Linux, `~/Library/Application Support/GameName/Replays/` macOS).

---

## 11. Export to Video

Pixels eventually need to be pixels. The export path runs the replay through Phase 9's headless renderer and feeds frames to an ffmpeg sidecar.

```
+------------------+     +------------------+     +----------+
| ReplayPlayer     | --> | Headless Renderer| --> | ffmpeg   |
| (FastReplay, 1x) |     | (Phase 9)        |     | subproc  |
+------------------+     +------------------+     +----------+
                                                       |--> out.mp4
```

| Preset        | Resolution | FPS | Render pass       | Use case               |
| ------------- | ---------- | --- | ----------------- | ---------------------- |
| Quick         | Native     | 60  | Same as game      | Share to Discord       |
| High          | Up to 4K   | 60  | Upscaled          | YouTube, marketing     |
| High + Cinema | Up to 4K   | 60  | +motion blur, TAA | Trailers               |

```rust
pub fn export_video(replay: &Path, range: TickRange, out: &Path, preset: ExportPreset) -> Result<()> {
    let mut player = ReplayPlayer::open(replay)?;
    let mut rx = HeadlessRenderer::new(preset.resolution, preset.fps);
    let mut ff = FfmpegSidecar::spawn(out, preset.codec)?;
    player.seek(range.start);
    while player.cursor < range.end {
        player.tick_fixed(1.0 / preset.fps as f32);
        ff.push_frame(rx.render(&player.world))?;
    }
    ff.finish()
}
```

ffmpeg is a sidecar, not link-time. If not on PATH, the dialog offers a one-click download. Editor-bundled; **not** bundled in shipped games (LGPL/GPL distribution mess). Audio taps the sim mixer to a WAV; ffmpeg muxes.

---

## 12. Determinism Requirements

Replays are worthless if re-simulation drifts. We require: (1) Rapier deterministic config from Phase 25 (`determinism: true`, single-threaded islands), (2) fixed 60Hz timestep, (3) seed capture for every RNG at session start with deterministic sub-system re-seeding, (4) no async GPU for sim — Phase 22 GPU particles are display-only, safe, (5) floating-point discipline — no `fast-math`, consistent `FMA` per build.

**Platform lock**: no promise of bit-exact replay across OS/CPU combos. A replay from `windows-x86_64-avx2` is guaranteed against another `windows-x86_64-avx2` build; cross-platform is best-effort. Header records `platform_tag`; playback on mismatch shows a yellow banner: "This replay was recorded on a different platform. Drift correction is enabled." Drift correction = the snapshot stream re-snapping every delta boundary, so even 2mm of drift gets corrected to zero every second.

---

## 13. Version Compatibility
| Mismatch             | Handling                                   |
| -------------------- | ------------------------------------------ |
| `engine_version`     | Refuse by default; loose mode option       |
| `game_version`       | Refuse by default                          |
| `game_id` (content)  | Hard refuse — different game               |
| `platform_tag`       | Warn, allow with drift correction          |

`game_id` is a content hash of asset manifest + gameplay scripts. A balance patch changes `game_version`; a truly different game changes `game_id`. **Loose mode** (opt-in) plays mismatched replays anyway — useful for archival viewing. Drift warnings are loud.

---

## 14. Phase 10 Rewind Integration

Phase 10 shipped a short-window physics rewind ring for in-gameplay uses (lag comp, parry, undo). Phase 33 refactors it to share infrastructure:

```
Before:    [ rewind ring, in-memory, 2s window ]

After:     [ ReplayRecorder ] --writes--> [ in-memory ring segment ]
                                  |
                                  +--> [ on-disk file (if enabled) ]
                                  |
                                  +--> [ Phase 10 rewind queries ]
```

Phase 10 rewind becomes a thin API over `ReplayRecorder` with in-memory backing and fixed ring size. Code paths unify: one snapshot serializer, one delta coder. Games already paying Phase 10's cost get session recording nearly free — just redirect the sink to disk.

```rust
pub fn rewind_to(&mut self, back: Duration) {
    let tick = self.current_tick() - back.as_ticks(self.tick_rate);
    self.replay_ring.seek(tick);
}
```

---

## 15. Streamer Mode — Live Replay

Remote viewers watch the session in near-real-time with ~1s buffer. Host publishes chunks as produced; a relay retains ~10 minutes and fans out to subscribers; each viewer runs a `ReplayPlayer` treating the stream as a growing file.

```
Player (host)  --tick stream--> Relay  --N subs-->  Viewers  (~1s buffer)
```

Target ~100 concurrent viewers per relay. Twitch-scale broadcasting is out of scope — games that need it tap the chunk stream into an external CDN. Host controls relay ACLs (public, unlisted, friends-only). Chunks unencrypted by default; privacy-sensitive games run their own relay over TLS.

---

## Build order

1. **Recording model** — unify Phase 16 input writer + Phase 14 snapshot writer into `ReplayRecorder`, zstd stream.
2. **File format** — seal `.rreplay` v1, reader with seek-index rebuild on truncation.
3. **Keyframes & deltas** — cadence scheduler, delta coding via Phase 14.
4. **Playback VM** — `ReplayPlayer`, `SimMode::Replay`/`FastReplay`, seek.
5. **Controls** — speed, pause, step, keyboard shortcuts.
6. **Multi-POV** — follow modes, free-cam integration with spectator overlay.
7. **Events** — script API, editor timeline markers.
8. **Editor replay panel** — timeline UI, roster, event log, dockable.
9. **PIE integration** — record checkbox, crash-recovery flow.
10. **Phase 10 refactor** — rewire rewind ring onto replay infra.
11. **Runtime API** — `replay::start/stop/event`, ring mode.
12. **Export to video** — headless driver, ffmpeg sidecar, quality presets.
13. **Determinism audit** — CI job that records 100 test matches, verifies bit-exact replay.
14. **Streamer mode** — chunk-stream publisher, relay reference server, viewer client.
15. **Version compat** — header enforcement, loose-mode toggle.

## Scope ❌

- Pixel-based screen recording. Use OBS / ShadowPlay; we are a simulation-replay system.
- Motion-capture-grade precision for animation/physics. Gameplay-accurate is the target, not VFX-house accurate.
- Cheat-proof replay attestation. Replays can be edited or forged; anti-cheat is a separate concern. The content hash is integrity, not authenticity.
- Cross-platform bit-exact determinism. We document the limit; we don't pretend to have solved it.
- Networked live co-watching at broadcast scale. ~100 viewers/relay, not 100k. Outsource to a CDN for more.
- Audio re-synthesis from scratch. We capture mixer output; we do not re-run the whole audio graph deterministically.
- Variable-tick-rate replay. Fixed timestep only.

## Risks

- **Determinism bit-rot**: one unseeded `rand::thread_rng()` in gameplay code drifts replays. Mitigation: clippy lint against non-ECS RNG, determinism CI job.
- **File size creep**: heavy particle-event spam into the replay stream blows file budgets. Mitigation: per-frame chunk budget + editor warning.
- **Seek stalls**: 30s of `FastReplay` on a heavy scene takes seconds. Mitigation: shorter keyframe interval for expensive games; seek progress UI.
- **Ffmpeg distribution**: LGPL/GPL mess, can't bundle in all jurisdictions for shipped games. Mitigation: sidecar-only, editor-side bundle.
- **PIE recorder overhead**: tanks editor tick on big scenes. Mitigation: recorder on dedicated thread behind bounded channel; drop frames with visible warning rather than stall the sim.
- **Platform drift fooling QA**: bug repros on dev machine but not CI. Mitigation: `platform_tag` check + prominent warning banner.

## Exit criteria

- Record a 10-minute PIE session; saved `.rreplay` plays back identically (state asserts pass on every keyframe).
- Editor replay panel supports play / pause / step / seek / speed 0.25x-4x / POV switch across a 4-player test match.
- Script-emitted events appear as timeline markers and are clickable.
- Export a 30-second clip to 1080p60 mp4 in Quick preset; output matches in-editor playback frame-for-frame.
- Export a 30-second clip to 4K60 mp4 in High+Cinema preset; motion blur and upscaling applied.
- Phase 10 rewind ring operates on top of replay infra with equal or better performance than its standalone implementation.
- Runtime API used by a sample game; ring-mode recording bounded to 10 minutes on a 4-hour run.
- Version mismatch refused by default, loose mode plays with warnings.
- Streamer mode: host publishes, 10 viewers attached; viewer playback lags host by <=2s.
- CI determinism job: 100 recorded matches all round-trip bit-exact on the reference platform.
