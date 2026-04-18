// Depth-only shadow map vertex shader — I-33.
//
// Runs before the main scene pass. The shadow pipeline binds the same
// `TransformUniform` as the cube/mesh pipelines at group 0 (dynamic
// offset per instance) so we can reuse the uniform buffer already
// uploaded by `upload_instances` without a parallel staging buffer.
//
// Output contract:
//   * `clip_position` is `light_view_proj * model * position`.
//   * No fragment shader — the shadow pipeline's color target list is
//     empty and depth-write is on, so rasterization fills the shadow
//     map depth texture directly.
//
// Vertex layout must match `PositionNormalColor3D`; normal + color are
// read but ignored so we don't have to author a second vertex struct.

struct TransformUniform {
    view_proj:       mat4x4<f32>,
    model:           mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    light_dir:       vec4<f32>,
    light_color:     vec4<f32>,
    albedo:          vec4<f32>,
};

@group(0) @binding(0) var<uniform> u_transform: TransformUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal:   vec3<f32>,
    @location(2) color:    vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> @builtin(position) vec4<f32> {
    let world = u_transform.model * vec4<f32>(in.position, 1.0);
    return u_transform.light_view_proj * world;
}
