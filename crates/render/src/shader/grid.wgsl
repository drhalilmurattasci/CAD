// Editor ground grid + origin axis markers — I-11.
//
// Line-list topology; vertex colors baked at generation time so the
// shader stays identical between the faint grid lines and the bright
// axis cardinals. One camera view-projection uniform, no per-line
// transform (grid geometry is authored in world space).

struct GridUniform {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u_grid: GridUniform;

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) color:    vec3<f32>,
};

struct VsOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0)       color:         vec3<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.clip_position = u_grid.view_proj * vec4<f32>(in.position, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
