# Phase 27 — Sky, Atmosphere & Weather

Post-1.0, RustForge has a competitive renderer (Phase 2, Phase 21), particles (Phase 18), materials (Phase 16/20), and audio (Phase 17). The one environment-authoring gap that still trails Unreal by a wide margin is the **sky**: Unreal ships Sky Atmosphere, Volumetric Clouds, Exponential Height Fog, and a suite of weather/time-of-day knobs that together let a scene look like an outdoor location in under a minute. RustForge currently offers a baseline exponential fog (Phase 2) and a constant-color clear, which is acceptable for abstract games and indoor scenes but visibly primitive outdoors. Phase 27 closes the gap with atmospheric scattering, time-of-day, volumetric clouds, a weather preset system, and the gameplay-facing glue (wetness, lightning, ambient audio crossfade) that makes weather more than a cosmetic layer.

This phase is explicitly **not** a physically-faithful climate or planetary simulator. Scope ends where those systems start; see §Scope ❌.

## Goals

By end of Phase 27:

1. **Atmospheric scattering** — Bruneton/Hillaire-style 4-LUT precompute; Rayleigh + Mie with configurable planet radius, ozone, and ground albedo. Matches Unreal Sky Atmosphere visual envelope.
2. **Time-of-day** — sun/moon position derived from latitude, longitude, and UTC time; Scene-resource-driven; edited in a **Time of Day** panel with scrubber and speed control.
3. **Volumetric clouds** — ray-marched, shape + erosion noise, wind-advected; High-tier only. Medium/Low use a cheap sky-dome with cloud billboards.
4. **Exponential + height fog** — extends Phase 2 baseline; integrates with atmospheric transmittance so god-rays and distance haze share one consistent look.
5. **Weather preset system** — discrete states (clear / overcast / rain / snow / storm) crossfade across clouds, particles, post, and audio. Authored as `.rweather` assets.
6. **Wetness** — a scalar 0..1 exposed globally; modulates material roughness via a Phase 16/20 material graph input. Puddles optional stretch.
7. **Stars + moon** — night sky rendering driven by the same time-of-day source.
8. **Lightning** — event-based screen flash, directional-light pulse, audio trigger.
9. **Author UX** — a single `Weather` component on a scene entity drives everything; presets and transitions are data, not code.
10. **Runtime scripting** — script hooks to set weather state, force transitions, or pin time-of-day.
11. **Tier policy** — clouds: High; scattering: Medium+; sky dome: Low (WebGL2).

---

## 1. Architecture overview

All environment rendering lives behind one scene-level resource, `SkyConfig`, and one component, `Weather`. Everything else is a consumer.

```
                ┌────────────────────────┐
                │   SkyConfig (Resource) │
                │  lat/long, UTC time,   │
                │  planet params, tier   │
                └───────────┬────────────┘
                            │
     ┌──────────────────────┼──────────────────────┐
     ▼                      ▼                      ▼
┌─────────┐           ┌─────────┐            ┌─────────┐
│ Sky LUT │──────────▶│ Scatter │───────────▶│  Fog /  │
│ bake    │           │  pass   │            │ god-ray │
└─────────┘           └─────────┘            └─────────┘
                            │                      │
                            ▼                      ▼
                      ┌───────────┐          ┌──────────┐
                      │  Clouds   │          │  Final   │
                      │  (compute)│          │ compose  │
                      └───────────┘          └──────────┘
                            ▲
                            │
                ┌───────────┴───────────┐
                │ Weather (Component)   │
                │ state, transition, t  │
                └───────────┬───────────┘
                            │
          ┌─────────┬───────┼───────┬─────────┐
          ▼         ▼       ▼       ▼         ▼
       clouds   particles  post   audio    wetness
```

The opinion: **one source of truth for "what time is it and what's the weather like"**, consumed by independent rendering/audio/gameplay subsystems. This avoids the Unreal pattern where time-of-day, sky, fog, post volume, and sound ambience all drift out of sync because each is authored on a different actor.

---

## 2. Atmospheric scattering — four-LUT Bruneton/Hillaire

The aerial perspective and sky dome share a single precomputed atmosphere. The four LUTs are the industry standard for real-time:

1. **Transmittance LUT** (256×64, 2D) — transmittance from a point at altitude `h` along view ray with zenith angle `mu`. Static per planet config, bakes once.
2. **Multiple-scattering LUT** (32×32, 2D) — the Hillaire isotropic multi-scatter approximation. Cheap and convincing.
3. **Sky-view LUT** (192×108, 2D) — sky dome from current camera altitude, re-baked each frame; mapped non-linearly around horizon.
4. **Aerial perspective LUT** (32×32×32, 3D) — in-scattering + transmittance along view frustum, sampled per pixel for fog-with-sun-color.

```rust
pub struct SkyAtmosphere {
    pub planet_radius_km:     f32,     // 6360
    pub atmosphere_radius_km: f32,     // 6420
    pub rayleigh_scatter:     Vec3,    // per-channel coefficients
    pub rayleigh_scale_h_km:  f32,     // 8.0
    pub mie_scatter:          f32,     // 3.996e-3
    pub mie_absorb:           f32,     // 4.4e-3
    pub mie_scale_h_km:       f32,     // 1.2
    pub mie_g:                f32,     // 0.8 phase anisotropy
    pub ozone_absorb:         Vec3,
    pub ground_albedo:        Vec3,
}
```

Opinion: we ship a **single** planet preset (earth-like) with tuned defaults. Expose the knobs but discourage gratuitous tweaking; the Unreal trap is giving every artist the full physical parameter set and ending up with thirty inconsistent atmospheres.

WGSL sketch for the sky-view pass:

```wgsl
@fragment
fn fs_sky(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let view_dir = sky_view_uv_to_dir(uv);            // non-linear, horizon-biased
    let sun_dir  = atmo.sun_dir;
    let mu       = dot(view_dir, vec3<f32>(0.0, 1.0, 0.0));
    let cos_sun  = dot(view_dir, sun_dir);

    // sampled along a single in-scattering integration with multi-scatter LUT fed in
    let lum = integrate_scattered_luminance(
        atmo.camera_pos, view_dir, sun_dir,
        transmittance_lut, ms_lut);
    return vec4<f32>(lum, 1.0);
}
```

Performance: on a mid-range GPU the sky-view LUT is ~0.2 ms, the aerial perspective volume ~0.3 ms, full re-bake of transmittance/ms only on parameter change. Well inside the renderer budget.

---

## 3. Time of day

The authoritative state is small:

```rust
pub struct TimeOfDay {
    pub utc_seconds:       f64,   // continuous; wraps internally
    pub latitude_deg:      f32,
    pub longitude_deg:     f32,
    pub day_of_year:       u16,   // drives sun declination
    pub time_scale:        f32,   // 1.0 = realtime; 0 = paused
    pub pinned:            bool,  // script override; stops auto-advance
}
```

Sun direction is computed every frame via a standard solar position algorithm (NOAA-ish); accuracy to ~1 arcminute is plenty and cheap on CPU. Moon direction uses a simplified lunar-node model good enough for a night sky; we are not doing eclipses.

### 3.1 Time of Day panel

```
┌─ Time of Day ─────────────────────────────────────────────┐
│  Latitude   [  47.61 ]   Longitude  [ -122.33 ]           │
│  Day of yr  [   172  ]   Time scale [  60.0  ] x realtime │
│                                                           │
│  ┌──────────────────────────────────────────────────────┐ │
│  │ 00   06   09   12   15   18   21   24               │ │
│  │ ─────────●──────────────────────────                │ │
│  │         06:47                                        │ │
│  └──────────────────────────────────────────────────────┘ │
│                                                           │
│  [ ▶ Play ]  [ ⏸ Pause ]  [ ● Pin ]    Preset: [ Noon ▾ ] │
│                                                           │
│  Sun azim  148.3°    elev  28.1°                          │
│  Moon      -42.0°     3.2°   (below horizon)              │
└───────────────────────────────────────────────────────────┘
```

Presets (Sunrise / Noon / Golden / Dusk / Night / Blue Hour) just set `utc_seconds` relative to solar noon for the configured lat/long — no magic, so the result is consistent everywhere.

---

## 4. Fog integration

The baseline exponential height fog from Phase 2 is preserved for the Low tier and for scenes that explicitly disable atmospheric scattering. For Medium+, height fog's **color** is replaced by sampling the aerial-perspective LUT along the pixel's view ray. Net effect: distance haze automatically takes on sunrise pink or overcast grey without an artist touching anything.

```rust
pub struct FogConfig {
    pub mode:          FogMode,     // Exponential | HeightExponential | AtmosphereBlended
    pub density:       f32,         // base extinction /m
    pub height_falloff:f32,         // /m altitude
    pub start_distance:f32,
    pub god_rays:      GodRayConfig,
}

pub struct GodRayConfig {
    pub enabled:      bool,
    pub strength:     f32,
    pub sample_count: u32,    // 16 Medium, 48 High
}
```

God-rays are a screen-space radial-blur-from-sun done after the sky, gated by depth; cheap, convincing for the cost. It is explicitly not volumetric shadowing — that would require shadowed ray-march through the fog volume and is out of scope.

---

## 5. Volumetric clouds

High-tier only. Ray-march the cloud layer (1.5–8 km altitude by default) from camera:

- **Shape noise**: 128³ tileable Perlin-Worley, baked offline, shipped as BC4/BC6H compressed.
- **Erosion noise**: 32³ high-freq Worley, added at detail scale after cheap coverage test.
- **Coverage + type**: a 2D weather map (R=coverage, G=cloud type, B=precipitation hint) that is advected by wind and modulated by the weather preset.
- **March**: cone-march with early-out where transmittance < 0.01; 64 primary samples budget, 6 light samples per hit.

```wgsl
fn sample_cloud_density(p: vec3<f32>) -> f32 {
    let weather = textureSample(weather_map, s_linear, p.xz * INV_WEATHER_SCALE).rgb;
    let coverage = weather.r * cloud_ctl.coverage;
    if (coverage < 0.01) { return 0.0; }

    let shape = textureSample(shape_noise, s_linear, p * INV_SHAPE).rgba;
    let base  = remap(shape.r, shape.g * 0.625 + 0.25 * shape.b, 1.0, 0.0, 1.0);
    let shaped = saturate(remap(base, 1.0 - coverage, 1.0, 0.0, 1.0));
    if (shaped < 0.05) { return shaped; }

    let erode = textureSample(erosion_noise, s_linear, p * INV_ERODE).rgb;
    return saturate(shaped - (erode.r * 0.625 + erode.g * 0.25 + erode.b * 0.125) * 0.35);
}
```

Tier fallback:

- **Medium**: a sky dome with 3–4 large camera-locked cloud-layer textures parallax-offset by view direction, tinted by the sky-view LUT. Takes 0.1 ms, looks fine at a distance, obviously wrong close up.
- **Low (WebGL2)**: flat gradient dome + four baked cloud billboards that rotate slowly. No compute, no ray-march.

Clouds are **not** shadow casters in Phase 27 — shadowed terrain below moving clouds is a stretch goal. The approximation: the sun's `luminance` in the aerial LUT is multiplied by the cloud-coverage scalar at the sun's footprint each frame. Cheap, looks right globally, visibly wrong on small-scale terrain features. We accept this.

---

## 6. Weather system

The preset is the authored unit; a runtime `Weather` component owns the current state and the active crossfade.

```rust
pub enum WeatherState { Clear, Overcast, Rain, Snow, Storm, Custom(AssetId) }

pub struct WeatherPreset {       // `.rweather`
    pub name:       String,
    pub cloud:      CloudParams,     // coverage, density, type, wind
    pub particles:  Vec<ParticleOverride>, // emitter id → rate scale, spawn override
    pub post:       PostOverrides,   // fog density, bloom bias, color grade
    pub wetness:    f32,
    pub audio_bus:  AudioBusMix,     // Phase 17 bus gains
    pub lightning:  Option<LightningConfig>,
}

pub struct Weather {
    pub current:     WeatherState,
    pub target:      WeatherState,
    pub transition:  f32,            // 0..1 crossfade
    pub transition_speed: f32,       // /sec
}
```

Transitioning is a straight crossfade of every scalar parameter over `transition_speed`; discrete switches (like which particle emitter is active) use a threshold at 0.5. The opinion: **no curve authoring on transitions**. If an artist wants a non-linear fade they can chain two presets with different speeds.

### 6.1 Weather panel

```
┌─ Weather ────────────────────────────────────────────────┐
│  Current:  [ Clear         ▾ ]    t = 1.00               │
│  Target:   [ Rain          ▾ ]                           │
│  Transition [■■■■■□□□□□]  0.52    Speed [ 0.10 /sec ]    │
│                                                          │
│  Preview                                                 │
│  ┌──────────────────────────────────────────┐            │
│  │ Coverage     ●────────────────  0.72     │            │
│  │ Wetness      ●──────────────    0.55     │            │
│  │ Rain rate    ●───────           2400 p/s │            │
│  │ Fog density  ●──────            0.018    │            │
│  │ Wind         ● 14.2 m/s  bearing 112°    │            │
│  └──────────────────────────────────────────┘            │
│                                                          │
│  [ Apply Preset ]  [ Save As `.rweather` ]   [ Revert ]  │
└──────────────────────────────────────────────────────────┘
```

### 6.2 Ambience audio

The `audio_bus` field names a Phase 17 bus mix; on transition the weather system calls the audio mixer's crossfade, which handles ducking and bus gains. No new audio feature is required — this is purely integration.

---

## 7. Wetness

One scalar, `wetness ∈ [0, 1]`, exposed as:

- A uniform in the global scene UBO, readable from any material.
- An automatically-declared Phase 16/20 material graph input node `Sky.Wetness`.

Materials that participate opt in by wiring `Sky.Wetness` into a roughness-override subgraph. Typical pattern:

```
roughness_final = lerp(roughness_base, 0.04, Sky.Wetness * wet_mask)
```

`wet_mask` comes from the material itself (horizontal surfaces, up-facing normals) — we do not auto-generate it. Auto-masking by normal is available as a helper node but opt-in.

Puddles (dynamic displaced surfaces of water in low-poly depressions) are **out of scope**; cheap flat puddle decals authored into levels are the intended workflow.

---

## 8. Lightning

Lightning is an event, not a continuous effect:

```rust
pub struct LightningConfig {
    pub min_interval_s: f32,
    pub max_interval_s: f32,
    pub flash_intensity: f32,   // directional-light pulse scalar
    pub flash_color:    Vec3,
    pub audio_cue:      AssetId, // thunder bank; delay derived from distance
    pub distance_range: (f32, f32),
}
```

The weather system rolls a next-strike time inside the configured interval and fires:

1. A 2-frame directional-light pulse (the sun's intensity is temporarily boosted or a dedicated lightning light is enabled).
2. A single full-screen flash quad in post, white with a short exponential decay (120 ms).
3. A delayed audio cue — thunder delay = distance / 340 m/s, distance sampled from `distance_range`.

No procedural bolt mesh in Phase 27. Games that want a visible bolt drive it via the particle/trail system (Phase 18) with a custom emitter triggered on the lightning event.

---

## 9. Stars and moon

The night sky is a single full-screen pass that samples:

- A **star cubemap** (BC6H, ~2k per face) modulated by a twinkle noise and gated by altitude × atmosphere transmittance (so stars fade in around dusk automatically).
- The **moon** as a textured disk at the computed lunar direction, lit by the sun direction through a simple Oren-Nayar approximation, with phase derived from sun/moon geometry.
- **Milky way** band — a single equirect texture added at low brightness, rotated by sidereal time.

Star field rendering is Low-tier compatible: it is a single textured pass with no compute. It is gated behind `sun.elevation < -2°` with a small crossfade, so the day sky does not pay for it.

---

## 10. Scripting API

```rust
// From a script
weather.set_target(WeatherState::Storm);
weather.set_transition_speed(0.2);   // full crossfade in 5s
time_of_day.pin_at(hour(6.75));      // lock to 6:45 AM
time_of_day.advance_by(hours(1.0));  // skip forward
sky.trigger_lightning();             // force a strike now
```

Events the script side can subscribe to:

- `WeatherChanged { from, to }`
- `WeatherTransitionComplete { state }`
- `LightningStruck { distance, world_dir }`
- `TimeOfDayCrossed { hour }` — e.g. fire at 20:00 to spawn NPCs going home.

---

## 11. Platform tier policy

| Feature                     | Low (WebGL2)  | Medium          | High            |
|-----------------------------|---------------|-----------------|-----------------|
| Sky dome                    | gradient      | atmosphere LUT  | atmosphere LUT  |
| Aerial perspective          | ❌            | 2D only         | 3D LUT          |
| Volumetric clouds           | ❌ billboards | ❌ sky-dome CL   | ray-marched     |
| God-rays                    | ❌            | 16 samples      | 48 samples      |
| Star field                  | cubemap only  | + milky way     | + twinkle       |
| Wetness roughness override  | ✓             | ✓               | ✓               |
| Lightning flash             | ✓             | ✓               | ✓ + contact shadow pulse |

The Phase 21 tier-policy system already owns the runtime feature-flag plumbing; Phase 27 registers its flags there.

---

## 12. Rainbow and sun halo (optional)

Cheap screen-space, gated by "sun is behind camera, rain just ended, wetness > 0.3" for rainbows, and "sun disc is near horizon and aerosol scattering is high" for halos. Both are stretch; neither is on the critical path.

Rainbow is a screen-space arc drawn at the anti-solar point with a prismatic gradient modulated by remaining precipitation particle density. Halo is a radial gradient around the sun disc in the sky view pass. Both clamp to <0.1 ms and are disabled on Low.

---

## 13. `.rweather` asset

RON-serialized, hot-reloadable, diff-friendly:

```ron
WeatherPreset(
    name: "Heavy Rain",
    cloud: (
        coverage: 0.85,
        density:  0.9,
        type_bias: 0.7,        // cumulonimbus end
        wind: (speed: 12.0, bearing_deg: 124.0),
    ),
    particles: [
        (emitter: "rain_default", rate_scale: 3.0),
        (emitter: "splash_puddle", rate_scale: 2.0),
    ],
    post: (
        fog_density: 0.022,
        bloom_bias:  -0.2,
        grade_lut:   Some("overcast_cool"),
    ),
    wetness: 0.9,
    audio_bus: (ambience: 1.0, rain_loop: 0.85, thunder: 0.5),
    lightning: Some((
        min_interval_s: 6.0,
        max_interval_s: 22.0,
        flash_intensity: 9.5,
        flash_color: (1.0, 0.96, 0.9),
        audio_cue: "sfx/thunder_bank",
        distance_range: (200.0, 4500.0),
    )),
)
```

---

## Build order

1. **Atmospheric scattering** — four-LUT bake and sky-view pass. Everything else consumes these LUTs.
2. **Fog integration** — migrate Phase 2 exponential fog to read aerial perspective color.
3. **Time of day** — solar algorithm, TOD panel, driving the scattering sun direction.
4. **Stars and moon** — trivial once TOD is in, unblocks night testing.
5. **Volumetric clouds** — High-tier compute path; Medium sky-dome fallback after.
6. **Weather preset system** — `.rweather` loader, crossfade engine, component, panel.
7. **Wetness material integration** — the `Sky.Wetness` input and documentation for opting in.
8. **Lightning** — event roll, directional-light pulse, audio cue dispatch.
9. **Rainbow and sun halo** — stretch; ship only if §1–8 land inside the tier budget.

## Scope ❌

- Full planetary scale (orbital mechanics, true sphere atmosphere from space) — space engine territory.
- Procedural terrain erosion driven by weather.
- Aurora borealis — beautiful, niche, bespoke shader; not worth the maintenance.
- Real ray-traced clouds, including self-shadowed cloud-on-terrain shadows beyond the global coverage approximation.
- Climate simulation, wind fields from pressure gradients, fluid-sim weather evolution.
- Procedural lightning bolt meshes (use particle system).
- Dynamic puddle displacement / water accumulation meshes.
- Snow accumulation on geometry (covered elsewhere if ever — not Phase 27).
- Eclipse rendering.
- Seasonal foliage response (leaves changing color, snow on branches).

## Risks

- **LUT bake determinism across drivers**: the multi-scatter LUT is a reduction; subtle fp differences across vendors have bitten real shipped titles. Mitigation: bake once per config to a deterministic CPU reference, then upload — not a runtime GPU reduction.
- **Cloud ray-march cost on mid-tier hardware claiming "High"**: the 64-sample budget can blow out on narrow FOV + low altitude. Mitigation: runtime frame-time governor that scales sample count downward and surfaces a tier-downgrade warning.
- **Time-of-day ↔ baked lighting conflict**: scenes with baked GI will look wrong when the sun moves. Mitigation: documented constraint — baked GI assumes a pinned TOD; warn in-editor when Weather animates TOD on a scene with baked probes.
- **Weather crossfade popping on discrete switches** (e.g., swapping particle emitters at t=0.5). Mitigation: cross-spawn both emitters during the transition window, scale rates to 0 outside it.
- **Shader compile stalls** on first cloud render (the cloud shader is chunky). Mitigation: warm pipeline at scene load; reuse Phase 2 pipeline cache.
- **Audio bus mix fighting user mix**: scripts that set bus gains directly collide with the weather preset. Mitigation: weather bus mix is a separate "scene ambience" channel that the game script can suppress.
- **Artists tuning Rayleigh coefficients by hand** and producing thirty slightly different earths. Mitigation: hide the physical params behind a "tweak atmosphere" disclosure, ship three named sub-presets (Temperate, Desert, Arctic).

## Exit criteria

- Scattering sky reproduces Bruneton reference imagery to within visually-indistinguishable tolerance at noon, sunrise, and sunset for the shipped earth preset.
- Time of Day panel drives sun, moon, sky colour, and star visibility coherently across a 24-hour scrub with no visible popping and no desync between subsystems.
- `.rweather` crossfade between Clear → Rain → Storm → Clear cycles at default speed with no frame-time spikes >2 ms on mid-tier High hardware.
- Volumetric clouds render at <3 ms on High-tier target (Phase 21 reference GPU) at 1080p and degrade gracefully to sky-dome on Medium.
- Wetness parameter, when ramped, is reflected in every material that opted in; unopted materials are unchanged — no accidental global roughness shifts.
- Lightning strike end-to-end (flash + delayed thunder + optional script hook) runs deterministically given a fixed RNG seed.
- Low-tier (WebGL2) build renders a usable outdoor scene with gradient sky, billboard clouds, and wetness without any compute-shader dependency.
- Script API can pin TOD, force a weather transition, trigger lightning, and observe all four event types in an integration test.
- Editor hot-reload on `.rweather` edits reflects inside the running viewport within one frame.
