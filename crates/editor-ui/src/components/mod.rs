mod console;
mod content_browser;
mod hierarchy;
mod inspector;
mod menu_bar;
mod profiler;
mod status_bar;
mod toolbar;
mod viewport;
pub mod viewport_3d;
mod workspace;

pub use menu_bar::show_menu_bar;
pub use status_bar::{show_status_bar, ResolvedStatusItem};
pub use toolbar::show_toolbar;
pub use workspace::show_workspace;

use eframe::egui;
use engine::assets::AssetMeta;
use engine::scene::{PrimitiveValue, SceneDocument, SceneId};

use engine::events::PlayModeState;

use crate::chrome::PanelKind;
use crate::shell::{RuntimeStats, ViewportState};
use crate::viewport_bridge::ViewportBridge;

#[derive(Debug, Clone, PartialEq)]
pub enum WorkspaceAction {
    RenameEntity {
        entity_id: SceneId,
        new_name: String,
    },
    SetComponentField {
        entity_id: SceneId,
        component_type: String,
        field_name: String,
        value: PrimitiveValue,
    },
    SelectEntity {
        entity_id: SceneId,
        source: &'static str,
        /// I-15: `true` = shift-click, extend/toggle the current
        /// selection instead of replacing it. `false` = plain click,
        /// replace.
        additive: bool,
    },
    NudgeTransform {
        entity_id: SceneId,
        dx: f64,
        dy: f64,
        dz: f64,
        source: &'static str,
    },
    /// I-16: incremental rotation (Euler radians) applied to one
    /// entity. The core layer re-composes the final quaternion from
    /// `rot_x/rot_y/rot_z` when the scene document syncs back into
    /// the runtime world.
    RotateTransform {
        entity_id: SceneId,
        d_rot_x: f64,
        d_rot_y: f64,
        d_rot_z: f64,
        source: &'static str,
    },
    /// I-17: multiplicative uniform scale. `factor == 1.0` is the
    /// identity; the command layer rejects zero up-front so undo
    /// stays invertible.
    ScaleTransform {
        entity_id: SceneId,
        factor: f64,
        source: &'static str,
    },
    /// I-23: content-browser-driven prefab spawn. `source_path` is
    /// project-relative (e.g. `"assets/prefabs/player.prefab.ron"`);
    /// the app resolves it against the current project root, parses
    /// the prefab, and dispatches a `SpawnPrefabCommand`.
    SpawnPrefab {
        source_path: std::path::PathBuf,
    },
}

pub fn render_panel_content(
    ui: &mut egui::Ui,
    panel_kind: PanelKind,
    scene: &SceneDocument,
    selected_entity: Option<SceneId>,
    selected_entities: &[SceneId],
    assets: &[AssetMeta],
    logs: &[String],
    runtime: &RuntimeStats,
    search_query: &str,
    viewport_bridge: &mut ViewportBridge,
    viewport_state: &mut ViewportState,
    // I-25: current play mode — viewport uses this to decide between
    // the editor orbit camera (Edit/Paused) and the gameplay camera
    // entity (Playing). Other panels ignore it.
    play_mode: PlayModeState,
) -> Vec<WorkspaceAction> {
    match panel_kind {
        PanelKind::Viewport => viewport::render(
            ui,
            scene,
            selected_entity,
            runtime,
            viewport_bridge,
            viewport_state,
            play_mode,
        ),
        PanelKind::Hierarchy => {
            hierarchy::render(ui, scene, selected_entity, selected_entities)
        }
        PanelKind::Inspector => inspector::render(ui, scene, selected_entity),
        PanelKind::ContentBrowser => content_browser::render(ui, assets, search_query),
        PanelKind::Console => {
            console::render(ui, logs);
            Vec::new()
        }
        PanelKind::Profiler => {
            profiler::render(ui, runtime);
            Vec::new()
        }
    }
}
