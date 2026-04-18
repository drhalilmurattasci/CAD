use eframe::egui;

pub fn render(ui: &mut egui::Ui, logs: &[String]) {
    ui.heading("Console");
    ui.separator();

    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in logs {
                ui.monospace(line);
            }
        });
}
