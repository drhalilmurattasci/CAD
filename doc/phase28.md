# Phase 28 — XR (VR/AR) Support

Unreal has shipped OpenXR as a first-class platform for years. Phase 28 brings RustForge to feature parity: not a plugin, not a fork, not a sample project — a supported target alongside desktop and console. The backend is OpenXR through `openxr-rs`, which covers SteamVR, Meta Quest (native Android + Link), Windows Mixed Reality, PICO, and Varjo without vendor-specific code in the engine. Apple VisionOS is intentionally out of scope; its Metal-only, Swift-fronted stack is a separate future phase.

XR is not a cosmetic layer on top of the renderer. It reshapes the frame loop (waits on the runtime, not the swapchain), the camera rig (two eyes, tracked origin), the input system (6DoF controllers with interaction profiles), and the performance envelope (90/120 Hz with a fixed budget per frame, no dynamic resolution saving you). The goal of this phase is to land those changes as first-class subsystems and wire everything that already exists — Phase 2's render-to-texture, Phase 16's action system, Phase 21's rendering tiers, Phase 10's profiler — into a coherent XR path without forking the engine.

## Goals

By end of Phase 28:

1. **OpenXR session** negotiated on launch when a headset is present, with graceful fallback to the flat desktop path when none is.
2. **Stereo rendering** via multiview where the GPU supports it, double-render otherwise; integrated with the Phase 21 tier system as a High-tier-minimum target.
3. **Head and controller poses** published to the ECS every frame, sampled from the OpenXR predicted display time.
4. **Controller input** plumbed through Phase 16 as a new device type, with OpenXR interaction profiles as the binding backend.
5. **Haptics** driven by the same action system (output actions, not a side channel).
6. **Hand tracking** joint data exposed as an input source; skeleton retargeting to in-game hand meshes. Gesture recognition is not part of this phase.
7. **Teleportation and smooth locomotion** helper components shipped first-party, opt-in, not forced on any project.
8. **AR passthrough** via OpenXR alpha blending and **spatial anchors** via the spatial-entity extension, wherever the runtime supports them.
9. **Profiler hooks** extending Phase 10 with reprojection rate, app-side frame time, compositor latency, and predicted-time drift.
10. **Editor VR preview**: when a headset is attached, the editor viewport can be mirrored into the HMD for in-headset prefab placement.
11. **Guardian / play-area API** exposed to game code, plus first-party comfort-tunnel and warning-fade components.

Non-goals: eye tracking, facial expression tracking, mixed-reality capture, WebXR, VisionOS, and any vendor SDK outside the OpenXR umbrella.

## 1. OpenXR backend (`rustforge-xr`)

New crate. Sits under `rustforge-core` as an optional subsystem behind a `xr` feature.

```
crates/rustforge-xr/
├── src/
│   ├── lib.rs              # public XrSession, XrFrame
│   ├── instance.rs         # xr::Instance, extension negotiation
│   ├── session.rs          # session lifecycle, frame loop
│   ├── swapchain.rs        # xr::Swapchain <-> wgpu::Texture bridge
│   ├── action.rs           # ActionSet <-> Phase 16 adapter
│   ├── spaces.rs           # reference spaces, local/stage/view
│   ├── composition.rs      # projection + quad + passthrough layers
│   └── extensions.rs       # feature probing
```

At startup the engine probes for an OpenXR runtime. If present, the renderer is told to defer swapchain creation until `XrSession::begin` has negotiated image formats with the runtime — XR swapchains are owned by the compositor, not wgpu, and we borrow views from them each frame.

```rust
pub struct XrSession {
    instance:   xr::Instance,
    system:     xr::SystemId,
    session:    xr::Session<xr::Vulkan>,   // wgpu-vulkan interop
    stage:      xr::Space,                 // room-scale reference
    view:       xr::Space,                 // head
    swapchains: [EyeSwapchain; 2],
    actions:    XrActionBridge,
}

impl XrSession {
    pub fn wait_frame(&mut self) -> XrFrameState { /* ... */ }
    pub fn begin_frame(&mut self, s: &XrFrameState) -> XrFrame<'_> { /* ... */ }
    pub fn end_frame(&mut self, frame: XrFrame, layers: &[&dyn Layer]) { /* ... */ }
}
```

The opinion here is that the XR frame loop drives the engine tick, not the other way around. `xrWaitFrame` gives a predicted display time; everything — physics step target, animation sampling, camera pose — is parameterised by that time. The desktop loop path keeps its `winit`-driven pump; the XR path substitutes the OpenXR wait/begin/end triple.

## 2. Graphics interop: wgpu ↔ OpenXR

OpenXR hands the engine a swapchain per eye (or one array swapchain when multiview is available). `openxr-rs` with the Vulkan binding gives us `VkImage` handles; we import those into wgpu with `wgpu::hal::vulkan::Device::texture_from_raw`. D3D12 follows the same pattern on Windows. Metal would not, which is part of why VisionOS is deferred.

Format: the runtime advertises a preferred list (typically `B8G8R8A8_SRGB`, sometimes `R16G16B16A16_FLOAT` for HDR). We pick the first that matches our render target format; if none match, we render to an intermediate and blit.

```rust
struct EyeSwapchain {
    xr_swapchain: xr::Swapchain<xr::Vulkan>,
    images:       Vec<wgpu::Texture>,   // imported views, one per image
    extent:       wgpu::Extent3d,
}
```

Per frame, each eye:

1. `xr_swapchain.acquire_image()` → index.
2. `xr_swapchain.wait_image(timeout)` — must be done before recording draws.
3. Record wgpu commands targeting `images[index]`.
4. `xr_swapchain.release_image()` — must be done before `xrEndFrame`.

Missing any of those four in order ends the session with a runtime error. We wrap the sequence in an RAII `EyeRender<'s>` guard.

## 3. Stereo rendering path

Two options, selected at startup based on GPU capability:

| Path | Requires | Cost | Notes |
|------|----------|------|-------|
| Multiview | `VK_KHR_multiview` + shader `gl_ViewIndex` | 1 draw, 2 outputs | Preferred; all Quest GPUs, most modern desktop |
| Double-render | Nothing | 2 draws | Fallback; higher CPU cost, any wgpu adapter |

Multiview changes the pipeline slightly: the color target is a 2-layer texture, the shader reads `@builtin(view_index)` to pick the eye's view-projection matrix from a uniform array, and culling has to use a combined frustum. wgpu exposes multiview via `RenderPassDescriptor::multiview = Some(2)`.

```
eye layout (multiview):

   ┌──────────────────────────┐
   │  left eye  (layer 0)     │
   ├──────────────────────────┤
   │  right eye (layer 1)     │
   └──────────────────────────┘
   single draw call, view_index selects layer
```

XR is set as a High-tier minimum in the Phase 21 tier matrix. That means: deferred shading, cascaded shadows, SSAO, temporal AA. Low/Medium tier features that require scene-wide re-projection (screen-space reflections with multi-bounce tracing, heavy volumetric fog) are gated off by default — they fight with foveation and reprojection and blow the frame budget.

### 3.1 Fixed foveated rendering

Variable-rate shading per OpenXR `XR_FB_foveation` (Quest) or `XR_EXT_foveated_rendering`. Peripheral regions shade at 1/2 or 1/4 rate. Behind an `xr.foveation.level` setting: `off | low | medium | high`. Default `medium` on mobile, `off` on PCVR.

### 3.2 Lens distortion and chromatic aberration

Handled by the OpenXR compositor. The engine must not apply any — render the undistorted eye image, hand it to the compositor, done. This is a common source of bugs: project templates with "VR post-processing" stacks forked from non-OpenXR engines often double-distort. Our render graph asserts no lens-warp pass is active when an XR session is in flight.

## 4. XR camera rig and scene integration

Runtime model: one `XrRig` component represents the tracked origin. Children are a `head`, two `hand` nodes (left / right), and any number of `anchor` nodes. Each frame, the XR subsystem writes poses onto those entities before the first gameplay system runs.

```
                    XR rig (world pose)
                           │
         ┌─────────────────┼─────────────────┐
         │                 │                 │
      head                hand (left)    hand (right)
     (view space)       (grip + aim)    (grip + aim)
         │                 │                 │
     camera             controller        controller
     (stereo)           mesh + beam      mesh + beam
```

```rust
#[derive(Component)]
pub struct XrRig {
    pub tracking_origin: TrackingOrigin,   // Local | Stage | LocalFloor
    pub play_area:       Option<Aabb2>,    // guardian bounds
}

#[derive(Component)]
pub struct XrHead {
    pub view_pose: Pose,
    pub ipd_m:     f32,
}

#[derive(Component)]
pub struct XrHand {
    pub handedness: Handedness,
    pub grip_pose:  Pose,
    pub aim_pose:   Pose,
    pub tracked:    bool,
}
```

Opinion: poses land on the ECS, not in some side-channel singleton. Every system that reads a transform sees the controller in the same shape as any other entity, which keeps gameplay scripting ignorant of XR-vs-flat.

## 5. Input: extending Phase 16 with XR controllers

Phase 16 established the invariant that gameplay code reads `Action`s, not devices. XR slots in as a new device category without breaking that rule.

```rust
pub enum DeviceKind {
    Keyboard,
    Mouse,
    Gamepad(GamepadId),
    Touch,
    Pen,
    XrController(Handedness),   // new
    XrHand(Handedness),         // new — tracked hands
}
```

The bindings file (`.rinput`) gains an `xr` section. Source entries for XR refer to OpenXR interaction profile paths; the engine handles translation to and from.

```ron
// snippet from player.rinput
xr: {
    "gameplay.jump": [ "/user/hand/right/input/a/click" ],
    "gameplay.grab": [
        "/user/hand/left/input/squeeze/value",
        "/user/hand/right/input/squeeze/value",
    ],
    "gameplay.move": [
        { axis2d: "/user/hand/left/input/thumbstick" },
    ],
}
```

### 5.1 Interaction profile suggestions

The engine suggests bindings for the common profiles and lets the runtime rebind them for whatever hardware is actually attached. Profiles we ship default suggestions for:

- `/interaction_profiles/oculus/touch_controller` (Quest, Rift)
- `/interaction_profiles/valve/index_controller`
- `/interaction_profiles/htc/vive_controller`
- `/interaction_profiles/microsoft/motion_controller` (WMR)
- `/interaction_profiles/khr/simple_controller` (fallback)

Users can override suggestions per project. Suggesting one profile's bindings doesn't limit the app to that hardware — the runtime rebinds for the user's actual device.

### 5.2 Hand tracking as an input source

When `XR_EXT_hand_tracking` is present, joints are exposed as a 26-element array of poses per hand. A `HandSkeletonSource` maps those joints onto a game-provided skeleton via a name map (`Thumb_Metacarpal` → bone id). Gesture recognition — pinch, point, fist — is not shipped; users can drive their own from joint data.

### 5.3 Haptics

Output actions, same registry as input. Amplitude is 0.0..=1.0, duration is a `Duration`, frequency is a hint the runtime may ignore.

```rust
pub struct Haptic {
    pub amplitude: f32,
    pub duration:  Duration,
    pub frequency: Option<f32>,   // Hz; None = runtime default
}

// Fire from a system:
haptics.fire("gameplay.feedback.hit", Haptic {
    amplitude: 0.6, duration: Duration::from_millis(80), frequency: None,
});
```

## 6. Locomotion helpers

First-party components, opt-in. Nothing in the engine forces a project to use them; a user writing a cockpit sim ignores them entirely.

- `Teleporter`: parabolic aim from the controller, projects onto navmesh or world geometry, plays a fade-blink on trigger. Configurable max distance, valid-surface tag filter.
- `SmoothLocomotion`: thumbstick-driven velocity, snap-turn or smooth-turn, configurable deadzone and acceleration curves.
- `ComfortTunnel`: vignette that darkens the peripheral view proportional to angular velocity. Reduces motion sickness; exposed as a separate component so games can wire it to any locomotion scheme, not just ours.
- `GuardianFade`: crossfade to a wireframe or solid color when the head approaches the guardian boundary. Respects the `XrRig::play_area`.

These are in `rustforge-xr-locomotion`, a separate crate. The base `rustforge-xr` does not depend on it.

## 7. AR: passthrough and spatial anchors

Passthrough via `XR_FB_passthrough` (Quest), `XR_EXT_passthrough` where standardised, or `XR_ENVIRONMENT_BLEND_MODE_ALPHA_BLEND` on runtimes that expose AR natively. The engine checks `supported_blend_modes` at session start and surfaces a `XrBlendMode` enum.

```rust
pub enum XrBlendMode {
    Opaque,            // VR
    Additive,          // some AR HMDs
    AlphaBlend,        // passthrough / see-through AR
}
```

For AR mode the scene clear color becomes transparent, and any sky dome / fog pass is suppressed by the render graph.

Spatial anchors via `XR_MSFT_spatial_anchor` or `XR_FB_spatial_entity`. The engine exposes:

```rust
pub trait SpatialAnchorStore {
    fn create(&self, pose: Pose) -> AnchorId;
    fn save(&self, id: AnchorId) -> io::Result<AnchorHandle>;   // persistent uuid
    fn load(&self, handle: AnchorHandle) -> io::Result<AnchorId>;
    fn destroy(&self, id: AnchorId);
}
```

Anchors attach to entities via an `XrAnchor { id: AnchorId }` component. Each frame, the XR subsystem refreshes the entity's transform from the anchor's current tracked pose — the runtime handles drift correction, the engine just reads.

## 8. Performance budgets and the Phase 10 profiler

XR targets are not negotiable. A dropped frame is not a frame-time blip — it's motion sickness. The compositor reprojects missed frames, but the app is still the villain.

| Platform | Target | Per-eye resolution |
|----------|--------|--------------------|
| Quest 3 (standalone) | 90 fps, stretch 120 | 2064 × 2208 |
| Quest 2 (standalone) | 72 fps, stretch 90  | 1832 × 1920 |
| PCVR (Index, Vive Pro 2) | 90–144 fps | hardware-dependent |
| Varjo Aero | 90 fps | 2880 × 2720 |

Phase 10's profiler gains an `xr` tab:

- **App FPS**: frames the engine submitted on time.
- **Compositor FPS**: what the user actually sees.
- **Reprojection rate**: missed-frame percentage; anything non-zero is a problem.
- **Predicted-time drift**: `actual_display_time - predicted` in ms.
- **GPU per-eye ms**: left and right separately when double-rendering.
- **Thermal state** (Quest): the runtime publishes a throttling signal; we surface it.

Budget rule of thumb, Quest 3 at 90 fps: 11.1 ms total, leave 2 ms for compositor, leaves 9 ms for the app. That's 4.5 ms GPU per eye with multiview, or 9 ms with double-render — which is why multiview is the default.

## 9. Editor VR preview

Optional. When a headset is connected and the editor has `xr.preview = true`, the editor viewport is mirrored into the HMD using a minimal OpenXR session: a single quad layer showing the current camera output. The user can place prefabs in-headset by pointing a controller at a target surface and pressing trigger, which fires back into the editor's command stream.

This is not in-headset scripting, not a full editor-in-VR experience, not a Quill clone. It's specifically "let me walk around the level at 1:1 scale and drop things" — the thing teams actually ask for and the thing that is small enough to ship. Anything beyond that is a future phase.

## 10. Guardian and safety UX

OpenXR publishes the play area as either a rectangle (most runtimes) or a polygon (some). The engine normalises to `Aabb2` for the rectangle case and `Polygon2` where available, stored on `XrRig::play_area`. Games can read this; they must not ignore it.

Recommended patterns, shipped as components in `rustforge-xr-locomotion`:

- `GuardianFade` (as above).
- `ComfortTunnel` (as above).
- `BoundaryWarning`: raises a `BoundaryApproached` event when the head is within N cm of the boundary. Default UX is a subtle wireframe overlay; games can replace it.

Documentation ships with the comfort guidelines from the Oculus VRC and the SteamVR submission checklist distilled into a short set of defaults. Nothing is enforced — it's a game engine, not a compliance tool — but the defaults do the right thing.

## 11. Build order

1. OpenXR instance + session lifecycle, behind the `xr` feature flag. Launch app with an HMD, get a session, see the `xr.session.state` flip through `READY → SYNCHRONIZED → VISIBLE → FOCUSED`.
2. wgpu ↔ OpenXR swapchain interop, single eye first (throwaway), confirm a clear-color frame shows up in the headset.
3. Stereo path: double-render first (simpler), multiview second (optimization).
4. Head pose on the ECS, driving the scene camera.
5. Controller poses on the ECS, rendering stub controller meshes.
6. Phase 16 integration: interaction profiles, action sync, first button action firing through the existing action pipeline.
7. Haptics output.
8. Hand tracking (skeleton exposure only; retargeting to a mesh is sample content).
9. Teleport + smooth locomotion + comfort tunnel components.
10. Profiler extension in Phase 10.
11. AR passthrough.
12. Spatial anchors.
13. Editor VR preview.

Each step ends with a shippable state — XR can be paused at step 7 and still be useful to a VR game that doesn't need AR or hand tracking.

## Scope ❌

The following are out of scope for Phase 28 and will not be implemented:

- Eye tracking (`XR_EXT_eye_gaze_interaction`, platform foveation driven by gaze).
- Facial expression tracking (face / body mocap from HMD cameras).
- Mixed-reality capture (third-person composite recording with a physical camera).
- WebXR (browser runtime, separate delivery channel).
- Apple VisionOS (Metal-only, Swift-fronted, no OpenXR — separate future phase).
- Vendor-specific SDKs outside OpenXR: Oculus Platform SDK for entitlements, SteamVR input via the SteamVR API rather than OpenXR, PSVR2 (non-OpenXR).
- Gesture recognition (pinch / point / fist classifiers). Joint data is exposed; classifiers are left to user scripts.
- AR occlusion from depth sensors (`XR_META_environment_depth`) — a likely Phase 29+ follow-up.
- Shared-space multi-user XR (co-located colocation with shared anchors across sessions).

## Risks

- **Runtime fragmentation**: OpenXR extensions vary wildly between runtimes. A game that works on Quest may not work on WMR if it assumes `XR_FB_passthrough` exists. Mitigation: extension probing is first-class, features degrade gracefully, the engine logs a clear extension-missing warning instead of crashing.
- **Vulkan interop on drivers**: importing VkImages into wgpu is thin ice on some Android drivers. Mitigation: double-render fallback works without multiview, and we keep a pure-Vulkan code path in reserve for Quest builds if wgpu's Vulkan backend trips on a specific driver.
- **Frame-time cliffs**: XR performance is bimodal. Either you hit 90 fps or you reproject and the user feels sick. Mitigation: the profiler surfaces reprojection rate prominently, and we document the budget in numbers, not vibes.
- **Input binding drift**: OpenXR interaction profiles are strings. A typo in `.rinput` fails silently (the action just never fires). Mitigation: the input subsystem validates profile paths against a built-in schema at load time and logs unknown paths.
- **Editor preview session conflicts**: an OpenXR runtime allows one session at a time. If the editor holds one, the launched game can't get one. Mitigation: editor drops its session when the user hits Play; reclaims it on stop.
- **Guardian UX is easy to forget**: games that never fade or warn are a submission rejection on Quest Store. Mitigation: the first-party components are opt-in but the project template includes them by default.

## Exit criteria

XR support lands when:

1. A Quest 3 APK built from the standard project template hits a sustained **90 fps at 2064×2208 per eye** on a 5,000-triangle scene with deferred shading and two shadow cascades, reprojection rate below 0.5% over a 10-minute session.
2. The same binary runs on SteamVR via Quest Link at 90 fps, on Valve Index at 120 fps, and on WMR at 90 fps, without engine changes.
3. `.rinput` bindings authored against the Oculus Touch suggestion bind correctly on Index and WMR controllers without project edits.
4. Haptics fire end-to-end from a script's `fire("hit", ..)` call to a controller vibration within one frame of display time.
5. Hand-tracking joints appear on `XrHand` components and drive a skeletal mesh in the sample scene on Quest 3 with no controller present.
6. AR passthrough scene on Quest 3 shows the real world behind transparent scene content at 90 fps, with at least one spatial anchor persisted across an app restart.
7. Editor VR preview mirrors the viewport into the HMD and round-trips a prefab-drop action from controller to editor command stream.
8. Profiler XR tab shows app fps, compositor fps, reprojection rate, per-eye GPU ms, and predicted-time drift live during a session.
9. The `xr` feature cleanly compiles out: `cargo build -p rustforge-core` without `--features xr` produces a binary with zero OpenXR symbols linked and the desktop path unaffected.
10. Sample project "XR Sandbox" ships: empty room, controllers, hands, teleport, a graspable cube, an AR mode toggle, a spatial anchor save/load button.
