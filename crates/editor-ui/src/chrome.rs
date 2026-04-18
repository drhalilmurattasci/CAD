use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiIcon {
    None,
    Folder,
    Save,
    Play,
    Pause,
    Stop,
    Search,
    Settings,
    Viewport,
    Hierarchy,
    Inspector,
    Content,
    Console,
    Profiler,
    Activity,
    Scene,
}

impl UiIcon {
    pub fn short_label(&self) -> &'static str {
        match self {
            Self::None => "--",
            Self::Folder => "FD",
            Self::Save => "SV",
            Self::Play => "PL",
            Self::Pause => "PS",
            Self::Stop => "ST",
            Self::Search => "SR",
            Self::Settings => "SE",
            Self::Viewport => "VP",
            Self::Hierarchy => "HI",
            Self::Inspector => "IN",
            Self::Content => "CB",
            Self::Console => "CL",
            Self::Profiler => "PR",
            Self::Activity => "AC",
            Self::Scene => "SC",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionDefinition {
    pub id: String,
    pub label: String,
    pub icon: UiIcon,
    pub tooltip: Option<String>,
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MenuEntry {
    Action(ActionDefinition),
    Separator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MenuDefinition {
    pub id: String,
    pub label: String,
    pub entries: Vec<MenuEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DockRegion {
    Left,
    Center,
    Right,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    Viewport,
    Hierarchy,
    Inspector,
    ContentBrowser,
    Console,
    Profiler,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PanelDefinition {
    pub id: String,
    pub label: String,
    pub tab: String,
    pub icon: UiIcon,
    pub region: DockRegion,
    pub kind: PanelKind,
    pub visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusItemDefinition {
    pub id: String,
    pub label: String,
    pub icon: UiIcon,
    pub value_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditorChromeDefinition {
    pub menus: Vec<MenuDefinition>,
    pub toolbar: Vec<ActionDefinition>,
    pub panels: Vec<PanelDefinition>,
    pub status_items: Vec<StatusItemDefinition>,
}

#[derive(Debug, Error)]
pub enum ChromeDefinitionError {
    #[error("failed to read chrome definition at `{path}`")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse chrome definition JSON")]
    Json(#[from] serde_json::Error),
}

impl EditorChromeDefinition {
    pub fn load_from_path(path: &Path) -> Result<Self, ChromeDefinitionError> {
        let source = fs::read_to_string(path).map_err(|error| ChromeDefinitionError::Io {
            path: path.display().to_string(),
            source: error,
        })?;
        Ok(Self::from_json_str(&source)?)
    }

    pub fn from_json_str(source: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(source)
    }

    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn panels_in(&self, region: DockRegion) -> Vec<PanelDefinition> {
        self.panels
            .iter()
            .filter(|panel| panel.region == region && panel.visible)
            .cloned()
            .collect()
    }

    pub fn panel_by_id(&self, panel_id: &str) -> Option<&PanelDefinition> {
        self.panels.iter().find(|panel| panel.id == panel_id)
    }

    pub fn toggle_panel(&mut self, panel_id: &str) {
        if let Some(panel) = self.panels.iter_mut().find(|panel| panel.id == panel_id) {
            panel.visible = !panel.visible;
        }
    }

    pub fn toggled_action_ids(&self, playing: bool) -> Vec<String> {
        let mut toggled = Vec::new();
        if playing {
            toggled.push("play.toggle".to_owned());
        }
        toggled
    }
}

impl Default for EditorChromeDefinition {
    fn default() -> Self {
        Self {
            menus: vec![
                MenuDefinition {
                    id: "file".into(),
                    label: "File".into(),
                    entries: vec![
                        MenuEntry::Action(ActionDefinition {
                            id: "file.new_scene".into(),
                            label: "New Scene".into(),
                            icon: UiIcon::Scene,
                            tooltip: Some("Create a new scene document".into()),
                            shortcut: Some("Ctrl+N".into()),
                        }),
                        MenuEntry::Action(ActionDefinition {
                            id: "file.open_project".into(),
                            label: "Open Project".into(),
                            icon: UiIcon::Folder,
                            tooltip: Some("Open a project folder".into()),
                            shortcut: Some("Ctrl+O".into()),
                        }),
                        MenuEntry::Action(ActionDefinition {
                            id: "file.save_scene".into(),
                            label: "Save Scene".into(),
                            icon: UiIcon::Save,
                            tooltip: Some("Save the active scene".into()),
                            shortcut: Some("Ctrl+S".into()),
                        }),
                    ],
                },
                MenuDefinition {
                    id: "edit".into(),
                    label: "Edit".into(),
                    entries: vec![
                        MenuEntry::Action(ActionDefinition {
                            id: "edit.undo".into(),
                            label: "Undo".into(),
                            icon: UiIcon::Activity,
                            tooltip: Some("Undo the last scene change".into()),
                            shortcut: Some("Ctrl+Z".into()),
                        }),
                        MenuEntry::Action(ActionDefinition {
                            id: "edit.redo".into(),
                            label: "Redo".into(),
                            icon: UiIcon::Activity,
                            tooltip: Some("Redo the last undone scene change".into()),
                            shortcut: Some("Ctrl+Shift+Z".into()),
                        }),
                        MenuEntry::Separator,
                        MenuEntry::Action(ActionDefinition {
                            id: "edit.focus_search".into(),
                            label: "Focus Search".into(),
                            icon: UiIcon::Search,
                            tooltip: Some("Jump to search".into()),
                            shortcut: Some("Ctrl+K".into()),
                        }),
                    ],
                },
                MenuDefinition {
                    id: "view".into(),
                    label: "View".into(),
                    entries: vec![
                        MenuEntry::Action(ActionDefinition {
                            id: "panel.console".into(),
                            label: "Toggle Console".into(),
                            icon: UiIcon::Console,
                            tooltip: Some("Show or hide the console panel".into()),
                            shortcut: Some("Ctrl+`".into()),
                        }),
                        MenuEntry::Action(ActionDefinition {
                            id: "panel.profiler".into(),
                            label: "Toggle Profiler".into(),
                            icon: UiIcon::Profiler,
                            tooltip: Some("Show or hide the profiler panel".into()),
                            shortcut: Some("Shift+P".into()),
                        }),
                    ],
                },
                MenuDefinition {
                    id: "play".into(),
                    label: "Play".into(),
                    entries: vec![
                        MenuEntry::Action(ActionDefinition {
                            id: "play.toggle".into(),
                            label: "Play / Stop".into(),
                            icon: UiIcon::Play,
                            tooltip: Some("Toggle play-in-editor".into()),
                            shortcut: Some("F5".into()),
                        }),
                        MenuEntry::Action(ActionDefinition {
                            id: "play.pause".into(),
                            label: "Pause".into(),
                            icon: UiIcon::Pause,
                            tooltip: Some("Pause simulation".into()),
                            shortcut: Some("F6".into()),
                        }),
                    ],
                },
            ],
            toolbar: vec![
                ActionDefinition {
                    id: "file.open_project".into(),
                    label: "Open".into(),
                    icon: UiIcon::Folder,
                    tooltip: Some("Open project".into()),
                    shortcut: Some("Ctrl+O".into()),
                },
                ActionDefinition {
                    id: "file.save_scene".into(),
                    label: "Save".into(),
                    icon: UiIcon::Save,
                    tooltip: Some("Save scene".into()),
                    shortcut: Some("Ctrl+S".into()),
                },
                ActionDefinition {
                    id: "edit.undo".into(),
                    label: "Undo".into(),
                    icon: UiIcon::Activity,
                    tooltip: Some("Undo".into()),
                    shortcut: Some("Ctrl+Z".into()),
                },
                ActionDefinition {
                    id: "edit.redo".into(),
                    label: "Redo".into(),
                    icon: UiIcon::Activity,
                    tooltip: Some("Redo".into()),
                    shortcut: Some("Ctrl+Shift+Z".into()),
                },
                ActionDefinition {
                    id: "edit.focus_search".into(),
                    label: "Search".into(),
                    icon: UiIcon::Search,
                    tooltip: Some("Focus search".into()),
                    shortcut: Some("Ctrl+K".into()),
                },
                ActionDefinition {
                    id: "play.toggle".into(),
                    label: "Play".into(),
                    icon: UiIcon::Play,
                    tooltip: Some("Play or stop".into()),
                    shortcut: Some("F5".into()),
                },
                ActionDefinition {
                    id: "play.pause".into(),
                    label: "Pause".into(),
                    icon: UiIcon::Pause,
                    tooltip: Some("Pause".into()),
                    shortcut: Some("F6".into()),
                },
                ActionDefinition {
                    id: "play.stop".into(),
                    label: "Stop".into(),
                    icon: UiIcon::Stop,
                    tooltip: Some("Stop PIE".into()),
                    shortcut: Some("F7".into()),
                },
            ],
            panels: vec![
                PanelDefinition {
                    id: "hierarchy".into(),
                    label: "Hierarchy".into(),
                    tab: "Hierarchy".into(),
                    icon: UiIcon::Hierarchy,
                    region: DockRegion::Left,
                    kind: PanelKind::Hierarchy,
                    visible: true,
                },
                PanelDefinition {
                    id: "content_browser".into(),
                    label: "Content Browser".into(),
                    tab: "Content".into(),
                    icon: UiIcon::Content,
                    region: DockRegion::Left,
                    kind: PanelKind::ContentBrowser,
                    visible: true,
                },
                PanelDefinition {
                    id: "viewport".into(),
                    label: "Viewport".into(),
                    tab: "Viewport".into(),
                    icon: UiIcon::Viewport,
                    region: DockRegion::Center,
                    kind: PanelKind::Viewport,
                    visible: true,
                },
                PanelDefinition {
                    id: "inspector".into(),
                    label: "Inspector".into(),
                    tab: "Inspector".into(),
                    icon: UiIcon::Inspector,
                    region: DockRegion::Right,
                    kind: PanelKind::Inspector,
                    visible: true,
                },
                PanelDefinition {
                    id: "console".into(),
                    label: "Console".into(),
                    tab: "Console".into(),
                    icon: UiIcon::Console,
                    region: DockRegion::Bottom,
                    kind: PanelKind::Console,
                    visible: true,
                },
                PanelDefinition {
                    id: "profiler".into(),
                    label: "Profiler".into(),
                    tab: "Profiler".into(),
                    icon: UiIcon::Profiler,
                    region: DockRegion::Bottom,
                    kind: PanelKind::Profiler,
                    visible: true,
                },
            ],
            status_items: vec![
                StatusItemDefinition {
                    id: "scene".into(),
                    label: "Scene".into(),
                    icon: UiIcon::Scene,
                    value_key: "scene".into(),
                },
                StatusItemDefinition {
                    id: "history".into(),
                    label: "History".into(),
                    icon: UiIcon::Activity,
                    value_key: "history".into(),
                },
                StatusItemDefinition {
                    id: "selection".into(),
                    label: "Selection".into(),
                    icon: UiIcon::Inspector,
                    value_key: "selection".into(),
                },
                StatusItemDefinition {
                    id: "play_mode".into(),
                    label: "Mode".into(),
                    icon: UiIcon::Play,
                    value_key: "play_mode".into(),
                },
                StatusItemDefinition {
                    id: "fps".into(),
                    label: "Perf".into(),
                    icon: UiIcon::Activity,
                    value_key: "fps".into(),
                },
            ],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DockRegion, EditorChromeDefinition};

    #[test]
    fn default_chrome_roundtrips_through_json() {
        let chrome = EditorChromeDefinition::default();
        let json = chrome.to_pretty_json().unwrap();
        let decoded = EditorChromeDefinition::from_json_str(&json).unwrap();

        assert_eq!(decoded.toolbar.len(), 8);
        assert_eq!(decoded.panels_in(DockRegion::Bottom).len(), 2);
    }
}
