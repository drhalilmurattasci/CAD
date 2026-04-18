use eframe::egui;
use engine::assets::AssetMeta;
use engine::events::PlayModeState;
use engine::scene::SceneDocument;

use crate::chrome::{DockRegion, EditorChromeDefinition, PanelDefinition};
use crate::components::{render_panel_content, WorkspaceAction};
use crate::shell::{EditorShellState, RuntimeStats};
use crate::viewport_bridge::ViewportBridge;

pub fn show_workspace(
    ctx: &egui::Context,
    chrome: &EditorChromeDefinition,
    shell: &mut EditorShellState,
    scene: &SceneDocument,
    assets: &[AssetMeta],
    logs: &[String],
    runtime: &RuntimeStats,
    viewport_bridge: &mut ViewportBridge,
) -> Vec<WorkspaceAction> {
    let mut actions = Vec::new();
    // I-25: snapshot the current play mode up front. `shell` is
    // reborrowed by each panel's render closure, so pulling this
    // scalar once keeps us from fighting the borrow checker over it.
    let play_mode = shell.play_mode;
    let left_panels = chrome.panels_in(DockRegion::Left);
    if !left_panels.is_empty() {
        egui::SidePanel::left("workspace_left")
            .default_width(290.0)
            .resizable(true)
            .show(ctx, |ui| {
                actions.extend(render_region(
                    ui,
                    DockRegion::Left,
                    &left_panels,
                    shell,
                    scene,
                    assets,
                    logs,
                    runtime,
                    viewport_bridge,
                    play_mode,
                ));
            });
    }

    let right_panels = chrome.panels_in(DockRegion::Right);
    if !right_panels.is_empty() {
        egui::SidePanel::right("workspace_right")
            .default_width(320.0)
            .resizable(true)
            .show(ctx, |ui| {
                actions.extend(render_region(
                    ui,
                    DockRegion::Right,
                    &right_panels,
                    shell,
                    scene,
                    assets,
                    logs,
                    runtime,
                    viewport_bridge,
                    play_mode,
                ));
            });
    }

    let bottom_panels = chrome.panels_in(DockRegion::Bottom);
    if !bottom_panels.is_empty() {
        egui::TopBottomPanel::bottom("workspace_bottom")
            .default_height(220.0)
            .resizable(true)
            .show(ctx, |ui| {
                actions.extend(render_region(
                    ui,
                    DockRegion::Bottom,
                    &bottom_panels,
                    shell,
                    scene,
                    assets,
                    logs,
                    runtime,
                    viewport_bridge,
                    play_mode,
                ));
            });
    }

    egui::CentralPanel::default().show(ctx, |ui| {
        let center_panels = chrome.panels_in(DockRegion::Center);
        actions.extend(render_region(
            ui,
            DockRegion::Center,
            &center_panels,
            shell,
            scene,
            assets,
            logs,
            runtime,
            viewport_bridge,
            play_mode,
        ));
    });

    actions
}

fn render_region(
    ui: &mut egui::Ui,
    region: DockRegion,
    panels: &[PanelDefinition],
    shell: &mut EditorShellState,
    scene: &SceneDocument,
    assets: &[AssetMeta],
    logs: &[String],
    runtime: &RuntimeStats,
    viewport_bridge: &mut ViewportBridge,
    play_mode: PlayModeState,
) -> Vec<WorkspaceAction> {
    let mut actions = Vec::new();
    if panels.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label("No visible panels in this region.");
        });
        return actions;
    }

    let current_active = shell.active_panel(region).map(ToOwned::to_owned);
    if current_active
        .as_ref()
        .is_none_or(|current| !panels.iter().any(|panel| &panel.id == current))
    {
        *shell.active_panel_mut(region) = Some(panels[0].id.clone());
    }

    ui.vertical(|ui| {
        ui.horizontal_wrapped(|ui| {
            for panel in panels {
                let selected = shell.active_panel(region) == Some(panel.id.as_str());
                // Tab is the user-visible name; the UiIcon::short_label()
                // 2-letter code is an internal identifier and shouldn't be
                // concatenated into button text.
                let tab_text = panel.tab.clone();
                if ui.selectable_label(selected, tab_text).clicked() {
                    *shell.active_panel_mut(region) = Some(panel.id.clone());
                }
            }
        });
        ui.separator();

        if let Some(panel) = panels
            .iter()
            .find(|panel| Some(panel.id.as_str()) == shell.active_panel(region))
        {
            // Each panel body renders its own heading (e.g. `Hierarchy`,
            // `Inspector`). Emitting a second one here produced the
            // "Hierarchy / Hierarchy" duplication visible in the GUI.
            let selected_entity = shell.selected_entity;
            // Clone the selection set so the panel closures can borrow
            // `shell` mutably for viewport state without fighting us
            // over simultaneous access to the selection vec.
            let selected_entities = shell.selected_entities.clone();
            let viewport_state = &mut shell.viewport;
            actions.extend(render_panel_content(
                ui,
                panel.kind,
                scene,
                selected_entity,
                &selected_entities,
                assets,
                logs,
                runtime,
                &shell.search_query,
                viewport_bridge,
                viewport_state,
                play_mode,
            ));
        }
    });

    actions
}
