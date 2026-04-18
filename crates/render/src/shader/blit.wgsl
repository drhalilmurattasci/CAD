// Fullscreen-triangle blit shader (I-30).
//
// Used by `BlitRenderer` to copy an offscreen color texture into the
// final surface pass. We synthesize a single triangle that covers the
// whole viewport from `vertex_index` alone — no vertex buffer required.
//
// The trick: three corners at NDC (-1,-1), (3,-1), (-1,3). Everything
// outside the [-1,1] square is clipped, which leaves the visible area
// of the triangle coinciding exactly with the viewport. Simpler than a
// quad (fewer vertices, no index buffer) and a standard WebGPU idiom.

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> VsOut {
    // Emits the three corners of a 2x-sized triangle that covers NDC.
    // UVs are derived so uv=(0,0) lands at the top-left of the visible
    // square (matching wgpu texture convention, y-down in sampler
    // space).
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    let uv = vec2<f32>(x, 1.0 - y);
    let ndc = vec2<f32>(x * 2.0 - 1.0, y * 2.0 - 1.0);
    var out: VsOut;
    out.clip_pos = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = uv;
    return out;
}

@group(0) @binding(0) var t_source: texture_2d<f32>;
@group(0) @binding(1) var s_source: sampler;

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(t_source, s_source, in.uv);
}
