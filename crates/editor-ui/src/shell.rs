use engine::events::PlayModeState;
use engine::picking::{GizmoAxis, GizmoHandle, GizmoMode};
use engine::scene::SceneId;
use serde::{Deserialize, Serialize};

use crate::chrome::{DockRegion, EditorChromeDefinition};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PanelSelectionState {
    pub left: Option<String>,
    pub center: Option<String>,
    pub right: Option<String>,
    pub bottom: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewportCameraState {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
    pub orbit_yaw: f32,
    pub orbit_pitch: f32,
}

impl Default for ViewportCameraState {
    fn default() -> Self {
        Self {
            pan_x: 0.0,
            pan_y: 0.0,
            zoom: 1.0,
            orbit_yaw: 0.0,
            orbit_pitch: 22.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewportDragState {
    pub dragging_entity: Option<SceneId>,
    /// Which gizmo axis (if any) the drag is locked to. `None` means
    /// a free-form entity drag — kept as a fallback when the user
    /// drags the entity body rather than a gizmo handle.
    pub dragging_axis: Option<GizmoAxis>,
    /// I-34: scale-mode can grab the uniform handle, which has no
    /// axis. When present, `dragging_axis` is `None` and this is
    /// `Some(GizmoHandle::Uniform)`. For translate/rotate this stays
    /// `None` and `dragging_axis` drives everything.
    #[serde(default)]
    pub dragging_handle: Option<GizmoHandle>,
    /// I-34: which mode initiated the current drag. Stored so the
    /// per-mode drag-end emits the right `WorkspaceAction`
    /// (translate/rotate/scale) even if the user toggles modes
    /// mid-drag (we ignore the toggle until the drag ends).
    #[serde(default)]
    pub drag_mode: GizmoMode,
    pub last_pointer_delta: (f32, f32),
    pub accumulated_world_delta: (f64, f64, f64),
    /// I-34: signed radians accumulated around `dragging_axis` during
    /// a rotate drag. Emitted as `RotateTransform` at drag end.
    #[serde(default)]
    pub accumulated_rotation: f64,
    /// I-34: last sampled angle on the active rotate ring. Each drag
    /// frame computes `new_angle - last_angle`, sums into
    /// `accumulated_rotation`, and stores the new value back here.
    /// Avoids wrap-around when the drag crosses ±π.
    #[serde(default)]
    pub last_ring_angle: f64,
    /// I-34: cumulative scale factor during a scale drag. Starts at
    /// 1.0, multiplies each frame by `(1.0 + pointer_scalar * SENS)`.
    /// Emitted as `ScaleTransform` at drag end with `factor =
    /// accumulated_scale_factor`.
    #[serde(default = "default_scale_factor")]
    pub accumulated_scale_factor: f64,
}

/// `serde` default for `accumulated_scale_factor`. A zero factor would
/// collapse the entity to a point on next apply, so every code path
/// must start from 1.0.
fn default_scale_factor() -> f64 {
    1.0
}

impl Default for ViewportDragState {
    fn default() -> Self {
        Self {
            dragging_entity: None,
            dragging_axis: None,
            dragging_handle: None,
            drag_mode: GizmoMode::default(),
            last_pointer_delta: (0.0, 0.0),
            accumulated_world_delta: (0.0, 0.0, 0.0),
            accumulated_rotation: 0.0,
            last_ring_angle: 0.0,
            accumulated_scale_factor: 1.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ViewportState {
    pub camera: ViewportCameraState,
    pub drag: ViewportDragState,
    /// I-34: current gizmo mode (W/E/R hotkeys toggle this). Drives
    /// which handle geometry the viewport paints and which hit test
    /// runs at drag-start.
    #[serde(default)]
    pub gizmo_mode: GizmoMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EditorShellState {
    pub project_name: String,
    pub open_scene: Option<String>,
    /// Primary selection — the entity the inspector shows and the
    /// gizmo anchors to. `None` = nothing selected.
    pub selected_entity: Option<SceneId>,
    /// Full multi-selection (I-15). Always contains `selected_entity`
    /// as its last element when `selected_entity` is `Some`; shift-
    /// click toggles membership. Ops that act on "the selection" —
    /// nudge, delete, etc. — should iterate this vec.
    #[serde(default)]
    pub selected_entities: Vec<SceneId>,
    pub play_mode: PlayModeState,
    pub search_query: String,
    pub status_message: String,
    pub active_panels: PanelSelectionState,
    pub viewport: ViewportState,
}

impl EditorShellState {
    pub fn new(project_name: impl Into<String>, chrome: &EditorChromeDefinition) -> Self {
        let mut shell = Self {
            project_name: project_name.into(),
            open_scene: None,
            selected_entity: None,
            selected_entities: Vec::new(),
            play_mode: PlayModeState::Editing,
            search_query: String::new(),
            status_message: "Ready".into(),
            active_panels: PanelSelectionState::default(),
            viewport: ViewportState::default(),
        };
        shell.ensure_valid_active_tabs(chrome);
        shell
    }

    /// Apply a click-selection result to the shell (I-15).
    ///
    /// - `additive = false` (plain click): selection becomes just `id`.
    /// - `additive = true`  (shift-click): toggle `id` in the set.
    ///   Promoting the newly-toggled entity to `selected_entity` keeps
    ///   the inspector pointing at whatever the user last interacted
    ///   with.
    pub fn apply_selection(&mut self, id: SceneId, additive: bool) {
        if additive {
            if let Some(pos) = self.selected_entities.iter().position(|e| *e == id) {
                self.selected_entities.remove(pos);
                // Picking the primary from whatever remains is better
                // than leaving a stale one; empty → None.
                self.selected_entity = self.selected_entities.last().copied();
            } else {
                self.selected_entities.push(id);
                self.selected_entity = Some(id);
            }
        } else {
            self.selected_entities = vec![id];
            self.selected_entity = Some(id);
        }
    }

    /// Replace selection with an explicit single id (or clear with
    /// `None`). Used when the hierarchy list picks an entity — no
    /// modifier key ergonomics there yet.
    pub fn set_single_selection(&mut self, id: Option<SceneId>) {
        self.selected_entity = id;
        self.selected_entities = id.into_iter().collect();
    }

    pub fn active_panel(&self, region: DockRegion) -> Option<&str> {
        self.active_panel_slot(region).as_deref()
    }

    pub fn active_panel_mut(&mut self, region: DockRegion) -> &mut Option<String> {
        match region {
            DockRegion::Left => &mut self.active_panels.left,
            DockRegion::Center => &mut self.active_panels.center,
            DockRegion::Right => &mut self.active_panels.right,
            DockRegion::Bottom => &mut self.active_panels.bottom,
        }
    }

    pub fn ensure_valid_active_tabs(&mut self, chrome: &EditorChromeDefinition) {
        self.ensure_region_active(DockRegion::Left, chrome);
        self.ensure_region_active(DockRegion::Center, chrome);
        self.ensure_region_active(DockRegion::Right, chrome);
        self.ensure_region_active(DockRegion::Bottom, chrome);
    }

    fn ensure_region_active(&mut self, region: DockRegion, chrome: &EditorChromeDefinition) {
        let visible = chrome.panels_in(region);
        let slot = self.active_panel_mut(region);

        if visible.is_empty() {
            *slot = None;
            return;
        }

        let current_is_valid = slot
            .as_ref()
            .is_some_and(|current| visible.iter().any(|panel| &panel.id == current));

        if !current_is_valid {
            *slot = Some(visible[0].id.clone());
        }
    }

    fn active_panel_slot(&self, region: DockRegion) -> &Option<String> {
        match region {
            DockRegion::Left => &self.active_panels.left,
            DockRegion::Center => &self.active_panels.center,
            DockRegion::Right => &self.active_panels.right,
            DockRegion::Bottom => &self.active_panels.bottom,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RuntimeStats {
    pub frame_index: u64,
    pub frame_time_ms: f32,
    pub fps: f32,
    pub draw_calls: u32,
    pub gpu_memory_mb: u32,
}

impl Default for RuntimeStats {
    fn default() -> Self {
        Self {
            frame_index: 0,
            frame_time_ms: 16.6,
            fps: 60.0,
            draw_calls: 184,
            gpu_memory_mb: 512,
        }
    }
}

impl RuntimeStats {
    pub fn tick(&mut self) {
        self.frame_index += 1;
        let phase = (self.frame_index % 120) as f32;
        self.frame_time_ms = 14.0 + (phase / 12.0).sin().abs() * 5.0;
        self.fps = 1000.0 / self.frame_time_ms;
        self.draw_calls = 180 + (self.frame_index % 25) as u32;
        self.gpu_memory_mb = 512 + (self.frame_index % 16) as u32;
    }
}

#[cfg(test)]
mod tests {
    use crate::chrome::{DockRegion, EditorChromeDefinition};

    use super::EditorShellState;

    #[test]
    fn apply_selection_toggles_with_shift_and_replaces_without() {
        use engine::scene::SceneId;
        let chrome = EditorChromeDefinition::default();
        let mut shell = EditorShellState::new("RustForge", &chrome);

        // Plain click → single selection.
        shell.apply_selection(SceneId::new(1), false);
        assert_eq!(shell.selected_entity, Some(SceneId::new(1)));
        assert_eq!(shell.selected_entities, vec![SceneId::new(1)]);

        // Shift-click a second id → extended set.
        shell.apply_selection(SceneId::new(2), true);
        assert_eq!(shell.selected_entity, Some(SceneId::new(2)));
        assert_eq!(
            shell.selected_entities,
            vec![SceneId::new(1), SceneId::new(2)]
        );

        // Shift-click the first id again → toggled off.
        shell.apply_selection(SceneId::new(1), true);
        assert_eq!(shell.selected_entities, vec![SceneId::new(2)]);

        // Plain click on a third id → replace with single.
        shell.apply_selection(SceneId::new(3), false);
        assert_eq!(shell.selected_entity, Some(SceneId::new(3)));
        assert_eq!(shell.selected_entities, vec![SceneId::new(3)]);
    }

    #[test]
    fn shell_chooses_active_tabs_from_visible_panels() {
        let chrome = EditorChromeDefinition::default();
        let shell = EditorShellState::new("RustForge", &chrome);

        assert_eq!(shell.active_panel(DockRegion::Center), Some("viewport"));
        assert_eq!(shell.active_panel(DockRegion::Right), Some("inspector"));
        assert_eq!(shell.viewport.camera.zoom, 1.0);
    }
}
