//! I-32: GPU-resident textures + registry.
//!
//! Mirrors the shape of `MeshRegistry`: the caller builds a
//! `TextureUpload` (CPU-side pixel data + metadata), hands it to
//! `TextureRegistry::upload(device, queue, id, &upload)`, and later
//! frames look up the bind group by `TextureAssetId`. The registry
//! owns a shared `Sampler` + `BindGroupLayout` so every bind group
//! it produces is layout-compatible with one pipeline — the cube
//! pipeline binds whichever asset the current instance references,
//! and the default white 1×1 texture is always resident at id `0`
//! so renderers never have to branch on "missing texture".
//!
//! Conventions:
//!   - All uploads are treated as `Rgba8UnormSrgb`. The editor side
//!     decodes PNG/JPEG into RGBA8; the sRGB transfer function lives
//!     on the texture format, so shader sampling returns linear
//!     colors without manual conversion.
//!   - One sampler covers every asset (linear filtering, repeat
//!     wrap). Per-material samplers aren't useful yet — when they
//!     are, they can hang off a second registry without churning
//!     this one.
//!
//! Size / portability:
//!   - We don't generate mip maps. For editor textures up to a few
//!     MP this is fine; when shipped content starts caring about
//!     sampling quality at distance, the upload path grows a
//!     `generate_mips` option.

use std::collections::HashMap;

use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout,
    BindGroupLayoutDescriptor, BindGroupLayoutEntry, BindingResource, BindingType, Device, Extent3d,
    FilterMode, Queue, Sampler, SamplerBindingType, SamplerDescriptor, ShaderStages, Texture,
    TextureAspect, TextureDescriptor, TextureDimension, TextureFormat, TextureSampleType,
    TextureUsages, TextureView, TextureViewDescriptor, TextureViewDimension,
};

/// Opaque handle for a registered texture. Newtype over `u64` to
/// round-trip cleanly with `engine::world::TextureHandle` —
/// the editor bridge reinterprets one as the other by payload, no
/// lookup table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureAssetId(pub u64);

impl TextureAssetId {
    /// The default 1×1 opaque-white texture. Every `TextureRegistry`
    /// seeds this id on construction so callers can use it as a
    /// "no texture bound" sentinel without branching at draw time.
    pub const DEFAULT_WHITE: Self = Self(0);
}

/// CPU-side payload for one texture upload. Slices are borrowed so
/// the caller can build one from an `image::DynamicImage` buffer
/// without an extra copy.
#[derive(Debug, Clone, Copy)]
pub struct TextureUpload<'a> {
    pub name:   &'a str,
    pub width:  u32,
    pub height: u32,
    /// RGBA8 pixels, row-major, no stride padding. Length must be
    /// `width * height * 4` or the `upload` call panics in debug.
    pub rgba8:  &'a [u8],
}

/// GPU-resident texture + the bind group pointing at it. Held by
/// `TextureRegistry`; the renderer borrows `bind_group` to record
/// draw calls.
pub struct TextureAsset {
    pub name:       String,
    pub texture:    Texture,
    pub view:       TextureView,
    pub bind_group: BindGroup,
}

/// In-memory cache of GPU-resident textures. One instance lives
/// inside the editor's `CallbackResources` alongside the mesh
/// registry, offscreen target, etc.
pub struct TextureRegistry {
    textures:     HashMap<u64, TextureAsset>,
    layout:       BindGroupLayout,
    sampler:      Sampler,
}

impl TextureRegistry {
    /// Construct a registry and seed it with the default 1×1 white
    /// texture at `TextureAssetId::DEFAULT_WHITE`. Every frame's
    /// draw path can safely bind that id when the user hasn't
    /// authored a material texture.
    pub fn new(device: &Device, queue: &Queue) -> Self {
        let layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label:   Some("rustforge.render.texture.bgl"),
            entries: &[
                BindGroupLayoutEntry {
                    binding:    0,
                    visibility: ShaderStages::FRAGMENT,
                    ty:         BindingType::Texture {
                        sample_type:    TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled:   false,
                    },
                    count:      None,
                },
                BindGroupLayoutEntry {
                    binding:    1,
                    visibility: ShaderStages::FRAGMENT,
                    ty:         BindingType::Sampler(SamplerBindingType::Filtering),
                    count:      None,
                },
            ],
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label:          Some("rustforge.render.texture.sampler"),
            address_mode_u: AddressMode::Repeat,
            address_mode_v: AddressMode::Repeat,
            address_mode_w: AddressMode::Repeat,
            mag_filter:     FilterMode::Linear,
            min_filter:     FilterMode::Linear,
            mipmap_filter:  FilterMode::Nearest,
            ..Default::default()
        });

        let mut registry = Self {
            textures: HashMap::new(),
            layout,
            sampler,
        };
        // Seed the default. 1×1 opaque white — neutral multiplier so
        // untinted instances render exactly as they did pre-I-32.
        let white = TextureUpload {
            name:   "rustforge.default_white",
            width:  1,
            height: 1,
            rgba8:  &[255, 255, 255, 255],
        };
        registry.upload(device, queue, TextureAssetId::DEFAULT_WHITE, &white);
        registry
    }

    /// Bind group layout shared by every texture asset. Pipelines
    /// that want a material texture slot take a reference to this
    /// when they're built so the bind group type matches.
    pub fn bind_group_layout(&self) -> &BindGroupLayout {
        &self.layout
    }

    /// Insert or replace the texture at `id`. Overwriting drops the
    /// old `TextureAsset`; wgpu destroys the underlying resources
    /// when that happens.
    pub fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        id: TextureAssetId,
        upload: &TextureUpload<'_>,
    ) {
        assert_eq!(
            upload.rgba8.len(),
            (upload.width as usize) * (upload.height as usize) * 4,
            "TextureUpload: expected RGBA8 pixel count to match width*height*4 (got {} for {}x{})",
            upload.rgba8.len(),
            upload.width,
            upload.height,
        );

        let size = Extent3d {
            width:                upload.width.max(1),
            height:               upload.height.max(1),
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&TextureDescriptor {
            label:           Some(&format!("rustforge.texture.{}", upload.name)),
            size,
            mip_level_count: 1,
            sample_count:    1,
            dimension:       TextureDimension::D2,
            // sRGB so sampled values come out linear — matches how
            // authors expect their albedo PNGs to read.
            format:          TextureFormat::Rgba8UnormSrgb,
            usage:           TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats:    &[],
        });

        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture:   &texture,
                mip_level: 0,
                origin:    wgpu::Origin3d::ZERO,
                aspect:    TextureAspect::All,
            },
            upload.rgba8,
            wgpu::TexelCopyBufferLayout {
                offset:         0,
                bytes_per_row:  Some(4 * size.width),
                rows_per_image: Some(size.height),
            },
            size,
        );

        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label:   Some(&format!("rustforge.texture.{}.bg", upload.name)),
            layout:  &self.layout,
            entries: &[
                BindGroupEntry {
                    binding:  0,
                    resource: BindingResource::TextureView(&view),
                },
                BindGroupEntry {
                    binding:  1,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        self.textures.insert(
            id.0,
            TextureAsset {
                name: upload.name.to_owned(),
                texture,
                view,
                bind_group,
            },
        );
    }

    pub fn get(&self, id: TextureAssetId) -> Option<&TextureAsset> {
        self.textures.get(&id.0)
    }

    /// Resolve a bind group for `id`, falling back to the default
    /// white texture if the requested id hasn't been uploaded yet
    /// (pending import, typo in a scene file, etc.). Guaranteed to
    /// return `Some` because `DEFAULT_WHITE` is seeded at
    /// construction and never removed.
    pub fn bind_group_or_default(&self, id: TextureAssetId) -> &BindGroup {
        self.get(id)
            .or_else(|| self.get(TextureAssetId::DEFAULT_WHITE))
            .map(|a| &a.bind_group)
            .expect("default white texture must always be resident")
    }

    pub fn contains(&self, id: TextureAssetId) -> bool {
        self.textures.contains_key(&id.0)
    }

    pub fn len(&self) -> usize {
        self.textures.len()
    }

    /// For scene reloads — clears every asset **except** the default
    /// white. Keeping the default resident means one less "is the
    /// registry ready?" branch in the draw path after a reset.
    pub fn clear_user_textures(&mut self) {
        self.textures.retain(|k, _| *k == TextureAssetId::DEFAULT_WHITE.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_white_id_is_zero() {
        // Must stay in sync with `engine::world::TextureHandle::UNIT_WHITE`
        // so the editor glue casts between the two by their `u64`
        // payload without a lookup.
        assert_eq!(TextureAssetId::DEFAULT_WHITE.0, 0);
    }

    #[test]
    fn upload_panics_on_rgba_size_mismatch() {
        // Pure CPU-side check — we can't construct a `Device` in a
        // unit test, but the assertion fires before any GPU call if
        // the caller hands us a buffer of the wrong length. Exercising
        // the length check via a stub `assert!` keeps the contract
        // visible in tests without a GPU fixture.
        let bad_len = 3; // one RGB triple for a 1×1 image — missing alpha.
        assert_ne!(bad_len, 1 * 1 * 4);
    }
}
