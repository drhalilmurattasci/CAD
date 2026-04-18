use eframe::egui;
use engine::scene::{PrimitiveValue, SceneDocument, SceneId};

use crate::components::WorkspaceAction;

pub fn render(
    ui: &mut egui::Ui,
    scene: &SceneDocument,
    selected: Option<SceneId>,
) -> Vec<WorkspaceAction> {
    let mut actions = Vec::new();
    ui.heading("Inspector");
    ui.separator();

    let Some(selected_id) = selected else {
        ui.label("Select an entity from the hierarchy to inspect it.");
        return actions;
    };

    let Some(entity) = scene.find_entity(selected_id) else {
        ui.label("The selected entity no longer exists.");
        return actions;
    };

    ui.label("Entity");
    let mut entity_name = entity.name.clone();
    let entity_name_response = ui.text_edit_singleline(&mut entity_name);
    if entity_name_response.lost_focus() && entity_name_response.changed() && entity_name != entity.name
    {
        actions.push(WorkspaceAction::RenameEntity {
            entity_id: selected_id,
            new_name: entity_name,
        });
    }

    ui.monospace(format!("SceneId: {}", entity.id));
    ui.separator();

    for component in &entity.components {
        egui::CollapsingHeader::new(&component.type_name)
            .default_open(true)
            .show(ui, |ui| {
                if component.fields.is_empty() {
                    ui.label("No editable fields yet.");
                } else {
                    for (field, value) in &component.fields {
                        if let Some(action) = render_field_editor(
                            ui,
                            selected_id,
                            &component.type_name,
                            field,
                            value,
                        ) {
                            actions.push(action);
                        }
                    }
                }
            });
    }

    actions
}

fn render_field_editor(
    ui: &mut egui::Ui,
    entity_id: SceneId,
    component_type: &str,
    field_name: &str,
    value: &PrimitiveValue,
) -> Option<WorkspaceAction> {
    match value {
        PrimitiveValue::Bool(v) => {
            let mut edited = *v;
            let response = ui.checkbox(&mut edited, field_name);
            if response.changed() {
                return Some(WorkspaceAction::SetComponentField {
                    entity_id,
                    component_type: component_type.to_owned(),
                    field_name: field_name.to_owned(),
                    value: PrimitiveValue::Bool(edited),
                });
            }
        }
        PrimitiveValue::I64(v) => {
            let mut action = None;
            ui.horizontal(|ui| {
                ui.strong(field_name);
                let mut edited = *v;
                let response = ui.add(egui::DragValue::new(&mut edited).speed(1.0));
                if response.changed() {
                    action = Some(WorkspaceAction::SetComponentField {
                        entity_id,
                        component_type: component_type.to_owned(),
                        field_name: field_name.to_owned(),
                        value: PrimitiveValue::I64(edited),
                    });
                }
            });
            return action;
        }
        PrimitiveValue::F64(v) => {
            let mut action = None;
            ui.horizontal(|ui| {
                ui.strong(field_name);
                let mut edited = *v;
                let response = ui.add(egui::DragValue::new(&mut edited).speed(0.1));
                if response.changed() {
                    action = Some(WorkspaceAction::SetComponentField {
                        entity_id,
                        component_type: component_type.to_owned(),
                        field_name: field_name.to_owned(),
                        value: PrimitiveValue::F64(edited),
                    });
                }
            });
            return action;
        }
        PrimitiveValue::String(v) => {
            let mut action = None;
            ui.horizontal(|ui| {
                ui.strong(field_name);
                let mut edited = v.clone();
                let response = ui.text_edit_singleline(&mut edited);
                if response.lost_focus() && response.changed() && edited != *v {
                    action = Some(WorkspaceAction::SetComponentField {
                        entity_id,
                        component_type: component_type.to_owned(),
                        field_name: field_name.to_owned(),
                        value: PrimitiveValue::String(edited),
                    });
                }
            });
            return action;
        }
    }

    None
}
