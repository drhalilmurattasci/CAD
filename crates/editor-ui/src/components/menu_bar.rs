use eframe::egui;

use crate::chrome::{MenuDefinition, MenuEntry};

pub fn show_menu_bar(ctx: &egui::Context, menus: &[MenuDefinition]) -> Vec<String> {
    let mut actions = Vec::new();

    egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
        egui::MenuBar::new().ui(ui, |ui| {
            for menu in menus {
                ui.menu_button(&menu.label, |ui| {
                    for entry in &menu.entries {
                        match entry {
                            MenuEntry::Action(action) => {
                                // We intentionally drop `icon.short_label()` here — the
                                // 2-letter codes (`FD`, `SV`, `AC`, …) are internal
                                // identifiers for the UiIcon enum, not glyphs the user
                                // should ever see. Until a real icon font lands, menu
                                // entries read cleaner with just the label.
                                let text = if let Some(shortcut) = &action.shortcut {
                                    format!("{}    {}", action.label, shortcut)
                                } else {
                                    action.label.clone()
                                };

                                if ui
                                    .button(text)
                                    .on_hover_text(action.tooltip.clone().unwrap_or_default())
                                    .clicked()
                                {
                                    actions.push(action.id.clone());
                                    ui.close();
                                }
                            }
                            MenuEntry::Separator => {
                                ui.separator();
                            }
                        }
                    }
                });
            }
        });
    });

    actions
}
