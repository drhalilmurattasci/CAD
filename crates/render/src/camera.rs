//! Camera math + GPU-facing uniform types.
//!
//! The [`Camera`] struct is a pure data/math type — no wgpu imports.
//! [`TransformUniform`] is the `#[repr(C)]` layout that matches the
//! `TransformUniform` struct inside `shader/cube.wgsl`.

use glam::{Mat4, Vec3};

/// Right-handed perspective camera suitable for editor viewports.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub position: Vec3,
    pub target:   Vec3,
    pub up:       Vec3,
    pub fov_y_rad: f32,
    pub near:     f32,
    pub far:      f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            position: Vec3::new(2.5, 2.0, 4.0),
            target:   Vec3::ZERO,
            up:       Vec3::Y,
            fov_y_rad: 45f32.to_radians(),
            near:     0.1,
            far:      100.0,
        }
    }
}

impl Camera {
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.position, self.target, self.up)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y_rad, aspect, self.near, self.far)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }
}

/// Orbit camera controller (I-7).
///
/// A classic editor camera that orbits a pivot point on a sphere.
/// The editor shell already ships yaw/pitch/zoom state from mouse
/// input — this struct turns that state into a concrete `Camera` for
/// the render path.
///
/// Conventions:
///   - `yaw` rotates around Y (horizontal drag).
///   - `pitch` rotates around X after yaw (vertical drag), clamped to
///     just short of ±π/2 so the up vector never flips.
///   - `distance` is the radius; the dolly-zoom input multiplies it.
///   - `target` is the orbit pivot; pan translates it in camera-local
///     screen space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub target:   Vec3,
    pub yaw:      f32,
    pub pitch:    f32,
    pub distance: f32,
    pub fov_y_rad: f32,
    pub near:     f32,
    pub far:      f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self {
            target:    Vec3::ZERO,
            yaw:       std::f32::consts::FRAC_PI_4,      // 45°
            pitch:     (-22.0_f32).to_radians(),          // look down a bit
            distance:  6.0,
            fov_y_rad: 45f32.to_radians(),
            near:      0.1,
            far:       200.0,
        }
    }
}

impl OrbitCamera {
    /// Minimum distance the camera can dolly in to — keeps the near
    /// plane from swallowing the pivot.
    pub const MIN_DISTANCE: f32 = 0.2;
    /// Maximum pitch magnitude in radians — just shy of straight up or
    /// down so `look_at` never produces a degenerate up vector.
    pub const MAX_PITCH: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

    /// World-space eye position derived from yaw/pitch/distance.
    pub fn eye(&self) -> Vec3 {
        let cp = self.pitch.cos();
        let sp = self.pitch.sin();
        let cy = self.yaw.cos();
        let sy = self.yaw.sin();
        // Spherical → Cartesian with Y up.
        let direction = Vec3::new(cp * sy, sp, cp * cy);
        self.target + direction * self.distance
    }

    /// Convert to a plain `Camera` the render path already consumes.
    pub fn to_camera(&self) -> Camera {
        Camera {
            position:  self.eye(),
            target:    self.target,
            up:        Vec3::Y,
            fov_y_rad: self.fov_y_rad,
            near:      self.near,
            far:       self.far,
        }
    }

    /// Apply a dolly multiplier — `factor > 1.0` pulls the camera
    /// away, `< 1.0` brings it closer. Clamped to `MIN_DISTANCE..far`.
    pub fn dolly(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(Self::MIN_DISTANCE, self.far * 0.5);
    }

    /// Yaw/pitch by the given radian deltas, clamping pitch so the up
    /// vector never flips.
    pub fn orbit(&mut self, yaw_delta: f32, pitch_delta: f32) {
        self.yaw += yaw_delta;
        self.pitch = (self.pitch + pitch_delta).clamp(-Self::MAX_PITCH, Self::MAX_PITCH);
    }

    /// Pan the pivot in screen-aligned axes.
    /// `dx` slides along the camera's right vector, `dy` along its up
    /// vector, scaled by `distance` so pan feels size-invariant.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let forward = (self.target - self.eye()).normalize_or_zero();
        let right = forward.cross(Vec3::Y).normalize_or_zero();
        let up = right.cross(forward).normalize_or_zero();
        let scale = self.distance * 0.0015;
        self.target += right * (-dx * scale) + up * (dy * scale);
    }
}

/// GPU-side layout for the cube shader's `TransformUniform`. 3×mat4
/// + 3×vec4 (light direction/intensity + light color/ambient + albedo).
///
/// Layout history:
///  - I-3: just `view_proj` + `model`.
///  - I-4: dynamic-offset friendly stride — still 128 bytes.
///  - I-12: added the two light vec4s so the Lambert shader has the
///    data it needs without a second bind group.
///  - I-31: added `albedo` — per-entity RGBA tint multiplied into the
///    final shaded color. Defaults to white `[1,1,1,1]` so scenes
///    authored before materials existed keep their vertex colors.
///    Total 176 bytes — still well under the 256-byte dynamic-offset
///    stride.
///  - I-33: added `light_view_proj` — the light-space clip-matrix
///    used by the shadow map pass (depth-only render from the light's
///    POV) and re-used by the main fragment shader to project world
///    positions into the shadow map for occlusion testing. Total 240
///    bytes — still fits a single 256-byte dynamic-offset slot.
///
/// `#[repr(C)]` + `Pod` + `Zeroable` for bytemuck.
#[repr(C)]
#[derive(Copy, Clone, bytemuck::Pod, bytemuck::Zeroable, Debug)]
pub struct TransformUniform {
    pub view_proj:   [[f32; 4]; 4],
    pub model:       [[f32; 4]; 4],
    /// I-33: light-space projection*view matrix, used both by the
    /// shadow pass vertex shader (writes depth from the light's POV)
    /// and the main pass fragment shader (projects fragments into the
    /// shadow map for an occlusion test). Built once per frame by the
    /// editor from `DirectionalLight.direction` + scene extents; all
    /// instances within a frame share the same matrix.
    pub light_view_proj: [[f32; 4]; 4],
    /// xyz = normalized world-space direction from surface toward the
    /// light, w = intensity scalar.
    pub light_dir:   [f32; 4],
    /// xyz = light RGB color, w = ambient term (flat additive).
    pub light_color: [f32; 4],
    /// xyzw = per-entity material albedo (RGBA). Multiplied with the
    /// vertex color inside the shader so meshes can be tinted without
    /// touching their vertex buffers.
    pub albedo:      [f32; 4],
}

/// Light parameters the shader consumes. Split out from
/// [`TransformUniform`] so I-13's ECS light driver can compose it once
/// per frame and reuse it across every cube instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DirectionalLight {
    /// World-space direction *toward* the light source from the surface
    /// — not the light's emission direction. Stored as the vector the
    /// shader can dot with a normal directly.
    pub direction: Vec3,
    pub color:     [f32; 3],
    pub intensity: f32,
    pub ambient:   f32,
}

impl Default for DirectionalLight {
    fn default() -> Self {
        // A slightly warm key light coming down+forward from the top-
        // right. Matches the default orbit camera's expectations so the
        // cubes read dimensionally out of the box.
        Self {
            direction: Vec3::new(0.5, 0.8, 0.3).normalize(),
            color:     [1.0, 0.97, 0.92],
            intensity: 1.0,
            ambient:   0.18,
        }
    }
}

/// Default albedo — identity multiplier for the shader tint. Kept as
/// a named constant so tests + call sites share one definition.
pub const DEFAULT_ALBEDO: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

impl TransformUniform {
    pub fn new(view_proj: Mat4, model: Mat4) -> Self {
        Self::with_light(view_proj, model, DirectionalLight::default())
    }

    pub fn with_light(view_proj: Mat4, model: Mat4, light: DirectionalLight) -> Self {
        Self::with_material(view_proj, model, light, DEFAULT_ALBEDO)
    }

    /// I-31: build a full uniform including the per-entity albedo tint.
    /// Legacy callers should keep using `with_light` — they'll get the
    /// identity albedo and render exactly as before.
    pub fn with_material(
        view_proj: Mat4,
        model: Mat4,
        light: DirectionalLight,
        albedo: [f32; 4],
    ) -> Self {
        Self::with_shadow(view_proj, model, Mat4::IDENTITY, light, albedo)
    }

    /// I-33: full constructor — bundles in the light-space clip matrix
    /// used by the shadow map pass. Callers that don't render shadows
    /// keep passing `Mat4::IDENTITY` (via `with_material` / `with_light`)
    /// and the shader's shadow sample degenerates to "fully lit".
    pub fn with_shadow(
        view_proj: Mat4,
        model: Mat4,
        light_view_proj: Mat4,
        light: DirectionalLight,
        albedo: [f32; 4],
    ) -> Self {
        let d = light.direction.normalize_or_zero();
        Self {
            view_proj:       view_proj.to_cols_array_2d(),
            model:           model.to_cols_array_2d(),
            light_view_proj: light_view_proj.to_cols_array_2d(),
            light_dir:       [d.x, d.y, d.z, light.intensity],
            light_color:     [light.color[0], light.color[1], light.color[2], light.ambient],
            albedo,
        }
    }
}

/// I-33: build a right-handed orthographic light-space view-projection
/// matrix from a directional light's direction and a half-extent
/// (the shadow frustum is a box `2*half_extent` units on a side,
/// centered on `center`). The light's "eye" sits `half_extent` behind
/// `center` along the light direction, so the whole frustum fits in
/// front of the near plane.
///
/// Kept free of scene-specific data — the editor plumbs the center
/// (e.g. orbit pivot) + half_extent (e.g. grid extent) from its own
/// state. This keeps the renderer ignorant of the editor's world size.
pub fn directional_light_view_proj(
    light_dir_to_source: Vec3,
    center: Vec3,
    half_extent: f32,
) -> Mat4 {
    // `light_dir_to_source` is the direction *from surface to light*,
    // matching `DirectionalLight.direction`. The light eye therefore
    // sits along that vector, far enough away to see the whole
    // frustum. Guarantee a non-zero direction; fall back to straight
    // down so the matrix stays well-formed if upstream feeds us zero.
    let dir = if light_dir_to_source.length_squared() > 1e-8 {
        light_dir_to_source.normalize()
    } else {
        Vec3::Y
    };
    let eye = center + dir * (half_extent * 2.0);
    // Choose an up vector that isn't parallel to the light direction —
    // `look_at_rh` requires this for a valid basis. Y is the usual
    // "up"; when the light points nearly straight up/down we swap to
    // Z so the cross product doesn't collapse.
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let view = Mat4::look_at_rh(eye, center, up);
    // Orthographic box, 2*half_extent units per side. `near = 0.0` is
    // valid with `look_at` because the eye is already `2*half_extent`
    // in front of the box; `far = 4*half_extent` leaves room behind
    // the center without clipping distant geometry.
    let proj = Mat4::orthographic_rh(
        -half_extent, half_extent,
        -half_extent, half_extent,
        0.0,          4.0 * half_extent,
    );
    proj * view
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_uniform_is_240_bytes() {
        // 3 mat4x4<f32> (192) + 3 vec4<f32> (48) = 240 bytes. Still
        // under the 256-byte dynamic-offset stride, so CubeRenderer's
        // instance packing is unaffected even after I-33 added the
        // light-space matrix.
        assert_eq!(std::mem::size_of::<TransformUniform>(), 240);
    }

    #[test]
    fn transform_uniform_fits_in_dynamic_offset_slot() {
        // Dynamic-offset alignment is 256 bytes on every wgpu backend
        // we ship; if the uniform grows past that, CubeRenderer /
        // MeshInstanceRenderer need a storage buffer instead.
        assert!(std::mem::size_of::<TransformUniform>() <= 256);
    }

    #[test]
    fn with_light_defaults_light_view_proj_to_identity() {
        // Legacy callers passing through `with_light` must not cause
        // the shadow lookup to misbehave — identity leaves the shadow
        // coords passing through unchanged, and the main shader's
        // "outside shadow map" branch defaults to fully lit.
        let u = TransformUniform::with_light(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            DirectionalLight::default(),
        );
        assert_eq!(u.light_view_proj, Mat4::IDENTITY.to_cols_array_2d());
    }

    #[test]
    fn with_shadow_records_light_view_proj() {
        let light_vp = Mat4::from_scale(Vec3::splat(2.0));
        let u = TransformUniform::with_shadow(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            light_vp,
            DirectionalLight::default(),
            DEFAULT_ALBEDO,
        );
        assert_eq!(u.light_view_proj, light_vp.to_cols_array_2d());
    }

    #[test]
    fn directional_light_view_proj_is_finite() {
        let m = directional_light_view_proj(Vec3::new(0.5, 0.8, 0.3), Vec3::ZERO, 10.0);
        for row in m.to_cols_array() {
            assert!(row.is_finite(), "light view-proj entry should be finite");
        }
    }

    #[test]
    fn directional_light_view_proj_handles_straight_down_light() {
        // Edge case: light direction nearly parallel to the default up
        // vector. The builder swaps to Z-up so `look_at_rh` stays
        // well-formed.
        let m = directional_light_view_proj(Vec3::Y, Vec3::ZERO, 5.0);
        for row in m.to_cols_array() {
            assert!(row.is_finite());
        }
    }

    #[test]
    fn directional_light_view_proj_places_origin_inside_frustum() {
        // With center at origin and a generous extent, the origin
        // should project to clip-space coords within [-1, 1] on x/y
        // and [0, 1] on z (wgpu clip-space Z range).
        let m = directional_light_view_proj(Vec3::new(0.0, 1.0, 0.5), Vec3::ZERO, 10.0);
        let clip = m * glam::Vec4::new(0.0, 0.0, 0.0, 1.0);
        assert!(clip.x.abs() < 1.0);
        assert!(clip.y.abs() < 1.0);
        assert!(clip.z >= 0.0 && clip.z <= 1.0);
    }

    #[test]
    fn transform_uniform_defaults_to_identity_albedo() {
        // Legacy `with_light` path must keep rendering the way it did
        // before I-31 added albedo. Identity = (1,1,1,1), i.e. no tint.
        let u = TransformUniform::with_light(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            DirectionalLight::default(),
        );
        assert_eq!(u.albedo, DEFAULT_ALBEDO);
    }

    #[test]
    fn transform_uniform_with_material_records_albedo() {
        let albedo = [0.2, 0.4, 0.6, 0.8];
        let u = TransformUniform::with_material(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            DirectionalLight::default(),
            albedo,
        );
        assert_eq!(u.albedo, albedo);
    }

    #[test]
    fn default_directional_light_direction_is_unit_length() {
        let dir = DirectionalLight::default().direction;
        assert!((dir.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn transform_uniform_with_light_normalizes_direction() {
        let light = DirectionalLight {
            direction: Vec3::new(3.0, 0.0, 4.0),
            ..Default::default()
        };
        let u = TransformUniform::with_light(Mat4::IDENTITY, Mat4::IDENTITY, light);
        let [dx, dy, dz, _] = u.light_dir;
        let len = (dx * dx + dy * dy + dz * dz).sqrt();
        assert!((len - 1.0).abs() < 1e-5);
    }

    #[test]
    fn camera_view_proj_respects_aspect() {
        let cam = Camera::default();
        let square = cam.view_proj(1.0);
        let wide   = cam.view_proj(2.0);
        // Columns [0][0] differ when aspect changes (horizontal scale).
        assert_ne!(square.to_cols_array_2d()[0][0], wide.to_cols_array_2d()[0][0]);
    }

    #[test]
    fn default_camera_looks_at_origin() {
        let cam = Camera::default();
        assert_eq!(cam.target, Vec3::ZERO);
        assert!(cam.position.length() > 0.0);
    }

    #[test]
    fn orbit_camera_dolly_clamps_to_min_distance() {
        let mut cam = OrbitCamera::default();
        cam.distance = 0.5;
        cam.dolly(0.01);
        assert!(cam.distance >= OrbitCamera::MIN_DISTANCE);
    }

    #[test]
    fn orbit_camera_pitch_is_clamped() {
        let mut cam = OrbitCamera::default();
        cam.orbit(0.0, 100.0); // try to flip
        assert!(cam.pitch <= OrbitCamera::MAX_PITCH);
        cam.orbit(0.0, -100.0);
        assert!(cam.pitch >= -OrbitCamera::MAX_PITCH);
    }

    #[test]
    fn orbit_camera_eye_orbits_target_at_distance() {
        let mut cam = OrbitCamera::default();
        cam.target = Vec3::ZERO;
        cam.pitch = 0.0;
        cam.yaw = 0.0;
        cam.distance = 5.0;
        // yaw = 0, pitch = 0 → direction = (0, 0, 1), eye = (0, 0, 5)
        let eye = cam.eye();
        assert!((eye - Vec3::new(0.0, 0.0, 5.0)).length() < 1e-4);
    }

    #[test]
    fn orbit_camera_to_camera_roundtrips_target_and_up() {
        let cam = OrbitCamera::default();
        let realized = cam.to_camera();
        assert_eq!(realized.target, cam.target);
        assert_eq!(realized.up, Vec3::Y);
    }
}
