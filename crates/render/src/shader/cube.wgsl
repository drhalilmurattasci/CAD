// Lambert-diffuse lit cube shader — I-12, I-31, I-32, I-33.
//
// Attribute layout matches `render::mesh::PositionNormalColor3D`.
// Uniform layout matches `render::camera::TransformUniform`.
//
// Lighting model:
//   * One directional light, color + intensity stored in the uniform so
//     I-13 can drive it from an ECS `Light` component.
//   * Lambert: diffuse = base_color * (ambient + max(0, N·L) * intensity).
//   * Normal transformed by the upper-left 3×3 of the model matrix. We
//     don't yet use an inverse-transpose (valid only for uniform scale,
//     which is all the editor exposes today). Swap in when non-uniform
//     scale lands.
//
// I-32: optional material albedo texture at bind group 1. Because the
// vertex layout doesn't carry UVs (and `PositionNormalColor3D` is shared
// by the glTF importer, cube bake, and MeshUpload), we derive UVs from
// the *world position* using a triplanar projection — sample the three
// world-axis planes, weight them by the absolute normal, and blend.
//
// I-33: optional shadow map at bind group 2. A depth-only prepass
// writes the scene depth from the directional light's POV into a
// `Depth32Float` texture. This fragment shader projects every world
// position into light-clip space, converts to UV + reference depth,
// and calls `textureSampleCompare` — the comparison sampler returns 1
// for "at or in front of" and 0 for "behind", with linear filtering
// giving free 2×2 PCF. The result scales the Lambert term so occluded
// fragments drop to ambient without affecting the albedo or tint.

struct TransformUniform {
    view_proj:       mat4x4<f32>,
    model:           mat4x4<f32>,
    // I-33: light-space clip matrix. Used here purely to transform the
    // vertex into shadow-map UV coords.
    light_view_proj: mat4x4<f32>,
    // xyz = world-space direction *from* surface *to* light (already
    // normalized on the CPU), w = intensity.
    light_dir:     vec4<f32>,
    // xyz = light RGB color, w = ambient term (flat additive).
    light_color:   vec4<f32>,
    // I-31: per-entity material albedo. xyz multiplied into the final
    // shaded color, w currently reserved (alpha) — pipelines are
    // opaque today so the value is ignored at blend time but still
    // ends up in the output `vec4<f32>` for future transparent passes.
    albedo:        vec4<f32>,
};

@group(0) @binding(0) var<uniform> u_transform: TransformUniform;

// I-32: material albedo texture + sampler. The cube renderer always
// binds *something* at group 1 (DEFAULT_WHITE when the instance has no
// texture), so the shader never branches on presence — sampling the
// default-white 1×1 texture just multiplies by 1.0.
@group(1) @binding(0) var t_albedo: texture_2d<f32>;
@group(1) @binding(1) var s_albedo: sampler;

// I-33: shadow map + comparison sampler. Depth-comparison sampling
// returns a float in [0, 1] — 1 when the fragment is lit, 0 when
// shadowed, with 2×2 PCF linear filtering giving free softening.
@group(2) @binding(0) var t_shadow: texture_depth_2d;
@group(2) @binding(1) var s_shadow: sampler_comparison;

// World-space tiling scale for the triplanar projection. 1.0 means one
// full texture repeat per world-unit cube face, which matches the
// editor's "1m cube primitive" convention.
const TRIPLANAR_SCALE: f32 = 1.0;

// Depth bias applied to the shadow reference depth before comparison
// — the pipeline already applies a rasterizer-level depth bias in the
// shadow pass, but a small shader-side constant handles the residual
// self-shadowing that rasterizer bias can't reach (especially on
// thin/near-coplanar geometry).
const SHADOW_DEPTH_BIAS: f32 = 0.001;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal:   vec3<f32>,
    @location(2) color:    vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0)       color:         vec3<f32>,
    @location(1)       world_normal:  vec3<f32>,
    // I-32: world-space position piped through for triplanar UV derivation.
    @location(2)       world_pos:     vec3<f32>,
    // I-33: light-space clip-position. The FS perspective-divides this
    // and remaps xy from NDC to UV, z becomes the reference depth for
    // `textureSampleCompare`.
    @location(3)       light_clip:    vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    let world = u_transform.model * vec4<f32>(in.position, 1.0);
    out.clip_position = u_transform.view_proj * world;
    out.color = in.color;
    // Assumes uniform scale — see header. Cheap path; swap to inverse-
    // transpose when non-uniform scale arrives.
    let m3 = mat3x3<f32>(
        u_transform.model[0].xyz,
        u_transform.model[1].xyz,
        u_transform.model[2].xyz,
    );
    out.world_normal = normalize(m3 * in.normal);
    out.world_pos = world.xyz;
    out.light_clip = u_transform.light_view_proj * world;
    return out;
}

// Triplanar sample: project onto the three world-axis planes, weight
// by the squared absolute normal so the face the fragment actually
// faces dominates, and blend.
fn sample_triplanar(world_pos: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let p = world_pos * TRIPLANAR_SCALE;
    // Raise to a power so blend bands between faces stay tight — pow 4
    // gives a near-hard transition at cube edges while preserving a
    // smooth blend on curved meshes. Re-normalize so the weights sum
    // to 1.0 (prevents brightness drift when two components are near
    // equal, e.g. on a 45° face).
    var w = abs(normal);
    w = w * w * w * w;
    let w_sum = max(w.x + w.y + w.z, 1e-5);
    w = w / w_sum;

    // YZ plane (normal points along X).
    let uv_x = p.zy;
    // XZ plane (normal along Y).
    let uv_y = p.xz;
    // XY plane (normal along Z).
    let uv_z = p.xy;

    let sx = textureSample(t_albedo, s_albedo, uv_x).rgb;
    let sy = textureSample(t_albedo, s_albedo, uv_y).rgb;
    let sz = textureSample(t_albedo, s_albedo, uv_z).rgb;
    return sx * w.x + sy * w.y + sz * w.z;
}

// I-33: sample the shadow map and return a [0, 1] multiplier for the
// directional-light term. Returns 1.0 (fully lit) when the fragment
// is outside the shadow frustum — the shadow frustum is bounded, so
// geometry past its edges must not suddenly go black.
fn sample_shadow(light_clip: vec4<f32>) -> f32 {
    // Perspective divide — orthographic projections keep w at 1.0 but
    // we divide anyway so the shader doesn't silently break the day
    // the engine adds a perspective-projection spotlight.
    let proj = light_clip.xyz / max(light_clip.w, 1e-5);
    // NDC x/y in [-1, 1] → UV in [0, 1]. Flip Y because wgpu texture
    // UVs grow downward while clip-space +Y is up.
    let uv = vec2<f32>(proj.x * 0.5 + 0.5, 0.5 - proj.y * 0.5);
    // Outside the shadow frustum → pass through as lit.
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || proj.z > 1.0 || proj.z < 0.0) {
        return 1.0;
    }
    let reference = proj.z - SHADOW_DEPTH_BIAS;
    return textureSampleCompare(t_shadow, s_shadow, uv, reference);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.world_normal);
    let l = normalize(u_transform.light_dir.xyz);
    let n_dot_l = max(dot(n, l), 0.0);
    let intensity = u_transform.light_dir.w;
    let ambient = u_transform.light_color.w;
    let light_rgb = u_transform.light_color.xyz;
    // I-32: triplanar albedo sample. The default white texture is 1×1
    // opaque white, so instances with no authored texture multiply by
    // 1.0 and keep the vertex-color + tint look they had pre-I-32.
    let tex = sample_triplanar(in.world_pos, n);
    // I-31: multiply vertex color by per-entity albedo *before* the
    // Lambert term so the tint survives in unlit (shadowed) regions.
    let base = in.color * u_transform.albedo.xyz * tex;
    // I-33: shadow factor scales *only* the direct-light term. Ambient
    // is deliberately unshadowed so occluded surfaces don't crush to
    // pure black — matches how every offline renderer does it.
    let shadow = sample_shadow(in.light_clip);
    let lit = ambient + n_dot_l * intensity * shadow;
    let shaded = base * lit * light_rgb;
    // Clamp so over-bright lights don't blow out to NaN in sRGB output.
    let final_rgb = clamp(shaded, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(final_rgb, u_transform.albedo.w);
}
