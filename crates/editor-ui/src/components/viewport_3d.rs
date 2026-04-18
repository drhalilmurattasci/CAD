//! Editor-side adapter that wires `render` renderers into an
//! `egui_wgpu::Callback`. The renderers themselves are pure engine code
//! (`render` crate) and know nothing about egui; this file
//! translates the egui paint stream into a wgpu render pass that the
//! renderers draw into.
//!
//! Evolution:
//!  - I-1: fullscreen-fill clear shader (no vertex buffer).
//!  - I-2: VBO-driven triangle.
//!  - I-2.1: extracted reusable primitives into the render crate.
//!  - I-3: rotating 3D cube with MVP uniform + back-face culling.
//!  - I-4: cube(s) driven by `engine::world::World` — one
//!    per-instance uniform per ECS entity, dispatched via dynamic
//!    offset into a shared uniform buffer.

use eframe::egui;
use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;

use engine::world::{DirectionalLight as EcsLight, RenderEntity};
use render::camera::{
    directional_light_view_proj, Camera, DirectionalLight, OrbitCamera, TransformUniform,
};
use render::mesh::{MeshAssetId, MeshRegistry, MeshUpload};
use render::renderer::{
    BlitRenderer, CubeRenderer, GridRenderer, MeshInstanceRenderer, OffscreenTarget,
    ShadowMapTarget, VIEWPORT_DEPTH_FORMAT,
};
use render::surface;
use render::texture::{TextureAssetId, TextureRegistry, TextureUpload};

/// Called once at editor start-up when eframe has negotiated a wgpu
/// surface format.
pub fn install_target_format(format: wgpu::TextureFormat) {
    surface::install_target_format(format);
}

/// I-22: a mesh scheduled for GPU upload this frame. The renderer's
/// `MeshRegistry` lives in `CallbackResources` and persists across
/// frames, so uploads only need to flow when the caller actually
/// imported or changed something — a steady-state frame has an empty
/// `mesh_uploads` vec.
#[derive(Debug, Clone)]
pub struct PendingMeshUpload {
    pub id:        MeshAssetId,
    pub name:      String,
    pub positions: Vec<[f32; 3]>,
    pub normals:   Vec<[f32; 3]>,
    pub indices:   Vec<u32>,
}

/// I-22: a single mesh-asset draw this frame. Complement to the cube
/// `entities` list — entities whose mesh handle points at a registry
/// asset land here, everything else stays on the cube fast path.
#[derive(Debug, Clone, Copy)]
pub struct MeshDraw {
    pub mesh:   MeshAssetId,
    pub model:  glam::Mat4,
    /// I-31: per-entity albedo tint, mirroring `RenderEntity::albedo`
    /// so mesh-registry draws respect the same material overrides as
    /// cube draws. Defaults to identity white when the caller has no
    /// material override to apply.
    pub albedo: [f32; 4],
    /// I-32: per-entity albedo texture. `TextureAssetId::DEFAULT_WHITE`
    /// routes to the 1×1 white texture the registry seeds on startup,
    /// which multiplies to 1.0 and leaves the tint unchanged.
    pub albedo_texture: TextureAssetId,
}

/// I-32: a texture scheduled for GPU upload this frame, sibling of
/// `PendingMeshUpload`. `TextureRegistry` lives in CallbackResources
/// and persists — uploads only flow on import.
#[derive(Debug, Clone)]
pub struct PendingTextureUpload {
    pub id:     TextureAssetId,
    pub name:   String,
    pub width:  u32,
    pub height: u32,
    /// RGBA8 pixels (length = width * height * 4). The bridge decodes
    /// PNG/JPEG into this layout before enqueuing so the render side
    /// stays format-agnostic.
    pub rgba8:  Vec<u8>,
}

/// Per-frame callback carrying everything `prepare` needs: viewport
/// size (for aspect ratio), the orbit camera that frames the scene,
/// one `RenderEntity` per cube + one `MeshDraw` per registry-resident
/// mesh instance, and any pending mesh uploads.
pub struct ViewportCallback {
    pub viewport_size_px: [u32; 2],
    pub camera:           OrbitCamera,
    pub entities:         Vec<RenderEntity>,
    /// Snapshot of the primary directional light pulled from the ECS
    /// this frame (I-13). `None` falls back to the renderer's default
    /// warm key light so newly-created scenes don't render unlit.
    pub light:            Option<EcsLight>,
    /// I-22: mesh draws through the registry path. Cube entities
    /// still go via `entities`; only non-cube meshes land here.
    pub mesh_draws:       Vec<MeshDraw>,
    /// I-22: mesh asset uploads to process during `prepare`. Typical
    /// frames carry an empty vec — uploads only fire on import.
    pub mesh_uploads:     Vec<PendingMeshUpload>,
    /// I-32: texture asset uploads for this frame. Same shape as
    /// `mesh_uploads` — steady-state frames are empty.
    pub texture_uploads:  Vec<PendingTextureUpload>,
    /// I-25: override the OrbitCamera-derived POV with a pre-resolved
    /// `Camera`. Set by the viewport panel when Play mode is active
    /// *and* the ECS carries a primary `Camera` component. `None` in
    /// Edit mode so the orbit camera keeps its usual role.
    pub camera_override:  Option<Camera>,
}

impl ViewportCallback {
    pub fn for_this_frame(
        rect_size_px: [u32; 2],
        camera: OrbitCamera,
        entities: Vec<RenderEntity>,
        light: Option<EcsLight>,
    ) -> Self {
        Self {
            viewport_size_px: rect_size_px,
            camera,
            entities,
            light,
            mesh_draws: Vec::new(),
            mesh_uploads: Vec::new(),
            texture_uploads: Vec::new(),
            camera_override: None,
        }
    }

    /// Attach a list of registry-mesh draws for this frame. Returns
    /// `self` for one-line builder use from the call site.
    pub fn with_mesh_draws(mut self, draws: Vec<MeshDraw>) -> Self {
        self.mesh_draws = draws;
        self
    }

    /// Attach mesh uploads to process during `prepare`.
    pub fn with_mesh_uploads(mut self, uploads: Vec<PendingMeshUpload>) -> Self {
        self.mesh_uploads = uploads;
        self
    }

    /// I-32: attach texture uploads to process during `prepare`.
    pub fn with_texture_uploads(mut self, uploads: Vec<PendingTextureUpload>) -> Self {
        self.texture_uploads = uploads;
        self
    }

    /// I-25: supply a pre-resolved gameplay camera. When set, the
    /// orbit camera still ships in (for tests/diagnostics) but the
    /// view-projection ignores it in favour of `override`.
    pub fn with_camera_override(mut self, override_camera: Option<Camera>) -> Self {
        self.camera_override = override_camera;
        self
    }
}

fn ecs_light_to_render(light: Option<EcsLight>) -> DirectionalLight {
    match light {
        Some(l) => DirectionalLight {
            direction: l.direction,
            color:     l.color,
            intensity: l.intensity,
            ambient:   l.ambient,
        },
        None => DirectionalLight::default(),
    }
}

impl CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        // I-30: renderers are constructed with depth enabled so they
        // match the offscreen target's depth attachment. Grid tests
        // depth but doesn't write — see `GridRenderer::new_with_depth`.
        // I-32: the `TextureRegistry` has to exist *before* the mesh
        // pipelines because the pipeline layout needs the registry's
        // bind-group-layout as group 1. Seed it with the default
        // 1×1 white texture so every draw path has something safe to
        // bind even before the first user texture import lands.
        let color_format = surface::target_format();
        if resources.get::<TextureRegistry>().is_none() {
            resources.insert(TextureRegistry::new(device, queue));
        }
        // I-33: seed the shadow map target before the main pipelines
        // so they can declare a group-2 layout against it. Shares the
        // egui_wgpu resources lifetime with everything else — persists
        // across frames, only its contents change.
        if resources.get::<ShadowMapTarget>().is_none() {
            resources.insert(ShadowMapTarget::new(device));
        }
        if resources.get::<CubeRenderer>().is_none() {
            // Build the renderer while the registry + shadow borrows
            // are live. Both are pushed into the pipeline layout —
            // dropped at end of statement.
            let texture_layout = resources
                .get::<TextureRegistry>()
                .expect("TextureRegistry seeded above")
                .bind_group_layout();
            let shadow_layout = resources
                .get::<ShadowMapTarget>()
                .expect("ShadowMapTarget seeded above")
                .bind_group_layout();
            let cube = CubeRenderer::new_with_depth(
                device,
                color_format,
                Some(VIEWPORT_DEPTH_FORMAT),
                Some(texture_layout),
                Some(shadow_layout),
            );
            resources.insert(cube);
        }
        if resources.get::<GridRenderer>().is_none() {
            resources.insert(GridRenderer::new_with_depth(
                device,
                color_format,
                Some(VIEWPORT_DEPTH_FORMAT),
            ));
        }
        if resources.get::<MeshInstanceRenderer>().is_none() {
            let texture_layout = resources
                .get::<TextureRegistry>()
                .expect("TextureRegistry seeded above")
                .bind_group_layout();
            let shadow_layout = resources
                .get::<ShadowMapTarget>()
                .expect("ShadowMapTarget seeded above")
                .bind_group_layout();
            let meshes = MeshInstanceRenderer::new_with_depth(
                device,
                color_format,
                Some(VIEWPORT_DEPTH_FORMAT),
                Some(texture_layout),
                Some(shadow_layout),
            );
            resources.insert(meshes);
        }
        if resources.get::<MeshRegistry>().is_none() {
            resources.insert(MeshRegistry::new());
        }
        // I-30: offscreen color+depth target the scene pass writes to,
        // plus the blit that copies it into the egui pass during paint.
        let [vw, vh] = self.viewport_size_px;
        if resources.get::<OffscreenTarget>().is_none() {
            resources.insert(OffscreenTarget::new(device, color_format, vw, vh));
        }
        if resources.get::<BlitRenderer>().is_none() {
            resources.insert(BlitRenderer::new(device, color_format));
        }

        // Process pending uploads before any instance packing — if
        // the same frame uploads and draws the same mesh, the draw
        // must see the resident asset.
        if !self.mesh_uploads.is_empty() {
            if let Some(registry) = resources.get_mut::<MeshRegistry>() {
                for pending in &self.mesh_uploads {
                    let upload = MeshUpload {
                        name:      &pending.name,
                        positions: &pending.positions,
                        normals:   &pending.normals,
                        indices:   &pending.indices,
                    };
                    registry.upload(device, pending.id, &upload);
                }
            }
        }
        // I-32: same pattern for texture uploads. `TextureRegistry`
        // persists so the steady-state frame only carries an empty
        // list and bypasses the lock entirely.
        if !self.texture_uploads.is_empty() {
            if let Some(registry) = resources.get_mut::<TextureRegistry>() {
                for pending in &self.texture_uploads {
                    let upload = TextureUpload {
                        name:   &pending.name,
                        width:  pending.width,
                        height: pending.height,
                        rgba8:  &pending.rgba8,
                    };
                    registry.upload(device, queue, pending.id, &upload);
                }
            }
        }

        // I-30: resize the offscreen target if the viewport changed.
        // `ensure_size` returns true on resize so we know to refresh
        // the blit bind group before it samples a dangling view.
        let resized = resources
            .get_mut::<OffscreenTarget>()
            .map(|t| t.ensure_size(device, vw, vh))
            .unwrap_or(false);

        let aspect = {
            let [w, h] = self.viewport_size_px;
            if h == 0 { 1.0 } else { w as f32 / h.max(1) as f32 }
        };
        // I-7: orbit camera comes from the editor shell state (mouse
        // yaw/pitch/zoom/pan) instead of the fixed I-3 debug view.
        // I-25: in Play mode with a primary gameplay camera in the
        // ECS, the viewport panel passes a `camera_override` pointing
        // at the gameplay POV; the orbit camera is bypassed for this
        // frame.
        let camera = self
            .camera_override
            .unwrap_or_else(|| self.camera.to_camera());
        let view_proj = camera.view_proj(aspect);

        // One `(TransformUniform, TextureAssetId)` per ECS entity.
        // view_proj + light are shared across instances; model matrix
        // comes from the ECS Transform (pre-composed in
        // `World::collect_render_snapshot`). I-32: the texture id is
        // carried alongside the uniform so the draw pass can rebind
        // group 1 per instance.
        let light = ecs_light_to_render(self.light);
        // I-33: one light-space clip matrix per frame — every
        // instance bakes it into its `TransformUniform` so the shadow
        // pass's vertex shader and the main pass's FS agree on
        // exactly one projection. Center on the orbit camera's pivot
        // so the shadow frustum tracks wherever the user is looking;
        // half-extent 20 matches the editor grid's half-extent so
        // anything visible on the grid also casts/receives shadows.
        let light_view_proj = directional_light_view_proj(
            light.direction,
            self.camera.target,
            20.0,
        );
        let cube_uploads: Vec<(TransformUniform, TextureAssetId)> = self
            .entities
            .iter()
            .map(|e| {
                // I-31/I-33: thread the per-entity albedo AND the
                // shared light-space matrix through so the shadow
                // pass + main pass both have what they need.
                (
                    TransformUniform::with_shadow(
                        view_proj,
                        e.model,
                        light_view_proj,
                        light,
                        e.albedo,
                    ),
                    // The core `TextureHandle` and the render-side
                    // `TextureAssetId` share a u64 payload by design
                    // (see the comment on TextureAssetId). Bit-cast
                    // through `.0` — no lookup table.
                    TextureAssetId(e.albedo_texture.0),
                )
            })
            .collect();
        if let Some(cubes) = resources.get::<CubeRenderer>() {
            cubes.upload_instances(queue, &cube_uploads);
        }

        // I-22: pack registry mesh instances alongside the cube
        // instances. Same TransformUniform layout — different VBO
        // bound at draw time. I-32 extends the tuple with the
        // per-instance texture id; I-33 bakes the shared
        // `light_view_proj` into every uniform.
        if let Some(mesh_renderer) = resources.get::<MeshInstanceRenderer>() {
            let mesh_uniforms: Vec<(MeshAssetId, TransformUniform, TextureAssetId)> = self
                .mesh_draws
                .iter()
                .map(|d| {
                    (
                        d.mesh,
                        TransformUniform::with_shadow(
                            view_proj, d.model, light_view_proj, light, d.albedo,
                        ),
                        d.albedo_texture,
                    )
                })
                .collect();
            mesh_renderer.upload_instances(queue, &mesh_uniforms);
        }

        // Grid shares the same view-projection — its mesh is already
        // authored in world space so no per-frame model matrix needed.
        if let Some(grid) = resources.get::<GridRenderer>() {
            grid.upload_view_proj(queue, view_proj);
        }

        // I-33: shadow pass runs *before* the scene pass — the main
        // shader samples the shadow map at fragment time, so the
        // depth texture has to be authoritative by the time the scene
        // pass starts. Depth-only, no color attachment, clears to 1.0
        // (farthest) so unwritten texels read as "lit".
        if let Some(shadow_target) = resources.get::<ShadowMapTarget>() {
            let mut shadow_pass =
                egui_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label:                    Some("rustforge.viewport.shadow_pass"),
                    color_attachments:        &[],
                    depth_stencil_attachment: Some(
                        wgpu::RenderPassDepthStencilAttachment {
                            view:        shadow_target.depth_view(),
                            depth_ops:   Some(wgpu::Operations {
                                load:  wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        },
                    ),
                    timestamp_writes:         None,
                    occlusion_query_set:      None,
                });
            // Cubes first, then mesh registry — draw order in the
            // shadow pass doesn't matter for correctness, but keeping
            // it identical to the scene pass simplifies reasoning
            // when debugging a single renderer's shadow output.
            if let Some(cubes) = resources.get::<CubeRenderer>() {
                cubes.draw_shadow(&mut shadow_pass);
            }
            if let (Some(meshes), Some(registry)) = (
                resources.get::<MeshInstanceRenderer>(),
                resources.get::<MeshRegistry>(),
            ) {
                meshes.draw_shadow(&mut shadow_pass, registry);
            }
            // `shadow_pass` drops here, flushing into `egui_encoder`.
        }

        // I-30: encode the depth-aware scene pass into egui's command
        // encoder. We record here rather than returning a separate
        // `CommandBuffer` so the submission ordering with egui's own
        // work is preserved (egui does not guarantee a specific order
        // between returned command buffers and its own encoder — see
        // `egui_wgpu::Callback` docs). The blit runs during `paint`
        // into the pass egui hands us.
        if let Some(target) = resources.get::<OffscreenTarget>() {
            let color_view = target.color_view();
            let depth_view = target.depth_view();
            let mut scene_pass =
                egui_encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label:                    Some("rustforge.viewport.scene_pass"),
                    color_attachments:        &[Some(wgpu::RenderPassColorAttachment {
                        view:           color_view,
                        resolve_target: None,
                        // `depth_slice` is only meaningful for 3D /
                        // array textures; our offscreen color is a
                        // single 2D slice, so None selects the only
                        // layer.
                        depth_slice:    None,
                        ops:            wgpu::Operations {
                            load:  wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.05,
                                g: 0.06,
                                b: 0.08,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(
                        wgpu::RenderPassDepthStencilAttachment {
                            view:        depth_view,
                            depth_ops:   Some(wgpu::Operations {
                                load:  wgpu::LoadOp::Clear(1.0),
                                store: wgpu::StoreOp::Store,
                            }),
                            stencil_ops: None,
                        },
                    ),
                    timestamp_writes:         None,
                    occlusion_query_set:      None,
                });
            // Draw order mirrors the old depth-less setup: grid first
            // so transparent-ish axis lines sit under everything in
            // the same frame, then solid meshes which write depth and
            // resolve via CompareFunction::Less.
            if let Some(grid) = resources.get::<GridRenderer>() {
                grid.draw(&mut scene_pass);
            }
            // I-33: resolve the shadow bind group once and hand the
            // same reference to both solid-geometry renderers — the
            // shadow map doesn't change mid-frame.
            let shadow_bg = resources.get::<ShadowMapTarget>().map(|s| s.bind_group());
            if let (Some(cubes), Some(textures)) = (
                resources.get::<CubeRenderer>(),
                resources.get::<TextureRegistry>(),
            ) {
                cubes.draw(&mut scene_pass, textures, shadow_bg);
            }
            if let (Some(meshes), Some(registry), Some(textures)) = (
                resources.get::<MeshInstanceRenderer>(),
                resources.get::<MeshRegistry>(),
                resources.get::<TextureRegistry>(),
            ) {
                meshes.draw(&mut scene_pass, registry, textures, shadow_bg);
            }
            // `scene_pass` drops here, finalizing the render pass into
            // `egui_encoder`. Subsequent egui draws in the same
            // encoder see the offscreen results as a plain sampled
            // texture via the blit bind group.
        }

        // I-30: point the blit at the (possibly just-recreated)
        // offscreen color view. `TextureView` is `Arc`-backed inside
        // wgpu so cloning it is cheap and sidesteps the need to hold
        // two live borrows of `CallbackResources` at once.
        let source_view = resources
            .get::<OffscreenTarget>()
            .map(|t| t.color_view().clone());
        if let (Some(source), Some(blit)) =
            (source_view, resources.get_mut::<BlitRenderer>())
        {
            // `resized` is true on every size change (including the
            // first frame, when the target promotes from its 1×1
            // placeholder to the real viewport size), so the bind
            // group is rebuilt exactly when the underlying view has
            // been replaced.
            blit.ensure_source(device, &source, resized);
        }

        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        resources: &CallbackResources,
    ) {
        // I-30: the real scene draw already happened inside `prepare`
        // (into a depth-aware offscreen pass). Paint is now a single
        // fullscreen blit that copies the offscreen color texture
        // into egui's pass, so all depth-correct compositing survives
        // into the final framebuffer.
        if let Some(blit) = resources.get::<BlitRenderer>() {
            blit.draw(render_pass);
        }
    }
}

/// Installs the viewport paint callback for the given screen-space
/// `rect`, rendering one cube per `RenderEntity` in `snapshot` through
/// the supplied `OrbitCamera`.
pub fn paint(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    camera: OrbitCamera,
    snapshot: Vec<RenderEntity>,
    light: Option<EcsLight>,
) {
    paint_with_meshes(
        ui,
        rect,
        camera,
        snapshot,
        light,
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
}

/// I-22: same as `paint` but also ships registry-mesh draws and any
/// pending uploads for this frame. Cube-only call sites keep using
/// `paint`; call sites that import glTF assets go through this.
/// I-32 extends with `texture_uploads` — a trailing positional Vec
/// rather than a builder call so Play-mode and thumbnail sites that
/// already pass explicit args don't need new intermediate types.
pub fn paint_with_meshes(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    camera: OrbitCamera,
    snapshot: Vec<RenderEntity>,
    light: Option<EcsLight>,
    mesh_draws: Vec<MeshDraw>,
    mesh_uploads: Vec<PendingMeshUpload>,
    texture_uploads: Vec<PendingTextureUpload>,
) {
    paint_with_camera_override(
        ui,
        rect,
        camera,
        None,
        snapshot,
        light,
        mesh_draws,
        mesh_uploads,
        texture_uploads,
    );
}

/// I-25: superset of `paint_with_meshes` that also accepts a
/// `camera_override`. When `Some`, the viewport renders from that
/// `Camera` instead of the orbit camera (gameplay POV). When `None`
/// it behaves identically to `paint_with_meshes`.
///
/// Exposed as a separate function rather than a 9th positional
/// parameter so existing call sites don't have to sprinkle `None`s
/// into their paint invocations.
pub fn paint_with_camera_override(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    camera: OrbitCamera,
    camera_override: Option<Camera>,
    snapshot: Vec<RenderEntity>,
    light: Option<EcsLight>,
    mesh_draws: Vec<MeshDraw>,
    mesh_uploads: Vec<PendingMeshUpload>,
    texture_uploads: Vec<PendingTextureUpload>,
) {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let size_px = [
        (rect.width() * pixels_per_point).round().max(1.0) as u32,
        (rect.height() * pixels_per_point).round().max(1.0) as u32,
    ];
    let callback = ViewportCallback::for_this_frame(size_px, camera, snapshot, light)
        .with_mesh_draws(mesh_draws)
        .with_mesh_uploads(mesh_uploads)
        .with_texture_uploads(texture_uploads)
        .with_camera_override(camera_override);
    ui.painter()
        .add(egui_wgpu::Callback::new_paint_callback(rect, callback));
}
