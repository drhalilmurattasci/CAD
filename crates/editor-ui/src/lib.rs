mod app;
pub mod asset_watcher;
pub mod audio_engine;
pub mod chrome;
pub mod components;
mod project;
mod shell;
mod viewport_bridge;

pub use app::{run_editor, EditorBootstrapSummary, RustForgeEditorApp};
pub use asset_watcher::{AssetChangeEvent, AssetWatcher, WatcherError};
pub use audio_engine::{AudioEngine, AudioError};
pub use chrome::{ChromeDefinitionError, DockRegion, EditorChromeDefinition, PanelKind, UiIcon};
pub use project::{ProjectError, ProjectManifest, ProjectWorkspace};
pub use shell::{
    EditorShellState, PanelSelectionState, RuntimeStats, ViewportCameraState, ViewportDragState,
    ViewportState,
};
pub use viewport_bridge::{ViewportBridge, ViewportSnapshot};

pub fn smoke_test_summary_json(
    chrome: EditorChromeDefinition,
) -> Result<String, serde_json::Error> {
    app::RustForgeEditorApp::smoke_test_summary_json(chrome)
}
