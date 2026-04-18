use std::env;
use std::path::PathBuf;

use eframe::egui;
use engine::assets::AssetMeta;
use engine::commands::{
    Command, CommandStack, NudgeTransformCommand, RenameEntityCommand, RotateTransformCommand,
    ScaleTransformCommand, SetComponentFieldCommand, SpawnPrefabCommand,
};
use engine::events::PlayModeState;
use engine::input::{Input, Key};
use engine::play::PlayModeSession;
use engine::scene::{
    ComponentData, IdAllocator, PrimitiveValue, SceneDocument, SceneEntity, SceneId,
};
use serde::Serialize;

use crate::asset_watcher::AssetWatcher;
use crate::audio_engine::AudioEngine;
use crate::chrome::{DockRegion, EditorChromeDefinition, StatusItemDefinition};
use crate::components::{
    show_menu_bar, show_status_bar, show_toolbar, show_workspace, ResolvedStatusItem,
    WorkspaceAction,
};
use crate::project::ProjectWorkspace;
use crate::shell::{EditorShellState, RuntimeStats};
use crate::viewport_bridge::ViewportBridge;

#[derive(Debug, Clone, Serialize)]
pub struct EditorBootstrapSummary {
    pub project_name: String,
    pub project_root: String,
    pub open_scene: Option<String>,
    pub asset_count: usize,
    pub menu_count: usize,
    pub toolbar_count: usize,
    pub panel_count: usize,
    pub left_active_tab: Option<String>,
    pub center_active_tab: Option<String>,
}

impl EditorBootstrapSummary {
    pub fn to_pretty_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

pub struct RustForgeEditorApp {
    chrome: EditorChromeDefinition,
    shell: EditorShellState,
    project: ProjectWorkspace,
    scene: SceneDocument,
    assets: Vec<AssetMeta>,
    commands: CommandStack,
    logs: Vec<String>,
    runtime: RuntimeStats,
    viewport_bridge: ViewportBridge,
    /// I-20: `Some` while Play mode is active. Holds the snapshot of
    /// the authoring scene taken at play-start; dropping it via
    /// `session.end(&mut self.scene)` restores the authored state so
    /// gameplay mutations never corrupt the saved project.
    play_session: Option<PlayModeSession>,
    /// I-24: filesystem watcher for hot-reload. `None` if the backend
    /// refused to start (permission-denied directories, exhausted
    /// inotify handles on Linux) — the editor still functions, it
    /// just won't notice out-of-band asset edits until the user hits
    /// the manual rescan action.
    asset_watcher: Option<AssetWatcher>,
    /// I-29: rodio-backed audio engine. Silently disables itself when
    /// the host has no output device, so CI runs never fail on
    /// audio init. Play mode transitions feed this with the world's
    /// autoplay commands; stop transitions drain every live sink.
    audio: AudioEngine,
}

impl RustForgeEditorApp {
    pub fn new(chrome: EditorChromeDefinition) -> Self {
        let (project, mut logs) = load_default_project();
        let scene = project.scene.clone();
        let assets = project.assets.clone();
        let mut shell = EditorShellState::new(project.manifest.name.clone(), &chrome);
        shell.open_scene = Some(project.active_scene_file_name());
        shell.set_single_selection(scene.root_entities.first().map(|entity| entity.id));
        shell.status_message = format!("Opened {}", project.relative_path(&project.active_scene_path));
        logs.push(format!(
            "Project root: {}",
            project.root.to_string_lossy().replace('\\', "/")
        ));

        let mut viewport_bridge = ViewportBridge::new();
        // I-5: push the loaded scene into the runtime ECS so the
        // viewport renders entities from the RON file instead of the
        // hardcoded starter tableau.
        viewport_bridge.rebuild_world_from_scene(&scene);
        // I-27: kick off glTF imports for any Mesh { source } entries
        // in the bootstrap scene. Inlined here (rather than going
        // through `rebuild_world_and_import_meshes`) because `self`
        // doesn't exist yet — we're still assembling it.
        let bootstrap_asset_root = project
            .manifest
            .asset_roots
            .first()
            .map(|root| project.root.join(root))
            .unwrap_or_else(|| project.root.join("assets"));
        for (path, result) in viewport_bridge.import_mesh_sources_from_disk(&bootstrap_asset_root) {
            match result {
                Ok(count) => logs.push(format!(
                    "Imported {count} mesh primitive(s) from `{path}`"
                )),
                Err(error) => logs.push(format!("Failed to import `{path}`: {error}")),
            }
        }
        // I-32: same bootstrap pass for Material albedo textures.
        for (path, result) in
            viewport_bridge.import_texture_sources_from_disk(&bootstrap_asset_root)
        {
            match result {
                Ok((w, h)) => {
                    logs.push(format!("Imported texture `{path}` ({w}×{h})"))
                }
                Err(error) => {
                    logs.push(format!("Failed to import texture `{path}`: {error}"))
                }
            }
        }

        // I-24: spin up the filesystem watcher on the project root.
        // Failure is non-fatal — log and move on so the editor opens
        // even if inotify/ReadDirectoryChangesW refuses the handle.
        let asset_watcher = match AssetWatcher::new(project.root.clone()) {
            Ok(watcher) => {
                logs.push(format!(
                    "Watching assets at {}",
                    watcher.root().display().to_string().replace('\\', "/")
                ));
                Some(watcher)
            }
            Err(error) => {
                logs.push(format!("Asset hot-reload disabled: {error}"));
                None
            }
        };

        Self {
            chrome,
            shell,
            project,
            scene,
            assets,
            commands: CommandStack::default(),
            logs,
            runtime: RuntimeStats::default(),
            viewport_bridge,
            play_session: None,
            asset_watcher,
            audio: AudioEngine::new(),
        }
    }

    pub fn summary(&self) -> EditorBootstrapSummary {
        EditorBootstrapSummary {
            project_name: self.shell.project_name.clone(),
            project_root: self.project.root.to_string_lossy().replace('\\', "/"),
            open_scene: self.shell.open_scene.clone(),
            asset_count: self.assets.len(),
            menu_count: self.chrome.menus.len(),
            toolbar_count: self.chrome.toolbar.len(),
            panel_count: self.chrome.panels.len(),
            left_active_tab: self
                .shell
                .active_panel(DockRegion::Left)
                .map(ToOwned::to_owned),
            center_active_tab: self
                .shell
                .active_panel(DockRegion::Center)
                .map(ToOwned::to_owned),
        }
    }

    pub fn smoke_test_summary_json(chrome: EditorChromeDefinition) -> Result<String, serde_json::Error> {
        Self::new(chrome).summary().to_pretty_json()
    }

    fn handle_action(&mut self, action_id: &str) {
        match action_id {
            "file.new_scene" => {
                self.scene = new_scene_document();
                self.project.set_active_scene_file("untitled.scene.ron");
                self.commands = CommandStack::default();
                self.shell.open_scene = Some(self.project.active_scene_file_name());
                self.shell.set_single_selection(
                    self.scene.root_entities.first().map(|entity| entity.id),
                );
                self.rebuild_world_and_import_meshes();
                self.logs.push("Created new in-memory scene".into());
                self.shell.status_message = "Created untitled scene".into();
            }
            "file.open_project" => {
                match ProjectWorkspace::load_or_bootstrap(default_project_root()) {
                    Ok(project) => {
                        self.shell.project_name = project.manifest.name.clone();
                        self.shell.open_scene = Some(project.active_scene_file_name());
                        self.scene = project.scene.clone();
                        self.assets = project.assets.clone();
                        self.commands = CommandStack::default();
                        self.shell.set_single_selection(
                            self.scene.root_entities.first().map(|entity| entity.id),
                        );
                        self.shell.status_message =
                            format!("Opened {}", project.relative_path(&project.active_scene_path));
                        self.logs.push(format!(
                            "Opened project `{}`",
                            project.root.to_string_lossy().replace('\\', "/")
                        ));
                        self.project = project;
                        // I-27: swap in the new project *before* the
                        // rebuild so `rebuild_world_and_import_meshes`
                        // resolves asset paths against the fresh root.
                        // Reset the import cache too — handles from the
                        // previous project shouldn't short-circuit new
                        // imports that happen to collide by hash.
                        self.viewport_bridge.reset_mesh_import_cache();
                        // I-35: a previously-opened project's ASTs can
                        // collide by relative path with a script in the
                        // new project. Dropping the cache is the only
                        // safe answer — next tick recompiles from disk
                        // under the new root.
                        self.viewport_bridge.reset_script_host();
                        self.rebuild_world_and_import_meshes();
                        // I-24: point the filesystem watcher at the
                        // new project root — the old handle watches
                        // the previous tree and would fire irrelevant
                        // rescans.
                        self.install_asset_watcher();
                    }
                    Err(error) => {
                        self.logs.push(format!("Failed to open project: {error}"));
                        self.shell.status_message = "Project open failed".into();
                    }
                }
            }
            "file.save_scene" => {
                match self.project.save_scene(&self.scene) {
                    Ok(saved_path) => {
                        self.assets = self.project.assets.clone();
                        let relative = self.project.relative_path(&saved_path);
                        self.shell.open_scene = Some(self.project.active_scene_file_name());
                        self.logs.push(format!("Saved `{relative}`"));
                        self.shell.status_message = format!("Saved {relative}");
                    }
                    Err(error) => {
                        self.logs.push(format!("Save failed: {error}"));
                        self.shell.status_message = "Save failed".into();
                    }
                }
            }
            "edit.focus_search" => {
                self.shell.search_query = "Transform".into();
                self.shell.status_message = "Search focused".into();
            }
            "edit.undo" => {
                match self.commands.undo(&mut self.scene) {
                    Ok(true) => {
                        self.viewport_bridge.resync_world_from_scene(&self.scene);
                        self.logs.push("Undid last scene change".into());
                        self.shell.status_message = "Undo".into();
                    }
                    Ok(false) => {
                        self.logs.push("Undo requested with empty history".into());
                        self.shell.status_message = "Nothing to undo".into();
                    }
                    Err(error) => {
                        self.logs.push(format!("Undo failed: {error}"));
                        self.shell.status_message = "Undo failed".into();
                    }
                }
            }
            "edit.redo" => {
                match self.commands.redo(&mut self.scene) {
                    Ok(true) => {
                        self.viewport_bridge.resync_world_from_scene(&self.scene);
                        self.logs.push("Redid last undone scene change".into());
                        self.shell.status_message = "Redo".into();
                    }
                    Ok(false) => {
                        self.logs.push("Redo requested with empty history".into());
                        self.shell.status_message = "Nothing to redo".into();
                    }
                    Err(error) => {
                        self.logs.push(format!("Redo failed: {error}"));
                        self.shell.status_message = "Redo failed".into();
                    }
                }
            }
            "play.toggle" => {
                // I-20: crossing Editing → Playing takes a snapshot;
                // crossing back restores it so authoring state survives
                // any gameplay mutations. Pause keeps the snapshot
                // (still mid-play). See `enter_play_mode`/`exit_play_mode`.
                self.shell.play_mode = match self.shell.play_mode {
                    PlayModeState::Editing => {
                        self.enter_play_mode();
                        PlayModeState::Playing
                    }
                    PlayModeState::Playing | PlayModeState::Paused => {
                        self.exit_play_mode();
                        PlayModeState::Editing
                    }
                };
                self.logs
                    .push(format!("Play mode changed to {}", self.play_mode_label()));
                self.shell.status_message = format!("Play mode: {}", self.play_mode_label());
            }
            "play.pause" => {
                self.shell.play_mode = PlayModeState::Paused;
                self.logs.push("Play mode paused".into());
                self.shell.status_message = "Play mode paused".into();
            }
            "play.stop" => {
                // "Stop" always exits to Editing regardless of the
                // current play sub-state (Playing or Paused). The
                // snapshot restore is idempotent — `exit_play_mode`
                // is a no-op if no session is live.
                if self.play_session.is_some() {
                    self.exit_play_mode();
                }
                self.shell.play_mode = PlayModeState::Editing;
                self.logs.push("Play mode stopped".into());
                self.shell.status_message = "Returned to editing".into();
            }
            action if action.starts_with("panel.") => {
                let panel_id = action.trim_start_matches("panel.");
                self.chrome.toggle_panel(panel_id);
                self.shell.ensure_valid_active_tabs(&self.chrome);
                self.logs.push(format!("Toggled panel `{panel_id}`"));
                self.shell.status_message = format!("Toggled {panel_id}");
            }
            _ => {
                self.logs.push(format!("Unhandled action `{action_id}`"));
                self.shell.status_message = format!("Action {action_id}");
            }
        }
    }

    fn apply_workspace_action(&mut self, action: WorkspaceAction) {
        match action {
            WorkspaceAction::RenameEntity { entity_id, new_name } => {
                let label = new_name.clone();
                self.execute_scene_command(Box::new(RenameEntityCommand::new(entity_id, new_name)));
                self.shell.status_message = format!("Renamed entity to {label}");
            }
            WorkspaceAction::SetComponentField {
                entity_id,
                component_type,
                field_name,
                value,
            } => {
                let field_label = format!("{component_type}.{field_name}");
                self.execute_scene_command(Box::new(SetComponentFieldCommand::new(
                    entity_id,
                    component_type,
                    field_name,
                    value,
                )));
                self.shell.status_message = format!("Updated {field_label}");
            }
            WorkspaceAction::SelectEntity {
                entity_id,
                source,
                additive,
            } => {
                self.shell.apply_selection(entity_id, additive);
                let count = self.shell.selected_entities.len();
                let verb = if additive { "toggled" } else { "selected" };
                self.logs
                    .push(format!("{verb} entity {entity_id} from {source}"));
                self.shell.status_message = if count <= 1 {
                    format!("Selected entity {entity_id}")
                } else {
                    format!("Selected {count} entities")
                };
            }
            WorkspaceAction::NudgeTransform {
                entity_id,
                dx,
                dy,
                dz,
                source,
            } => {
                self.execute_scene_command(Box::new(NudgeTransformCommand::new(
                    entity_id, dx, dy, dz,
                )));
                self.logs.push(format!(
                    "Nudged entity {entity_id} from {source} by ({dx:.1}, {dy:.1}, {dz:.1})"
                ));
                self.shell.status_message = format!("Moved entity {entity_id}");
            }
            WorkspaceAction::RotateTransform {
                entity_id,
                d_rot_x,
                d_rot_y,
                d_rot_z,
                source,
            } => {
                self.execute_scene_command(Box::new(RotateTransformCommand::new(
                    entity_id, d_rot_x, d_rot_y, d_rot_z,
                )));
                let deg = (
                    d_rot_x.to_degrees(),
                    d_rot_y.to_degrees(),
                    d_rot_z.to_degrees(),
                );
                self.logs.push(format!(
                    "Rotated entity {entity_id} from {source} by ({:.1}°, {:.1}°, {:.1}°)",
                    deg.0, deg.1, deg.2
                ));
                self.shell.status_message = format!("Rotated entity {entity_id}");
            }
            WorkspaceAction::ScaleTransform {
                entity_id,
                factor,
                source,
            } => {
                self.execute_scene_command(Box::new(ScaleTransformCommand::new(
                    entity_id, factor,
                )));
                self.logs.push(format!(
                    "Scaled entity {entity_id} from {source} by ×{factor:.3}"
                ));
                self.shell.status_message = format!("Scaled entity {entity_id}");
            }
            WorkspaceAction::SpawnPrefab { source_path } => {
                // I-23: load the prefab from disk, materialise fresh
                // scene ids starting above the current max so the
                // spawned subtree can't collide with authored entities,
                // and auto-select the root so the user sees what
                // landed. A prefab spawn adds entities (not just
                // transforms), so we go full-rebuild rather than the
                // resync path `execute_scene_command` normally uses.
                let path_label = source_path.to_string_lossy().replace('\\', "/");
                match self.project.load_prefab(&source_path) {
                    Ok(prefab) => {
                        let mut ids = IdAllocator::new(max_scene_id(&self.scene));
                        let command = SpawnPrefabCommand::new(None, &prefab, &mut ids, None);
                        let root_id = command.root_id();
                        let label = command.label();
                        match self.commands.execute(&mut self.scene, Box::new(command)) {
                            Ok(()) => {
                                self.rebuild_world_and_import_meshes();
                                self.shell.set_single_selection(Some(root_id));
                                self.logs.push(format!(
                                    "Spawned prefab `{path_label}` as entity {root_id}"
                                ));
                                self.shell.status_message =
                                    format!("Spawned {path_label}");
                            }
                            Err(error) => {
                                self.logs.push(format!(
                                    "Command `{label}` failed: {error}"
                                ));
                                self.shell.status_message =
                                    format!("Spawn failed: {path_label}");
                            }
                        }
                    }
                    Err(error) => {
                        self.logs.push(format!(
                            "Failed to load prefab `{path_label}`: {error}"
                        ));
                        self.shell.status_message = format!("Load failed: {path_label}");
                    }
                }
            }
        }
    }

    fn execute_scene_command(
        &mut self,
        command: Box<dyn Command<engine::scene::SceneDocument, engine::commands::CommandError>>,
    ) {
        let label = command.label();
        match self.commands.execute(&mut self.scene, command) {
            Ok(()) => {
                // I-10: propagate authoring edits into the runtime ECS
                // so the viewport reflects the command immediately. We
                // use `resync_transforms_from_scene` (not a full
                // rebuild) to preserve transient state — Spin rotation
                // progress, future per-frame caches — across the edit.
                self.viewport_bridge.resync_world_from_scene(&self.scene);
                self.logs.push(format!("Executed `{label}`"));
            }
            Err(error) => {
                self.logs.push(format!("Command `{label}` failed: {error}"));
                self.shell.status_message = format!("Command failed: {label}");
            }
        }
    }

    fn resolved_status_items(&self) -> Vec<ResolvedStatusItem> {
        self.chrome
            .status_items
            .iter()
            .map(|item| ResolvedStatusItem {
                id: item.id.clone(),
                label: item.label.clone(),
                icon_text: item.icon.short_label().to_owned(),
                value: self.resolve_status_value(item),
            })
            .collect()
    }

    fn resolve_status_value(&self, item: &StatusItemDefinition) -> String {
        match item.value_key.as_str() {
            "scene" => self.shell.open_scene.clone().unwrap_or_else(|| "No scene".into()),
            "project" => self.shell.project_name.clone(),
            "selection" => self
                .shell
                .selected_entity
                .map(|id| format!("Entity {id}"))
                .unwrap_or_else(|| "Nothing selected".into()),
            "history" => format!("{} / {}", self.commands.undo_len(), self.commands.redo_len()),
            "play_mode" => self.play_mode_label().to_owned(),
            "fps" => format!("{:.0} fps", self.runtime.fps),
            "frame_time_ms" => format!("{:.1} ms", self.runtime.frame_time_ms),
            "status" => self.shell.status_message.clone(),
            _ => "n/a".into(),
        }
    }

    fn play_mode_label(&self) -> &'static str {
        match self.shell.play_mode {
            PlayModeState::Editing => "Editing",
            PlayModeState::Playing => "Playing",
            PlayModeState::Paused => "Paused",
        }
    }

    /// I-20: snapshot the authoring scene before gameplay mutations
    /// land, and drop the edit-time undo history. Command history
    /// spanning play-mode would be dangerous — undoing past a
    /// gameplay spawn would leave authored entities missing after
    /// stop. Clearing is the safe default; a richer system (separate
    /// play-mode stack) can layer on later.
    fn enter_play_mode(&mut self) {
        debug_assert!(
            self.play_session.is_none(),
            "enter_play_mode called while a session already exists"
        );
        self.play_session = Some(PlayModeSession::begin(&self.scene));
        self.commands = CommandStack::default();
        // I-35: fresh `TIME` for scripts on each Play entry — a
        // lingering clock from a previous session would teleport any
        // `sin(TIME)`-driven animation. Cache is preserved across
        // sessions so the first frame after Play doesn't stall
        // recompiling every script from scratch.
        self.viewport_bridge.reset_script_host();
        // I-29: fire autoplay audio sources exactly once as the scene
        // transitions into Play. Running this inside `update()` every
        // frame would re-trigger loops constantly; doing it on the
        // edge means "autoplay" behaves the way ambient-music
        // authoring intends.
        let commands = self.viewport_bridge.world().collect_autoplay_audio();
        if !commands.is_empty() {
            let asset_root = self.project.root.clone();
            self.audio.apply_commands(&commands, &asset_root);
            self.logs.push(format!(
                "Audio: {} autoplay source(s) triggered",
                commands.len()
            ));
        }
    }

    /// I-27: rebuild the runtime world from the current scene and
    /// kick off any glTF imports the scene references. Every call
    /// site that used to do a bare `rebuild_world_from_scene` now
    /// funnels through here so newly-authored `Mesh { source: ... }`
    /// entities pick up their GPU uploads consistently.
    ///
    /// Logs a line per successful / failed import. Failures are
    /// non-fatal — the entity just renders with the fallback (no
    /// mesh) until the file is fixed and the user triggers a rescan.
    fn rebuild_world_and_import_meshes(&mut self) {
        self.viewport_bridge.rebuild_world_from_scene(&self.scene);
        self.import_scene_meshes();
    }

    /// Import every glTF asset the current scene references, logging
    /// per-file results. Separated from `rebuild_world_and_import_meshes`
    /// so hot-reload callers can re-run imports without rebuilding the
    /// entire ECS (handles persist through a resync).
    fn import_scene_meshes(&mut self) {
        let asset_root = self
            .project
            .manifest
            .asset_roots
            .first()
            .map(|root| self.project.root.join(root))
            .unwrap_or_else(|| self.project.root.join("assets"));
        let results = self.viewport_bridge.import_mesh_sources_from_disk(&asset_root);
        for (path, result) in results {
            match result {
                Ok(count) => self.logs.push(format!(
                    "Imported {count} mesh primitive(s) from `{path}`"
                )),
                Err(error) => self
                    .logs
                    .push(format!("Failed to import `{path}`: {error}")),
            }
        }
        // I-32: texture import runs alongside mesh import so a scene
        // reload (project switch, scene swap) warms the GPU registry
        // for every authored `albedo_texture` before the next frame.
        let texture_results = self
            .viewport_bridge
            .import_texture_sources_from_disk(&asset_root);
        for (path, result) in texture_results {
            match result {
                Ok((w, h)) => self
                    .logs
                    .push(format!("Imported texture `{path}` ({w}×{h})")),
                Err(error) => self
                    .logs
                    .push(format!("Failed to import texture `{path}`: {error}")),
            }
        }
    }

    /// I-24: drain filesystem events and refresh the asset list when
    /// anything changed. Called every frame from `update()`; cheap on
    /// idle frames (single atomic channel poll returning empty vec).
    ///
    /// We only react to paths that fall under a watched asset root.
    /// Events outside `assets/` (e.g. the user editing
    /// `rustforge-project.json`) are dropped for now — a future pass
    /// can expand into manifest/scene live-reload.
    fn pump_asset_watcher(&mut self) {
        let Some(watcher) = &self.asset_watcher else {
            return;
        };
        let events = watcher.drain();
        if events.is_empty() {
            return;
        }

        // Filter to events that touch one of the manifest's asset
        // roots. A project may watch `assets/` but also receive noise
        // from `.git/`, editor lock files, etc.
        let asset_roots: Vec<_> = self
            .project
            .manifest
            .asset_roots
            .iter()
            .map(|root| self.project.root.join(root))
            .collect();

        let relevant: Vec<_> = events
            .into_iter()
            .filter(|event| asset_roots.iter().any(|root| event.path.starts_with(root)))
            .collect();

        if relevant.is_empty() {
            return;
        }

        match self.project.rescan_assets() {
            Ok(()) => {
                self.assets = self.project.assets.clone();
                let summary = if relevant.len() == 1 {
                    format!(
                        "Asset changed: {}",
                        self.project.relative_path(&relevant[0].path)
                    )
                } else {
                    format!("{} asset changes detected", relevant.len())
                };
                self.logs.push(summary.clone());
                self.shell.status_message = summary;
            }
            Err(error) => {
                self.logs.push(format!("Asset rescan failed: {error}"));
                self.shell.status_message = "Asset rescan failed".into();
            }
        }
    }

    /// I-24: (re)install the filesystem watcher on the current project
    /// root. Called after `file.open_project` swaps to a new root so
    /// the watcher doesn't keep firing events at the old tree.
    fn install_asset_watcher(&mut self) {
        self.asset_watcher = match AssetWatcher::new(self.project.root.clone()) {
            Ok(watcher) => {
                self.logs.push(format!(
                    "Watching assets at {}",
                    watcher.root().display().to_string().replace('\\', "/")
                ));
                Some(watcher)
            }
            Err(error) => {
                self.logs.push(format!("Asset hot-reload disabled: {error}"));
                None
            }
        };
    }

    /// I-20: restore the authored scene and resync the ECS world so
    /// transient runtime state (Spin rotation progress, selection,
    /// hovered gizmo axis) starts fresh from the snapshot. No-op if
    /// the session was already torn down (paused-then-stopped path).
    fn exit_play_mode(&mut self) {
        if let Some(session) = self.play_session.take() {
            session.end(&mut self.scene);
            // Full rebuild rather than resync — gameplay may have
            // added or removed entities, which the in-place resync
            // path can't reconcile. Re-running imports is cheap
            // because `uploaded_handles` short-circuits anything still
            // resident in the GPU registry.
            self.rebuild_world_and_import_meshes();
            self.commands = CommandStack::default();
            // I-29: silence every active sink on stop. A lingering
            // loop bleeding into Edit mode would make audio
            // authoring miserable — changes to AudioSource fields
            // wouldn't take effect until the next Play entry.
            self.audio.stop_all();
        }
    }
}

impl eframe::App for RustForgeEditorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.runtime.tick();
        // I-4: advance the runtime ECS. Using the same cadence as the
        // headless runtime stats — frame_time_ms / 1000 — keeps the
        // Spin component synced with wall-clock regardless of how the
        // editor is paced.
        self.viewport_bridge
            .tick_world(self.runtime.frame_time_ms / 1000.0);

        // I-24: drain any filesystem changes observed since the last
        // frame. Cheap on idle frames — returns immediately when the
        // channel is empty.
        self.pump_asset_watcher();

        // I-26: while Play mode is active, sample the egui keyboard
        // state, translate into a core `Input`, and step gameplay
        // systems (`Mover`) once. `tick_world` above is the demo Spin
        // animation and keeps running in both modes; `tick_gameplay`
        // is gated on Play so edit-mode drags don't drift the player.
        if self.shell.play_mode == PlayModeState::Playing {
            let dt = self.runtime.frame_time_ms / 1000.0;
            let input = sample_input_from_egui(ctx);
            self.viewport_bridge.tick_gameplay(&input, dt);
            // I-35: scripts run *after* physics/mover so they observe
            // the post-physics pose and their writes become the frame's
            // authoritative transform. Errors drain into the same log
            // channel the asset watcher / audio engine use so the
            // Console panel shows them without extra plumbing.
            let project_root = self.project.root.clone();
            self.viewport_bridge.tick_scripts(&input, dt, &project_root);
            for error in self.viewport_bridge.drain_script_errors() {
                self.logs.push(format!(
                    "Script `{}`: {}",
                    error.path, error.message,
                ));
            }
        }

        for action in show_menu_bar(ctx, &self.chrome.menus) {
            self.handle_action(&action);
        }

        for action in show_toolbar(
            ctx,
            &self.chrome.toolbar,
            &self.chrome.toggled_action_ids(self.shell.play_mode == PlayModeState::Playing),
        ) {
            self.handle_action(&action);
        }

        let workspace_actions = show_workspace(
            ctx,
            &self.chrome,
            &mut self.shell,
            &self.scene,
            &self.assets,
            &self.logs,
            &self.runtime,
            &mut self.viewport_bridge,
        );
        for action in workspace_actions {
            self.apply_workspace_action(action);
        }

        show_status_bar(ctx, &self.resolved_status_items());
    }
}

pub fn run_editor(chrome: EditorChromeDefinition) -> Result<(), eframe::Error> {
        let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 920.0])
            .with_min_inner_size([1200.0, 760.0])
            .with_title("GameCAD Editor"),
        ..Default::default()
    };

    eframe::run_native(
        "GameCAD Editor",
        native_options,
        Box::new(move |cc| {
            // I-1: capture the negotiated wgpu surface format so the
            // viewport paint callback can build a pipeline that matches
            // egui's compositor color target.
            if let Some(render_state) = cc.wgpu_render_state.as_ref() {
                crate::components::viewport_3d::install_target_format(
                    render_state.target_format,
                );
            }
            Ok(Box::new(RustForgeEditorApp::new(chrome)))
        }),
    )
}

/// I-26: snapshot egui's current keyboard state into a core `Input`.
///
/// The mapping is deliberately one-to-one — no auto-repeat smoothing,
/// no chorded bindings, no axis deadzones. Gameplay systems see the
/// same "is this key held right now?" signal the user is giving the
/// OS, and anything richer (input actions, remapping, analog sticks)
/// layers on top in a later phase.
///
/// Keys the core input layer doesn't know about are silently ignored
/// — the enum is the source of truth for what the ECS cares about.
fn sample_input_from_egui(ctx: &egui::Context) -> Input {
    let mut input = Input::new();
    ctx.input(|state| {
        for (egui_key, core_key) in EGUI_KEY_MAP {
            if state.key_down(*egui_key) {
                input.press(*core_key);
            }
        }
    });
    input
}

/// Static translation table between egui and core input keys. Keeping
/// this as a const slice (rather than a match) means additions are
/// one-line and the compiler catches any drift if either enum gains
/// or loses variants we reference.
const EGUI_KEY_MAP: &[(egui::Key, Key)] = &[
    (egui::Key::W, Key::W),
    (egui::Key::A, Key::A),
    (egui::Key::S, Key::S),
    (egui::Key::D, Key::D),
    (egui::Key::Q, Key::Q),
    (egui::Key::E, Key::E),
    (egui::Key::Space, Key::Space),
    (egui::Key::ArrowUp, Key::ArrowUp),
    (egui::Key::ArrowDown, Key::ArrowDown),
    (egui::Key::ArrowLeft, Key::ArrowLeft),
    (egui::Key::ArrowRight, Key::ArrowRight),
];

fn load_default_project() -> (ProjectWorkspace, Vec<String>) {
    match ProjectWorkspace::load_or_bootstrap(default_project_root()) {
        Ok(project) => (
            project,
            vec![
                "Editor bootstrap complete".into(),
                "Loaded JSON chrome definition".into(),
                "Project workspace ready".into(),
            ],
        ),
        Err(error) => {
            let scene = sample_scene();
            let fallback_root = PathBuf::from("projects/sandbox");
            let project = ProjectWorkspace {
                root: fallback_root.join("fallback"),
                manifest: crate::project::ProjectManifest::default(),
                active_scene_path: fallback_root.join("assets/scenes/sandbox.scene.ron"),
                scene: scene.clone(),
                assets: Vec::new(),
            };
            (
                project,
                vec![
                    "Editor bootstrap complete".into(),
                    "Loaded JSON chrome definition".into(),
                    format!("Project bootstrap failed: {error}"),
                ],
            )
        }
    }
}

fn default_project_root() -> PathBuf {
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("projects")
        .join("sandbox")
}

/// I-23: walk the scene and return the highest `SceneId` in use, or 0
/// if the scene is empty. Feeding this into `IdAllocator::new(max)`
/// guarantees the next allocation lands at `max + 1` — so prefab
/// instantiation can never collide with authored entities regardless
/// of what ids the user has been editing with.
fn max_scene_id(scene: &SceneDocument) -> u64 {
    fn walk(entities: &[SceneEntity], acc: &mut u64) {
        for entity in entities {
            if entity.id.0 > *acc {
                *acc = entity.id.0;
            }
            walk(&entity.children, acc);
        }
    }
    let mut max = 0u64;
    walk(&scene.root_entities, &mut max);
    max
}

fn new_scene_document() -> SceneDocument {
    let camera = SceneEntity::new(SceneId::new(10), "Camera").with_component(
        ComponentData::new("Transform")
            .with_field("x", PrimitiveValue::F64(0.0))
            .with_field("y", PrimitiveValue::F64(3.0))
            .with_field("z", PrimitiveValue::F64(-8.0)),
    );
    // Give new scenes a starter cube so the 3D viewport isn't empty on
    // File → New.
    let cube = SceneEntity::new(SceneId::new(11), "SpinCube")
        .with_component(ComponentData::new("Transform"))
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        );
    SceneDocument::new("Untitled")
        .with_root(camera)
        .with_root(cube)
}

fn sample_scene() -> SceneDocument {
    let camera = SceneEntity::new(SceneId::new(1), "Editor Camera").with_component(
        ComponentData::new("Transform")
            .with_field("x", PrimitiveValue::F64(0.0))
            .with_field("y", PrimitiveValue::F64(2.5))
            .with_field("z", PrimitiveValue::F64(-6.0)),
    );
    let player = SceneEntity::new(SceneId::new(2), "Player")
        .with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(1.0))
                .with_field("y", PrimitiveValue::F64(0.0))
                .with_field("z", PrimitiveValue::F64(0.0)),
        )
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        );
    let light = SceneEntity::new(SceneId::new(3), "Key Light").with_component(
        ComponentData::new("Light")
            .with_field("intensity", PrimitiveValue::F64(4500.0))
            .with_field("casts_shadows", PrimitiveValue::Bool(true)),
    );
    let center = SceneEntity::new(SceneId::new(4), "SpinCube Center")
        .with_component(ComponentData::new("Transform"))
        .with_component(
            ComponentData::new("Mesh")
                .with_field("primitive", PrimitiveValue::String("cube".into())),
        );

    SceneDocument::new("Sandbox")
        .with_root(camera)
        .with_root(player)
        .with_root(light)
        .with_root(center)
}
