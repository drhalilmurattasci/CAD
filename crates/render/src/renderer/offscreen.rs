//! I-30: offscreen color + depth render target for the 3D viewport.
//!
//! The editor viewport paints through an egui_wgpu callback, which
//! hands us a `RenderPass` that writes to egui's framebuffer with no
//! depth attachment. Enabling Z-testing therefore needs its own
//! render target: we draw the scene into an offscreen color + depth
//! pair with depth enabled, then present the result by blitting the
//! color texture into the egui pass (see [`BlitRenderer`]).
//!
//! `OffscreenTarget` owns both textures, resizes them lazily when the
//! viewport dimensions change, and exposes the texture views the
//! render pass descriptor needs.
//!
//! Lifetimes:
//!   - The target lives inside egui_wgpu's `CallbackResources` so it
//!     persists across frames — no re-creating textures every paint.
//!   - Resize is cheap-ish (two `create_texture` calls) but we guard
//!     it behind a dimension check because egui emits identical
//!     sizes on idle frames and we don't want to thrash the GPU
//!     allocator.

use wgpu::{
    Device, Extent3d, Texture, TextureDescriptor, TextureDimension, TextureFormat, TextureUsages,
    TextureView, TextureViewDescriptor,
};

/// Depth format the render pipelines target. `Depth32Float` is
/// supported on every wgpu backend (native + WebGPU), sidesteps the
/// stencil plumbing we don't need, and gives ~7 decimal digits of
/// depth precision — more than enough for the editor's world scale.
pub const VIEWPORT_DEPTH_FORMAT: TextureFormat = TextureFormat::Depth32Float;

pub struct OffscreenTarget {
    color_format: TextureFormat,
    size:         (u32, u32),
    color:        Texture,
    color_view:   TextureView,
    depth:        Texture,
    depth_view:   TextureView,
}

impl OffscreenTarget {
    pub fn new(device: &Device, color_format: TextureFormat, width: u32, height: u32) -> Self {
        let size = (width.max(1), height.max(1));
        let (color, color_view) = create_color(device, color_format, size);
        let (depth, depth_view) = create_depth(device, size);
        Self {
            color_format,
            size,
            color,
            color_view,
            depth,
            depth_view,
        }
    }

    /// Resize if the requested dimensions differ from the current
    /// ones. Returns `true` when a resize actually happened so callers
    /// can invalidate downstream state (e.g. a bind group that sampled
    /// the old color texture).
    pub fn ensure_size(&mut self, device: &Device, width: u32, height: u32) -> bool {
        let target = (width.max(1), height.max(1));
        if target == self.size {
            return false;
        }
        self.size = target;
        let (color, color_view) = create_color(device, self.color_format, target);
        let (depth, depth_view) = create_depth(device, target);
        self.color = color;
        self.color_view = color_view;
        self.depth = depth;
        self.depth_view = depth_view;
        true
    }

    pub fn color_view(&self) -> &TextureView {
        &self.color_view
    }

    pub fn depth_view(&self) -> &TextureView {
        &self.depth_view
    }

    /// Texture handle — exposed so the blit pipeline can bind it as a
    /// sampled resource in its own bind group.
    pub fn color_texture(&self) -> &Texture {
        &self.color
    }

    /// Useful for diagnostics + tests.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Color format the viewport renders into. Callers building
    /// depth-aware pipelines must pass this so the pipeline's color
    /// target matches the attachment it eventually writes to.
    pub fn color_format(&self) -> TextureFormat {
        self.color_format
    }
}

fn create_color(
    device: &Device,
    format: TextureFormat,
    (width, height): (u32, u32),
) -> (Texture, TextureView) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("rustforge.render.offscreen.color"),
        size: Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       TextureDimension::D2,
        format,
        // RENDER_ATTACHMENT so the scene pass can draw into it;
        // TEXTURE_BINDING so the blit shader can sample it back.
        usage: TextureUsages::RENDER_ATTACHMENT | TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&TextureViewDescriptor::default());
    (texture, view)
}

fn create_depth(device: &Device, (width, height): (u32, u32)) -> (Texture, TextureView) {
    let texture = device.create_texture(&TextureDescriptor {
        label: Some("rustforge.render.offscreen.depth"),
        size: Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count:    1,
        dimension:       TextureDimension::D2,
        format:          VIEWPORT_DEPTH_FORMAT,
        usage:           TextureUsages::RENDER_ATTACHMENT,
        view_formats:    &[],
    });
    let view = texture.create_view(&TextureViewDescriptor::default());
    (texture, view)
}

#[cfg(test)]
mod tests {
    #[test]
    fn depth_format_is_depth32_float() {
        // Lock in the contract so pipelines that hard-code the format
        // stay in sync with the attachment.
        assert_eq!(
            super::VIEWPORT_DEPTH_FORMAT,
            wgpu::TextureFormat::Depth32Float
        );
    }
}
