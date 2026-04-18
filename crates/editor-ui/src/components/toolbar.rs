use eframe::egui;

use crate::chrome::ActionDefinition;

pub fn show_toolbar(
    ctx: &egui::Context,
    actions: &[ActionDefinition],
    toggled_action_ids: &[String],
) -> Vec<String> {
    let mut triggered = Vec::new();

    egui::TopBottomPanel::top("toolbar")
        .show_separator_line(true)
        .exact_height(42.0)
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for action in actions {
                    // Bare label only — see the note in `menu_bar.rs`: the
                    // UiIcon::short_label() 2-letter codes are internal
                    // identifiers and don't belong on user-visible buttons.
                    let label = action.label.clone();
                    let selected = toggled_action_ids.iter().any(|id| id == &action.id);
                    let response = ui
                        .selectable_label(selected, label)
                        .on_hover_text(action.tooltip.clone().unwrap_or_default());
                    if response.clicked() {
                        triggered.push(action.id.clone());
                    }
                }
            });
        });

    triggered
}
