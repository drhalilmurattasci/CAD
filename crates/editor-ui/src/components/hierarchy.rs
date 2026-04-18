use eframe::egui;
use engine::scene::{SceneDocument, SceneEntity, SceneId};

use crate::components::WorkspaceAction;

/// Render the scene hierarchy panel. Returns any selection-change
/// actions the user clicked during this frame — the caller (workspace
/// dispatcher) routes them back through `apply_workspace_action` so
/// the multi-select state (I-15) stays consistent regardless of
/// whether the click came from the hierarchy or the viewport.
pub fn render(
    ui: &mut egui::Ui,
    scene: &SceneDocument,
    selected: Option<SceneId>,
    selected_set: &[SceneId],
) -> Vec<WorkspaceAction> {
    ui.heading("Hierarchy");
    ui.separator();

    let mut actions = Vec::new();
    for entity in &scene.root_entities {
        render_entity(ui, entity, selected, selected_set, 0, &mut actions);
    }
    actions
}

fn render_entity(
    ui: &mut egui::Ui,
    entity: &SceneEntity,
    selected: Option<SceneId>,
    selected_set: &[SceneId],
    depth: usize,
    actions: &mut Vec<WorkspaceAction>,
) {
    ui.horizontal(|ui| {
        ui.add_space(depth as f32 * 12.0);
        // "Highlighted" covers both the primary selection (shown in a
        // stronger tone) and the secondary multi-select members so
        // shift-click feedback reads at a glance.
        let is_primary = selected.is_some_and(|current| current == entity.id);
        let is_member = selected_set.iter().any(|id| *id == entity.id);
        let label = if is_member && !is_primary {
            format!("● {}", entity.name)
        } else {
            entity.name.clone()
        };
        if ui.selectable_label(is_primary, label).clicked() {
            let additive = ui.input(|input| input.modifiers.shift);
            actions.push(WorkspaceAction::SelectEntity {
                entity_id: entity.id,
                source: "hierarchy",
                additive,
            });
        }
    });

    for child in &entity.children {
        render_entity(ui, child, selected, selected_set, depth + 1, actions);
    }
}
