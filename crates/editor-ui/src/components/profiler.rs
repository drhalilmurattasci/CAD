use eframe::egui;

use crate::shell::RuntimeStats;

pub fn render(ui: &mut egui::Ui, runtime: &RuntimeStats) {
    ui.heading("Profiler");
    ui.separator();

    ui.label(format!("Frame: {}", runtime.frame_index));
    ui.label(format!("Frame time: {:.2} ms", runtime.frame_time_ms));
    ui.label(format!("FPS: {:.0}", runtime.fps));
    ui.label(format!("Draw calls: {}", runtime.draw_calls));
    ui.label(format!("GPU memory (est): {} MB", runtime.gpu_memory_mb));

    ui.separator();
    ui.label("Subsystem health");
    ui.add(
        egui::ProgressBar::new((runtime.fps / 120.0).clamp(0.0, 1.0))
            .text(format!("{:.0} / 120 fps budget", runtime.fps)),
    );
}
