// Standard 2D position + vertex-color pipeline.
//
// Shared by:
//  - `TriangleRenderer` (I-2) — colored triangle in viewport.
//  - Future 2D debug-draw helpers.
//
// Attribute layout matches `render::mesh::PositionColor2D`.

struct VsIn {
    @location(0) position: vec2<f32>,
    @location(1) color:    vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0)       color:         vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_position = vec4<f32>(in.position, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
