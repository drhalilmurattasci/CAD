use eframe::egui;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStatusItem {
    pub id: String,
    pub label: String,
    pub icon_text: String,
    pub value: String,
}

pub fn show_status_bar(ctx: &egui::Context, items: &[ResolvedStatusItem]) {
    egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
        ui.horizontal_wrapped(|ui| {
            for item in items {
                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        // `icon_text` is the internal 2-letter UiIcon code
                        // (e.g. "SC" for Scene). Until real icon glyphs land
                        // we show the human-readable label only.
                        ui.small(&item.label);
                        ui.separator();
                        ui.monospace(&item.value);
                    });
                });
            }
        });
    });
}
