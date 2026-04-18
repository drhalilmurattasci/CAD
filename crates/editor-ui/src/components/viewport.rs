use eframe::egui;
use glam::{Mat4, Vec3, Vec4};
use engine::events::PlayModeState;
use engine::picking::{
    angle_on_ring, pick_gizmo, pick_rotate_handle, pick_scale_handle, ray_plane_hit, GizmoAxis,
    GizmoHandle, GizmoLayout, GizmoMode, Ray,
};
use engine::scene::{PrimitiveValue, SceneDocument, SceneId};
use render::camera::OrbitCamera;

use crate::components::{viewport_3d, WorkspaceAction};
use crate::shell::{RuntimeStats, ViewportCameraState, ViewportState};
use crate::viewport_bridge::ViewportBridge;

pub fn render(
    ui: &mut egui::Ui,
    scene: &SceneDocument,
    selected: Option<SceneId>,
    runtime: &RuntimeStats,
    viewport_bridge: &mut ViewportBridge,
    viewport_state: &mut ViewportState,
    // I-25: current play state. `Playing` with a primary gameplay
    // camera in the ECS swaps the orbit camera out for the gameplay
    // POV. `Editing` / `Paused` keep the orbit camera so the user can
    // still fly around.
    play_mode: PlayModeState,
) -> Vec<WorkspaceAction> {
    let mut actions = Vec::new();
    ui.horizontal(|ui| {
        ui.heading("Viewport");
        ui.separator();
        ui.label(format!("Scene: {}", scene.name));
        if let Some(selected_id) = selected {
            ui.separator();
            ui.label(format!("Selected: {}", selected_id));
        }
        ui.separator();
        ui.monospace(format!("Zoom {:.2}", viewport_state.camera.zoom));
        ui.separator();
        // I-34: gizmo-mode toolbar. Each button is a radio selector —
        // click to switch modes, highlighted when active. Keyboard
        // shortcuts W/E/R mirror Unity; we consume the keypress only
        // if the viewport has focus so typing in the inspector or
        // hierarchy doesn't inadvertently flip the gizmo mode.
        ui.label("Gizmo:");
        let mode = &mut viewport_state.gizmo_mode;
        if ui
            .selectable_label(*mode == GizmoMode::Translate, "Move (W)")
            .clicked()
        {
            *mode = GizmoMode::Translate;
        }
        if ui
            .selectable_label(*mode == GizmoMode::Rotate, "Rotate (E)")
            .clicked()
        {
            *mode = GizmoMode::Rotate;
        }
        if ui
            .selectable_label(*mode == GizmoMode::Scale, "Scale (R)")
            .clicked()
        {
            *mode = GizmoMode::Scale;
        }
    });
    // Keyboard hotkeys — only when no widget is holding keyboard focus,
    // so typing "w" inside the inspector name field or the content-
    // browser search box doesn't inadvertently flip the gizmo mode.
    let keyboard_is_free = ui.memory(|m| m.focused().is_none());
    if keyboard_is_free {
        ui.input(|input| {
            if input.key_pressed(egui::Key::W) {
                viewport_state.gizmo_mode = GizmoMode::Translate;
            }
            if input.key_pressed(egui::Key::E) {
                viewport_state.gizmo_mode = GizmoMode::Rotate;
            }
            if input.key_pressed(egui::Key::R) {
                viewport_state.gizmo_mode = GizmoMode::Scale;
            }
        });
    }
    ui.separator();

    ui.group(|ui| {
        ui.horizontal(|ui| {
            ui.strong("Camera");
            if ui.button("Reset").clicked() {
                viewport_state.camera = Default::default();
            }
            if ui.button("Frame Selection").clicked() {
                viewport_state.camera.pan_x = 0.0;
                viewport_state.camera.pan_y = 0.0;
                viewport_state.camera.zoom = 1.2;
            }
            ui.separator();
            ui.label(format!(
                "pan {:.1}, {:.1} | orbit {:.1}, {:.1}",
                viewport_state.camera.pan_x,
                viewport_state.camera.pan_y,
                viewport_state.camera.orbit_yaw,
                viewport_state.camera.orbit_pitch
            ));
        });
    });
    ui.add_space(8.0);

    if let Some(selected_id) = selected
        && let Some(entity) = scene.find_entity(selected_id)
    {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.strong("Viewport Gizmo");
                ui.separator();
                ui.label(&entity.name);
                if let Some((x, y, z)) = transform_triplet(entity) {
                    ui.separator();
                    ui.monospace(format!("x {x:.1}  y {y:.1}  z {z:.1}"));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Left").clicked() {
                    actions.push(WorkspaceAction::NudgeTransform {
                        entity_id: selected_id,
                        dx: -1.0,
                        dy: 0.0,
                        dz: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Right").clicked() {
                    actions.push(WorkspaceAction::NudgeTransform {
                        entity_id: selected_id,
                        dx: 1.0,
                        dy: 0.0,
                        dz: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Up").clicked() {
                    actions.push(WorkspaceAction::NudgeTransform {
                        entity_id: selected_id,
                        dx: 0.0,
                        dy: 1.0,
                        dz: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Down").clicked() {
                    actions.push(WorkspaceAction::NudgeTransform {
                        entity_id: selected_id,
                        dx: 0.0,
                        dy: -1.0,
                        dz: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Forward").clicked() {
                    actions.push(WorkspaceAction::NudgeTransform {
                        entity_id: selected_id,
                        dx: 0.0,
                        dy: 0.0,
                        dz: 1.0,
                        source: "viewport",
                    });
                }
                if ui.button("Back").clicked() {
                    actions.push(WorkspaceAction::NudgeTransform {
                        entity_id: selected_id,
                        dx: 0.0,
                        dy: 0.0,
                        dz: -1.0,
                        source: "viewport",
                    });
                }
            });
            // I-16: rotate gizmo buttons. Each press is a fixed 15°
            // step around one Euler axis; the command layer snapshots
            // previous values so undo/redo land exactly back on the
            // original orientation without float drift.
            ui.horizontal(|ui| {
                let step = std::f64::consts::FRAC_PI_2 / 6.0; // 15°
                ui.label("Rotate:");
                if ui.button("X-").clicked() {
                    actions.push(WorkspaceAction::RotateTransform {
                        entity_id: selected_id,
                        d_rot_x: -step,
                        d_rot_y: 0.0,
                        d_rot_z: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("X+").clicked() {
                    actions.push(WorkspaceAction::RotateTransform {
                        entity_id: selected_id,
                        d_rot_x: step,
                        d_rot_y: 0.0,
                        d_rot_z: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Y-").clicked() {
                    actions.push(WorkspaceAction::RotateTransform {
                        entity_id: selected_id,
                        d_rot_x: 0.0,
                        d_rot_y: -step,
                        d_rot_z: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Y+").clicked() {
                    actions.push(WorkspaceAction::RotateTransform {
                        entity_id: selected_id,
                        d_rot_x: 0.0,
                        d_rot_y: step,
                        d_rot_z: 0.0,
                        source: "viewport",
                    });
                }
                if ui.button("Z-").clicked() {
                    actions.push(WorkspaceAction::RotateTransform {
                        entity_id: selected_id,
                        d_rot_x: 0.0,
                        d_rot_y: 0.0,
                        d_rot_z: -step,
                        source: "viewport",
                    });
                }
                if ui.button("Z+").clicked() {
                    actions.push(WorkspaceAction::RotateTransform {
                        entity_id: selected_id,
                        d_rot_x: 0.0,
                        d_rot_y: 0.0,
                        d_rot_z: step,
                        source: "viewport",
                    });
                }
            });
            // I-17: uniform scale buttons. 1.1 / (1/1.1) are exact
            // inverses, so round-tripping ×↑ then ×↓ is a no-op
            // through undo even without the snapshot — but the
            // snapshot guards against accumulated FP drift anyway.
            ui.horizontal(|ui| {
                ui.label("Scale:");
                if ui.button("×1.1").clicked() {
                    actions.push(WorkspaceAction::ScaleTransform {
                        entity_id: selected_id,
                        factor: 1.1,
                        source: "viewport",
                    });
                }
                if ui.button("÷1.1").clicked() {
                    actions.push(WorkspaceAction::ScaleTransform {
                        entity_id: selected_id,
                        factor: 1.0 / 1.1,
                        source: "viewport",
                    });
                }
                if ui.button("×2").clicked() {
                    actions.push(WorkspaceAction::ScaleTransform {
                        entity_id: selected_id,
                        factor: 2.0,
                        source: "viewport",
                    });
                }
                if ui.button("÷2").clicked() {
                    actions.push(WorkspaceAction::ScaleTransform {
                        entity_id: selected_id,
                        factor: 0.5,
                        source: "viewport",
                    });
                }
            });
        });
        ui.add_space(8.0);
    }

    let available = ui.available_size();
    let desired = egui::Vec2::new(available.x.max(320.0), available.y.max(220.0));
    let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
    let width = desired.x.max(1.0).round() as u32;
    let height = desired.y.max(1.0).round() as u32;
    let snapshot = viewport_bridge.render_scene(scene, width, height, 1.0 / runtime.fps.max(1.0));

    if response.hovered() {
        let scroll = ui.input(|input| input.raw_scroll_delta.y);
        if scroll.abs() > f32::EPSILON {
            viewport_state.camera.zoom = (viewport_state.camera.zoom + scroll * 0.0015).clamp(0.3, 3.0);
        }
    }

    if response.dragged_by(egui::PointerButton::Middle) {
        let delta = response.drag_delta();
        viewport_state.camera.pan_x += delta.x * 0.02;
        viewport_state.camera.pan_y += delta.y * 0.02;
    }

    if response.dragged_by(egui::PointerButton::Secondary) {
        let delta = response.drag_delta();
        viewport_state.camera.orbit_yaw += delta.x * 0.05;
        viewport_state.camera.orbit_pitch =
            (viewport_state.camera.orbit_pitch + delta.y * 0.05).clamp(-89.0, 89.0);
    }

    // I-1: real wgpu render pass fills the viewport rect.
    // I-4: the callback draws one cube per entity in the runtime
    // world. Snapshotting happens on the main thread so the paint
    // callback stays lifetime-free.
    // I-7: build the orbit camera from the editor shell's camera
    // state (yaw/pitch/zoom/pan) so mouse drags in the viewport
    // actually rotate the view.
    let camera = orbit_camera_from_state(&viewport_state.camera);
    // I-9: render gizmo handles for the selected entity alongside
    // the world cubes.
    // I-27: split the snapshot into cube entities vs. imported-mesh
    // instances — the cube pipeline takes the former, the
    // MeshInstanceRenderer takes the latter. Gizmo handles are baked
    // into the cube list by `split_render_snapshot` (they share the
    // unit cube mesh).
    let (world_snapshot, mesh_draws) =
        viewport_bridge.split_render_snapshot_for_mode(selected, viewport_state.gizmo_mode);
    // Drain any glTF uploads queued since the last paint. Steady-state
    // frames return an empty vec; only an import-triggering rebuild
    // surfaces pending data here.
    let mesh_uploads = viewport_bridge.take_pending_mesh_uploads();
    // I-32: drain texture uploads the same way. Again, only non-empty
    // on import-triggering frames (new Material with albedo_texture,
    // or scene switch).
    let texture_uploads = viewport_bridge.take_pending_texture_uploads();
    // I-13: pull the primary directional light out of the runtime
    // world so the lit shader uses authored values.
    let light = viewport_bridge.world().primary_directional_light();
    // I-25: in Play mode, swap the orbit POV for the ECS's primary
    // gameplay camera — if one exists. Edit/Paused always use orbit
    // so the designer can frame shots while the scene is frozen.
    //
    // `effective_camera` is the *actual* POV the GPU renders from;
    // it's what picking / gizmo math has to match or clicks land
    // at the wrong world-space location.
    let camera_override = if matches!(play_mode, PlayModeState::Playing) {
        viewport_bridge.primary_gameplay_camera()
    } else {
        None
    };
    let effective_camera = camera_override.unwrap_or_else(|| camera.to_camera());
    viewport_3d::paint_with_camera_override(
        ui,
        rect,
        camera,
        camera_override,
        world_snapshot,
        light,
        mesh_draws,
        mesh_uploads,
        texture_uploads,
    );
    ui.painter().rect_stroke(
        rect,
        12.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(80, 120, 170)),
        egui::StrokeKind::Outside,
    );

    // Camera-control hint sits top-left as an unobtrusive overlay —
    // this one is useful and stays. The `Attachment #N` center text
    // and the row of `Entity N` slot badges that used to draw here
    // were pre-real-render debug scaffolding from when the viewport
    // was a stub; now that the wgpu pipeline actually renders the
    // cubes/meshes, those 2-D overlays just occluded the 3-D scene.
    let camera_note = egui::pos2(rect.left() + 16.0, rect.top() + 14.0);
    ui.painter().text(
        camera_note,
        egui::Align2::LEFT_TOP,
        format!(
            "MMB pan | RMB orbit | Wheel zoom | zoom {:.2}",
            viewport_state.camera.zoom
        ),
        egui::FontId::monospace(12.0),
        egui::Color32::from_rgb(166, 194, 230),
    );

    if let Some(selected_id) = selected {
        // Compute the view-projection + its inverse once per frame —
        // used by gizmo hit-testing at drag start and screen-space
        // projection of the axis during drag.
        let aspect = rect.width().max(1.0) / rect.height().max(1.0);
        let view_proj_drag = effective_camera.view_proj(aspect);
        let inv_view_proj_drag = view_proj_drag.inverse();
        let viewport_wh = [rect.width(), rect.height()];

        // Snapshot the editor's current mode; on drag-start we commit
        // it into `drag.drag_mode` so any mode flip mid-drag is
        // ignored and accumulators stay consistent with the handle
        // the user actually grabbed.
        let editor_mode = viewport_state.gizmo_mode;

        if response.drag_started_by(egui::PointerButton::Primary)
            && let Some(pointer) = response.interact_pointer_pos()
            && let Some(transform) = viewport_bridge.selected_world_transform(selected_id)
            && inv_view_proj_drag.is_finite()
        {
            let local = [pointer.x - rect.left(), pointer.y - rect.top()];
            let ray = Ray::from_viewport_pixel(local, viewport_wh, inv_view_proj_drag);
            let layout = GizmoLayout::centered(transform.translation);
            // Reset every accumulator; the mode we commit to this
            // drag is whichever the editor is currently in.
            viewport_state.drag.dragging_entity = Some(selected_id);
            viewport_state.drag.dragging_axis = None;
            viewport_state.drag.dragging_handle = None;
            viewport_state.drag.drag_mode = editor_mode;
            viewport_state.drag.last_pointer_delta = (0.0, 0.0);
            viewport_state.drag.accumulated_world_delta = (0.0, 0.0, 0.0);
            viewport_state.drag.accumulated_rotation = 0.0;
            viewport_state.drag.last_ring_angle = 0.0;
            viewport_state.drag.accumulated_scale_factor = 1.0;

            match editor_mode {
                GizmoMode::Translate => {
                    viewport_state.drag.dragging_axis = pick_gizmo(&layout, &ray);
                }
                GizmoMode::Rotate => {
                    if let Some(axis) = pick_rotate_handle(&layout, &ray) {
                        viewport_state.drag.dragging_axis = Some(axis);
                        // Snapshot the initial drag-point on the ring
                        // plane so the per-frame angle delta is
                        // well-defined from frame 1.
                        if let Some((_, hit)) =
                            ray_plane_hit(&ray, layout.pivot, axis.direction())
                        {
                            viewport_state.drag.last_ring_angle =
                                angle_on_ring(&layout, axis, hit) as f64;
                        }
                    }
                }
                GizmoMode::Scale => {
                    if let Some(handle) = pick_scale_handle(&layout, &ray) {
                        viewport_state.drag.dragging_handle = Some(handle);
                        if let GizmoHandle::Axis(axis) = handle {
                            viewport_state.drag.dragging_axis = Some(axis);
                        }
                    }
                }
            }
        } else if response.drag_started_by(egui::PointerButton::Primary) {
            // Degenerate drag-start (no transform, non-finite view-proj,
            // no pointer) — reset state so a stale prior drag doesn't
            // bleed into this frame.
            viewport_state.drag.dragging_entity = Some(selected_id);
            viewport_state.drag.dragging_axis = None;
            viewport_state.drag.dragging_handle = None;
            viewport_state.drag.drag_mode = editor_mode;
            viewport_state.drag.last_pointer_delta = (0.0, 0.0);
            viewport_state.drag.accumulated_world_delta = (0.0, 0.0, 0.0);
            viewport_state.drag.accumulated_rotation = 0.0;
            viewport_state.drag.last_ring_angle = 0.0;
            viewport_state.drag.accumulated_scale_factor = 1.0;
        }

        // Read the committed drag mode AFTER drag-start writes it, so
        // a drag_started + dragged same-frame sequence applies the
        // correct mode's drag math.
        let active_mode = viewport_state.drag.drag_mode;

        if response.dragged_by(egui::PointerButton::Primary)
            && viewport_state.drag.dragging_entity == Some(selected_id)
        {
            let drag_delta = response.drag_delta();
            let incremental = egui::vec2(
                drag_delta.x - viewport_state.drag.last_pointer_delta.0,
                drag_delta.y - viewport_state.drag.last_pointer_delta.1,
            );
            viewport_state.drag.last_pointer_delta = (drag_delta.x, drag_delta.y);

            match active_mode {
                GizmoMode::Translate => {
                    match viewport_state.drag.dragging_axis {
                        Some(axis) => {
                            let pivot = viewport_bridge
                                .selected_world_transform(selected_id)
                                .map(|t| t.translation)
                                .unwrap_or(Vec3::ZERO);
                            let world_delta = project_pointer_delta_onto_axis(
                                axis,
                                pivot,
                                incremental,
                                view_proj_drag,
                                viewport_wh,
                            );
                            match axis {
                                GizmoAxis::X => {
                                    viewport_state.drag.accumulated_world_delta.0 +=
                                        world_delta as f64
                                }
                                GizmoAxis::Y => {
                                    viewport_state.drag.accumulated_world_delta.1 +=
                                        world_delta as f64
                                }
                                GizmoAxis::Z => {
                                    viewport_state.drag.accumulated_world_delta.2 +=
                                        world_delta as f64
                                }
                            }
                        }
                        None => {
                            // Free-form drag — legacy XY-plane behaviour.
                            viewport_state.drag.accumulated_world_delta.0 +=
                                incremental.x as f64 * 0.02;
                            viewport_state.drag.accumulated_world_delta.1 -=
                                incremental.y as f64 * 0.02;
                        }
                    }
                }
                GizmoMode::Rotate => {
                    // Only defined when an axis ring was grabbed. We
                    // re-cast the ray from the current pointer and
                    // intersect the ring's plane, then take the signed
                    // angle delta from the previous frame's hit.
                    //
                    // Working in the plane frame means the result is
                    // independent of camera distance — a full pointer
                    // sweep around the pivot is one revolution
                    // regardless of zoom.
                    if let Some(axis) = viewport_state.drag.dragging_axis
                        && let Some(pointer) = response.interact_pointer_pos()
                        && inv_view_proj_drag.is_finite()
                    {
                        let local = [pointer.x - rect.left(), pointer.y - rect.top()];
                        let ray =
                            Ray::from_viewport_pixel(local, viewport_wh, inv_view_proj_drag);
                        let pivot = viewport_bridge
                            .selected_world_transform(selected_id)
                            .map(|t| t.translation)
                            .unwrap_or(Vec3::ZERO);
                        let layout = GizmoLayout::centered(pivot);
                        if let Some((_, hit)) =
                            ray_plane_hit(&ray, layout.pivot, axis.direction())
                        {
                            let new_angle = angle_on_ring(&layout, axis, hit) as f64;
                            // Unwrap the ±π wrap so a drag across the
                            // discontinuity still produces a small,
                            // signed delta rather than a near-full
                            // revolution snap.
                            let mut d = new_angle - viewport_state.drag.last_ring_angle;
                            if d > std::f64::consts::PI {
                                d -= std::f64::consts::TAU;
                            } else if d < -std::f64::consts::PI {
                                d += std::f64::consts::TAU;
                            }
                            viewport_state.drag.accumulated_rotation += d;
                            viewport_state.drag.last_ring_angle = new_angle;
                        }
                    }
                }
                GizmoMode::Scale => {
                    // Axis scale: project pointer delta onto the axis'
                    // screen-space direction (same math as translate),
                    // then convert the world-space scalar into a
                    // multiplicative factor centered at 1.0. Sensitivity
                    // is tuned so one arm_length of drag doubles size.
                    //
                    // Uniform scale: use vertical pointer movement —
                    // down grows, up shrinks. Matches most DCC tools.
                    let pivot = viewport_bridge
                        .selected_world_transform(selected_id)
                        .map(|t| t.translation)
                        .unwrap_or(Vec3::ZERO);
                    let sensitivity = 0.01_f64; // factor per pointer pixel
                    let frame_factor = match viewport_state.drag.dragging_handle {
                        Some(GizmoHandle::Axis(axis)) => {
                            let world_delta = project_pointer_delta_onto_axis(
                                axis,
                                pivot,
                                incremental,
                                view_proj_drag,
                                viewport_wh,
                            ) as f64;
                            // arm_length = 1.2; tune so one arm-length
                            // of drag ≈ 2× scale.
                            1.0 + world_delta * 0.5
                        }
                        Some(GizmoHandle::Uniform) => {
                            // Downward pointer motion (positive y) grows;
                            // upward shrinks.
                            1.0 + incremental.y as f64 * sensitivity
                        }
                        None => 1.0,
                    };
                    // Clamp to a strictly-positive band — a sign flip
                    // would invert the mesh, and 0 would collapse it
                    // irreversibly.
                    let frame_factor = frame_factor.max(0.05);
                    viewport_state.drag.accumulated_scale_factor *= frame_factor;
                }
            }
        }

        if response.drag_stopped_by(egui::PointerButton::Primary)
            && viewport_state.drag.dragging_entity == Some(selected_id)
        {
            match active_mode {
                GizmoMode::Translate => {
                    let (dx, dy, dz) = viewport_state.drag.accumulated_world_delta;
                    if dx.abs() > f64::EPSILON
                        || dy.abs() > f64::EPSILON
                        || dz.abs() > f64::EPSILON
                    {
                        let source = if viewport_state.drag.dragging_axis.is_some() {
                            "viewport_gizmo"
                        } else {
                            "viewport_drag"
                        };
                        actions.push(WorkspaceAction::NudgeTransform {
                            entity_id: selected_id,
                            dx,
                            dy,
                            dz,
                            source,
                        });
                    }
                }
                GizmoMode::Rotate => {
                    if let Some(axis) = viewport_state.drag.dragging_axis {
                        let angle = viewport_state.drag.accumulated_rotation;
                        if angle.abs() > 1e-5 {
                            let (drx, dry, drz) = match axis {
                                GizmoAxis::X => (angle, 0.0, 0.0),
                                GizmoAxis::Y => (0.0, angle, 0.0),
                                GizmoAxis::Z => (0.0, 0.0, angle),
                            };
                            actions.push(WorkspaceAction::RotateTransform {
                                entity_id: selected_id,
                                d_rot_x: drx,
                                d_rot_y: dry,
                                d_rot_z: drz,
                                source: "viewport_gizmo",
                            });
                        }
                    }
                }
                GizmoMode::Scale => {
                    let factor = viewport_state.drag.accumulated_scale_factor;
                    if (factor - 1.0).abs() > 1e-5 && factor > 0.0 {
                        actions.push(WorkspaceAction::ScaleTransform {
                            entity_id: selected_id,
                            factor,
                            source: "viewport_gizmo",
                        });
                    }
                }
            }
            viewport_state.drag.dragging_entity = None;
            viewport_state.drag.dragging_axis = None;
            viewport_state.drag.dragging_handle = None;
            viewport_state.drag.last_pointer_delta = (0.0, 0.0);
            viewport_state.drag.accumulated_world_delta = (0.0, 0.0, 0.0);
            viewport_state.drag.accumulated_rotation = 0.0;
            viewport_state.drag.last_ring_angle = 0.0;
            viewport_state.drag.accumulated_scale_factor = 1.0;
        }
    }

    if response.clicked()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let local_x = (pointer.x - rect.left()).clamp(0.0, rect.width());
        let local_y = (pointer.y - rect.top()).clamp(0.0, rect.height());
        // I-8: real ray-AABB pick through the live camera matrix.
        // Fall back to the legacy MockEngine pick only if we somehow
        // can't invert view-proj (shouldn't happen for a valid camera).
        let aspect = rect.width().max(1.0) / rect.height().max(1.0);
        let view_proj = effective_camera.view_proj(aspect);
        let pick_result = view_proj
            .inverse()
            .is_finite()
            .then(|| {
                viewport_bridge.pick_in_viewport(
                    [local_x, local_y],
                    [rect.width(), rect.height()],
                    view_proj.inverse(),
                )
            })
            .flatten()
            .or_else(|| viewport_bridge.pick(local_x, local_y));
        if let Some(entity_id) = pick_result {
            // I-15: shift-click extends / toggles the selection; plain
            // click replaces it.
            let additive = ui.input(|input| input.modifiers.shift);
            actions.push(WorkspaceAction::SelectEntity {
                entity_id,
                source: "viewport",
                additive,
            });
        }
    }

    let footer = egui::pos2(rect.left() + 16.0, rect.bottom() - 26.0);
    ui.painter().text(
        footer,
        egui::Align2::LEFT_BOTTOM,
        format!(
            "{} entities | {:.0} fps | {} draw calls | {}x{} | drag {:.2},{:.2}",
            snapshot.entity_slots.len(),
            runtime.fps,
            runtime.draw_calls,
            snapshot.width,
            snapshot.height,
            viewport_state.drag.accumulated_world_delta.0,
            viewport_state.drag.accumulated_world_delta.1
        ),
        egui::FontId::monospace(13.0),
        egui::Color32::from_rgb(155, 181, 210),
    );

    actions
}

/// Convert a pointer-pixel delta to a signed world-space translation
/// along `axis`, given the current view-projection and viewport size.
///
/// Math: project the pivot and the pivot-plus-axis endpoint into
/// pixel space via `view_proj`, take their difference as the axis'
/// screen-space direction, and dot the pointer delta onto that
/// direction (divided by its squared length). The result is a scalar
/// measuring "how far along the world axis the pointer moved".
///
/// Handles the degenerate case where the axis is viewed edge-on
/// (screen length ≈ 0) by returning 0.
fn project_pointer_delta_onto_axis(
    axis: GizmoAxis,
    pivot: Vec3,
    pointer_delta_px: egui::Vec2,
    view_proj: Mat4,
    viewport_wh: [f32; 2],
) -> f32 {
    let p0 = world_to_pixel(pivot, view_proj, viewport_wh);
    let p1 = world_to_pixel(pivot + axis.direction(), view_proj, viewport_wh);
    let screen_vec = p1 - p0;
    let len2 = screen_vec.length_sq();
    if len2 < 1e-3 {
        return 0.0;
    }
    (pointer_delta_px.x * screen_vec.x + pointer_delta_px.y * screen_vec.y) / len2
}

fn world_to_pixel(point: Vec3, view_proj: Mat4, viewport_wh: [f32; 2]) -> egui::Vec2 {
    let clip = view_proj * Vec4::new(point.x, point.y, point.z, 1.0);
    let w = if clip.w.abs() < f32::EPSILON { 1.0 } else { clip.w };
    let ndc_x = clip.x / w;
    let ndc_y = clip.y / w;
    let [vw, vh] = viewport_wh;
    egui::vec2(
        (ndc_x * 0.5 + 0.5) * vw,
        (1.0 - (ndc_y * 0.5 + 0.5)) * vh,
    )
}

/// Translate the editor shell's camera state into a render-ready
/// `OrbitCamera` (I-7). Yaw/pitch are stored in degrees for UI
/// convenience but the camera math uses radians; zoom is a multiplier
/// we convert to orbit distance; pan offsets the pivot in world
/// space (currently XZ-plane via pan_x/pan_y).
fn orbit_camera_from_state(state: &ViewportCameraState) -> OrbitCamera {
    let mut cam = OrbitCamera::default();
    cam.yaw = state.orbit_yaw.to_radians();
    cam.pitch = state
        .orbit_pitch
        .to_radians()
        .clamp(-OrbitCamera::MAX_PITCH, OrbitCamera::MAX_PITCH);
    // zoom 1.0 → default distance; higher zoom → closer, lower → further.
    let zoom = state.zoom.max(0.01);
    cam.distance = (OrbitCamera::default().distance / zoom)
        .clamp(OrbitCamera::MIN_DISTANCE, cam.far * 0.5);
    // Pan offsets the pivot. The UI accumulates screen-space deltas
    // scaled by 0.02, so we treat them as XZ world units — good
    // enough for I-7, refined when gizmos land.
    cam.target = Vec3::new(state.pan_x, 0.0, state.pan_y);
    cam
}

fn transform_triplet(entity: &engine::scene::SceneEntity) -> Option<(f64, f64, f64)> {
    let transform = entity
        .components
        .iter()
        .find(|component| component.type_name == "Transform")?;
    Some((
        field_to_f64(transform.fields.get("x")),
        field_to_f64(transform.fields.get("y")),
        field_to_f64(transform.fields.get("z")),
    ))
}

fn field_to_f64(value: Option<&PrimitiveValue>) -> f64 {
    match value {
        Some(PrimitiveValue::F64(v)) => *v,
        Some(PrimitiveValue::I64(v)) => *v as f64,
        _ => 0.0,
    }
}
