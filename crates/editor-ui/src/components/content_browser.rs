use eframe::egui;
use engine::assets::{AssetKind, AssetMeta};

use crate::components::WorkspaceAction;

/// I-23: content browser now returns a list of [`WorkspaceAction`]s
/// so asset-specific verbs (currently: "Spawn" for prefabs) can flow
/// through the same dispatcher every other panel uses.
///
/// Filter: rows whose project-relative path contains `search_query`
/// (case-insensitive) pass. Empty query shows everything.
pub fn render(
    ui: &mut egui::Ui,
    assets: &[AssetMeta],
    search_query: &str,
) -> Vec<WorkspaceAction> {
    let mut actions = Vec::new();

    let needle = search_query.trim().to_lowercase();
    let matching: Vec<&AssetMeta> = assets
        .iter()
        .filter(|asset| {
            needle.is_empty()
                || asset
                    .source
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&needle)
        })
        .collect();

    ui.horizontal(|ui| {
        ui.heading("Content Browser");
        ui.separator();
        ui.label(format!(
            "Filter: {}",
            if search_query.is_empty() {
                "all".into()
            } else {
                format!("`{search_query}`")
            }
        ));
        ui.separator();
        ui.label(format!(
            "{} / {} assets",
            matching.len(),
            assets.len()
        ));
    });
    ui.separator();

    egui::Grid::new("content_browser_grid")
        .num_columns(4)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.strong("Name");
            ui.strong("Kind");
            ui.strong("Actions");
            ui.strong("Guid");
            ui.end_row();

            for asset in matching {
                ui.label(asset.source.to_string_lossy());
                ui.label(format!("{:?}", asset.kind));
                // Prefab assets get a Spawn button. Other kinds show
                // a dimmed placeholder so the column width stays
                // stable as the user filters.
                match asset.kind {
                    AssetKind::Prefab => {
                        if ui.button("Spawn").clicked() {
                            actions.push(WorkspaceAction::SpawnPrefab {
                                source_path: asset.source.clone(),
                            });
                        }
                    }
                    _ => {
                        ui.weak("—");
                    }
                }
                ui.monospace(asset.guid.to_string());
                ui.end_row();
            }
        });

    actions
}
