//! I-33: shadow map render target + comparison-sampler bind group.
//!
//! Owns a single depth texture the shadow pass writes into and that the
//! main scene pass samples from with a depth-comparison lookup. The
//! bind group is laid out at `@group(2)` inside `shader/cube.wgsl`:
//!   * binding 0: `texture_depth_2d` — the shadow map.
//!   * binding 1: `sampler_comparison` — PCF-friendly sampler with
//!     `CompareFunction::LessEqual` so fragments at (or in front of) the
//!     stored depth sample as fully lit.
//!
//! Why its own bind group instead of piggybacking on the offscreen
//! target's depth:
//!   * the offscreen depth is `RENDER_ATTACHMENT` only — we need an
//!     extra `TEXTURE_BINDING` flag to sample it, and mixing those
//!     flags on one texture invites backend-specific complaints;
//!   * the shadow resolution (2048×2048) is independent of the
//!     viewport resolution — separating them lets the user change one
//!     without realloc'ing the other.
//!
//! Lifetimes:
//!   - The target lives inside egui_wgpu's `CallbackResources` so it
//!     persists across frames. Resize is rare (only if the shadow
//!     resolution changes) so we don't bother with a dimension guard.

use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, CompareFunction,
    Device, Extent3d, FilterMode, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages,
    Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType, TextureUsages,
    TextureView, TextureViewDescriptor, TextureViewDimension,
};

/// Shadow map depth format. `Depth32Float` matches the offscreen
/// target and every backend supports sampling it with a comparison
/// sampler. A `Depth24Plus` variant gives a bit of VRAM back but adds
/// backend caveats for sampled-depth — not worth the knob yet.
pub const SHADOW_MAP_FORMAT: TextureFormat = TextureFormat::Depth32Float;

/// Default shadow map resolution. 2048 is the sweet spot for a small
/// scene: enough texels to avoid obvious aliasing on the ground plane,
/// cheap enough to fit comfortably in a single-pass render budget.
pub const DEFAULT_SHADOW_RESOLUTION: u32 = 2048;

pub struct ShadowMapTarget {
    resolution: u32,
    texture:    Texture,
    view:       TextureView,
    sampler:    Sampler,
    layout:     BindGroupLayout,
    bind_group: BindGroup,
}

impl ShadowMapTarget {
    pub fn new(device: &Device) -> Self {
        Self::with_resolution(device, DEFAULT_SHADOW_RESOLUTION)
    }

    pub fn with_resolution(device: &Device, resolution: u32) -> Self {
        let resolution = resolution.max(1);
        let layout = create_layout(device);
        let sampler = create_sampler(device);
        let (texture, view) = create_depth(device, resolution);
        let bind_group = create_bind_group(device, &layout, &view, &sampler);
        Self {
            resolution,
            texture,
            view,
            sampler,
            layout,
            bind_group,
        }
    }

    /// Bind group layout — pipelines that sample the shadow map take a
    /// reference to this at construction so the layout slot matches at
    /// draw time.
    pub fn bind_group_layout(&self) -> &BindGroupLayout {
        &self.layout
    }

    /// Bind group bound at `@group(2)` in the main pipeline.
    pub fn bind_group(&self) -> &BindGroup {
        &self.bind_group
    }

    /// Depth attachment used by the shadow pass render pipeline.
    pub fn depth_view(&self) -> &TextureView {
        &self.view
    }

    /// Resize if `resolution` differs. Returns `true` on actual resize
    /// so callers can invalidate cached bind groups that reference the
    /// old view — though `ShadowMapTarget` recreates its own bind
    /// group internally, downstream that referenced it by value would
    /// need to reacquire.
    pub fn ensure_resolution(&mut self, device: &Device, resolution: u32) -> bool {
        let target = resolution.max(1);
        if target == self.resolution {
            return false;
        }
        self.resolution = target;
        let (texture, view) = create_depth(device, target);
        let bind_group = create_bind_group(device, &self.layout, &view, &self.sampler);
        self.texture = texture;
        self.view = view;
        self.bind_group = bind_group;
        true
    }

    pub fn resolution(&self) -> u32 {
        self.resolution
    }

    /// Exposed for diagnostics.
    pub fn texture(&self) -> &Texture {
        &self.texture
    }
}

fn create_layout(device: &Device) -> BindGroupLayout {
    device.create_bind_group_layout(&BindGroupLayoutDescriptor {
        label:   Some("rustforge.render.shadow.bgl"),
        entries: &[
            BindGroupLayoutEntry {
                binding:    0,
                visibility: ShaderStages::FRAGMENT,
                ty:         BindingType::Texture {
                    // `Depth` sample type paired with a comparison
                    // sampler — WGSL `textureSampleCompare` requires
                    // exactly this combo.
                    sample_type:    TextureSampleType::Depth,
                    view_dimension: TextureViewDimension::D2,
                    multisampled:   false,
                },
                count:      None,
            },
            BindGroupLayoutEntry {
                binding:    1,
                visibility: ShaderStages::FRAGMENT,
                ty:         BindingType::Sampler(SamplerBindingType::Comparison),
                count:      None,
            },
        ],
    })
}

fn create_sampler(device: &Device) -> Sampler {
    device.create_sampler(&SamplerDescriptor {
        label:          Some("rustforge.render.shadow.sampler"),
        // Clamp so out-of-frustum fragments sample the border instead
        // of wrapping into the wrong side of the shadow map. The WGSL
        // shader also short-circuits out-of-range UVs to "lit".
        address_mode_u: AddressMode::ClampToEdge,
        address_mode_v: AddressMode::ClampToEdge,
        address_mode_w: AddressMode::ClampToEdge,
        // Linear filter on a comparison sampler gives us 2×2 PCF for
        // free on every backend — a cheap softening of shadow edges.
        mag_filter:     FilterMode::Linear,
        min_filter:     FilterMode::Linear,
        mipmap_filter:  FilterMode::Nearest,
        // LessEqual: a fragment's depth ≤ the stored shadow depth
        // means the fragment is *at or in front of* the occluder from
        // the light's view, i.e. lit. Everything else is shadowed.
        compare:        Some(CompareFunction::LessEqual),
        ..Default::default()
    })
}

fn create_depth(device: &Device, resolution: u32) -> (Texture, TextureView) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("rustforge.render.shadow.depth"),
        size: Extent3d {
            width:                resolution,
            height:               resolution,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       TextureDimension::D2,
        format:          SHADOW_MAP_FORMAT,
        // Both: the shadow pass writes to it as an attachment, the
        // main pass samples it through the bind group.
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&TextureViewDescriptor::default());
    (texture, view)
}

fn create_bind_group(
    device: &Device,
    layout: &BindGroupLayout,
    view: &TextureView,
    sampler: &Sampler,
) -> BindGroup {
    device.create_bind_group(&BindGroupDescriptor {
        label:   Some("rustforge.render.shadow.bg"),
        layout,
        entries: &[
            BindGroupEntry {
                binding:  0,
                resource: BindingResource::TextureView(view),
            },
            BindGroupEntry {
                binding:  1,
                resource: BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn shadow_map_format_is_depth32_float() {
        assert_eq!(super::SHADOW_MAP_FORMAT, wgpu::TextureFormat::Depth32Float);
    }

    #[test]
    fn default_shadow_resolution_is_square_power_of_two() {
        let r = super::DEFAULT_SHADOW_RESOLUTION;
        assert!(r > 0);
        // Power-of-two keeps every backend's texture-allocation path on
        // the fast road and matches common shadow-atlas conventions.
        assert_eq!(r & (r - 1), 0, "shadow resolution {r} is not a power of two");
    }

    #[test]
    fn shadow_shader_declares_group2_bindings() {
        // Belt-and-braces: the fragment shader expects the shadow bind
        // group at `@group(2)` — if someone renumbers a group the
        // pipeline layout still matches the old number and silently
        // breaks on every backend that doesn't validate layout at
        // pipeline creation. A cheap string check catches the drift.
        let cube_wgsl = crate::shader::CUBE_WGSL;
        assert!(
            cube_wgsl.contains("@group(2) @binding(0) var t_shadow"),
            "cube.wgsl missing @group(2) @binding(0) shadow texture"
        );
        assert!(
            cube_wgsl.contains("@group(2) @binding(1) var s_shadow"),
            "cube.wgsl missing @group(2) @binding(1) shadow sampler"
        );
    }

    #[test]
    fn shadow_shader_has_light_view_proj_uniform() {
        // Vertex shader depends on reading `light_view_proj` from the
        // TransformUniform block — drop the field and the shader still
        // compiles but every fragment falls outside the shadow
        // frustum and renders fully lit, masking the regression.
        let cube_wgsl = crate::shader::CUBE_WGSL;
        assert!(
            cube_wgsl.contains("light_view_proj:"),
            "cube.wgsl no longer references `light_view_proj` in its uniform block",
        );
        let shadow_wgsl = crate::shader::SHADOW_WGSL;
        assert!(
            shadow_wgsl.contains("light_view_proj"),
            "shadow.wgsl no longer references `light_view_proj`",
        );
    }
}
