//! I-35: Rhai scripting host.
//!
//! Attaches a `Script { source }` component to any scene entity that
//! wants gameplay logic driven from a `.rhai` file. During Play mode the
//! editor calls [`ScriptHost::tick_world`] once per frame; the host
//! walks every entity carrying a `Script` + `Transform`, compiles the
//! script on first touch (re-compiling on disk mtime change), and
//! evaluates it inside a per-entity [`rhai::Scope`] whose variables
//! mirror the entity's transform fields.
//!
//! ## The per-entity scope
//!
//! **Mutable vars (read back into `Transform` after eval):**
//!   - `pos_x / pos_y / pos_z` — translation (world space)
//!   - `rot_x / rot_y / rot_z` — Euler angles (XYZ, radians)
//!   - `scl_x / scl_y / scl_z` — scale
//!
//! **Read-only constants:**
//!   - `DT`      — seconds since last script tick (`f64`)
//!   - `TIME`    — seconds since Play mode began
//!   - `KEY_W/A/S/D/Q/E/SPACE/SHIFT/UP/DOWN/LEFT/RIGHT` — held bools
//!   - `AXIS_X`  — `+1` for D, `-1` for A, `0` otherwise
//!   - `AXIS_Z`  — `+1` for W (forward is `-Z`, script handles the sign)
//!   - `AXIS_V`  — `+1` for Space, `-1` for LeftShift
//!
//! The variable-bag shape was chosen over a fat `entity` object because
//! Rhai's scope is O(1) to read/write and the assignment grammar
//! (`pos_y = pos_y + DT`) reads closer to pseudo-code than method-chain
//! mutation would. When more components need scripting access, each one
//! gets its own flat prefix (`vel_x`, `body_mass`, …) rather than a
//! nested table — keeps auto-complete / error messages short.
//!
//! ## Determinism & hot-reload
//!
//! The AST cache is keyed by the resolved absolute `PathBuf` and
//! invalidated on `mtime` change, so saving a script in your editor
//! triggers a recompile on the next tick without touching Play mode
//! state. Compile failures and runtime errors both surface through
//! [`ScriptHost::drain_errors`]; gameplay continues with the failing
//! entity's transform untouched that frame.
//!
//! ## Safety caps
//!
//! The underlying [`rhai::Engine`] is built with
//! `set_max_operations(100_000)` so a runaway loop surfaces as a
//! [`ScriptError`] instead of freezing the editor. 100k is comfortably
//! above the bootstrap `spinner.rhai` cost measured at I-35; bump if
//! real gameplay scripts hit it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{Quat, Vec3};
use rhai::{Engine, AST};

use crate::input::{Input, Key};
use crate::world::{Script, Transform, World};

/// Error captured by the script host — compile failure, missing source
/// file, or runtime panic inside a script. Surfaced to the editor's
/// Console panel via [`ScriptHost::drain_errors`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptError {
    /// Project-relative source path (the `Script.source` string).
    /// Empty when the failure is host-level rather than per-script.
    pub path:    String,
    /// Human-readable message. Never includes absolute paths so log
    /// output stays portable across machines.
    pub message: String,
}

/// Owns the Rhai engine + per-path compiled-AST cache. Shared across
/// all scripted entities in a scene; lives in the editor (not the
/// runtime world) because it performs filesystem I/O and carries
/// editor-only state like the error log.
pub struct ScriptHost {
    engine:       Engine,
    cache:        HashMap<PathBuf, CachedScript>,
    errors:       Vec<ScriptError>,
    /// Accumulated Play-mode seconds, exposed to scripts as `TIME`.
    /// Reset to 0 by [`ScriptHost::reset_play_time`] whenever the
    /// editor enters Play mode — exiting and re-entering should give
    /// scripts a fresh clock so `TIME` doesn't jump wildly.
    elapsed_secs: f32,
}

struct CachedScript {
    ast:          AST,
    source_mtime: Option<std::time::SystemTime>,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptHost {
    /// Construct an empty host. The Rhai engine is configured with a
    /// conservative operation budget so a runaway script can't freeze
    /// the editor — see module docs.
    pub fn new() -> Self {
        let mut engine = Engine::new();
        engine.set_max_operations(100_000);
        Self {
            engine,
            cache: HashMap::new(),
            errors: Vec::new(),
            elapsed_secs: 0.0,
        }
    }

    /// Take the accumulated error log. Called by the editor each frame
    /// to drain new entries into the Console panel.
    pub fn drain_errors(&mut self) -> Vec<ScriptError> {
        std::mem::take(&mut self.errors)
    }

    /// Drop every cached AST. Called on project switch so a scene
    /// opened from a different project root doesn't get a stale
    /// compile from the previous one.
    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// Reset the Play-mode clock to zero. Editor calls this on entering
    /// Play mode so `TIME` starts fresh each session.
    pub fn reset_play_time(&mut self) {
        self.elapsed_secs = 0.0;
    }

    /// Current Play-mode elapsed seconds. Test / diagnostics hook.
    pub fn elapsed_secs(&self) -> f32 {
        self.elapsed_secs
    }

    /// Advance every `Script` entity in `world` by one tick.
    ///
    /// `project_root` anchors the script's `source` path so a scene
    /// component carrying `source = "scripts/spinner.rhai"` resolves
    /// against the currently-open project. `dt` is the frame delta in
    /// seconds; `input` is the same keyboard snapshot fed to
    /// [`World::tick_gameplay`] so scripts and gameplay systems see a
    /// consistent view of input.
    ///
    /// The loop is written in two passes so the borrow checker stays
    /// happy: we collect `(Entity, source)` pairs from an immutable
    /// query first, then do the compile + eval + transform-writeback
    /// with an unborrowed world. Cheap in practice — scripted entity
    /// counts stay tiny and the cloned `String` avoids a double-borrow
    /// dance.
    pub fn tick_world(
        &mut self,
        world: &mut World,
        input: &Input,
        dt: f32,
        project_root: &Path,
    ) {
        self.elapsed_secs += dt;

        // Pass 1: snapshot (entity, source) pairs. An open `query::<&Script>`
        // would conflict with the mutable transform writes later.
        let jobs: Vec<(hecs::Entity, String)> = world
            .ecs()
            .query::<&Script>()
            .iter()
            .map(|(entity, script)| (entity, script.source.clone()))
            .collect();

        if jobs.is_empty() {
            return;
        }

        let time_secs = self.elapsed_secs;
        for (entity, source_path) in jobs {
            self.tick_entity(world, entity, &source_path, input, dt, time_secs, project_root);
        }
    }

    /// Per-entity body of [`Self::tick_world`]. Extracted so the
    /// double-borrow of `self.cache` + `self.errors` stays local and
    /// doesn't infect the outer loop's control flow.
    fn tick_entity(
        &mut self,
        world: &mut World,
        entity: hecs::Entity,
        source_path: &str,
        input: &Input,
        dt: f32,
        time_secs: f32,
        project_root: &Path,
    ) {
        let abs_path = project_root.join(source_path);

        // Compile (or re-use cache) before touching the entity. Any
        // failure here records an error and bails — the transform is
        // left alone so a broken script doesn't teleport the entity.
        if !self.ensure_compiled(&abs_path, source_path) {
            return;
        }

        // Snapshot the current transform. Missing-transform is silent:
        // a Script on a pure-data entity (e.g. a manager node without a
        // Transform) is legal, but today's scope-binding contract only
        // knows how to round-trip transform fields, so nothing to do.
        let current = match world.ecs().get::<&Transform>(entity) {
            Ok(t) => *t,
            Err(_) => return,
        };
        let (rx, ry, rz) = current.rotation.to_euler(glam::EulerRot::XYZ);
        let mut scope = rhai::Scope::new();
        scope
            .push("pos_x", current.translation.x as f64)
            .push("pos_y", current.translation.y as f64)
            .push("pos_z", current.translation.z as f64)
            .push("rot_x", rx as f64)
            .push("rot_y", ry as f64)
            .push("rot_z", rz as f64)
            .push("scl_x", current.scale.x as f64)
            .push("scl_y", current.scale.y as f64)
            .push("scl_z", current.scale.z as f64)
            .push_constant("DT", dt as f64)
            .push_constant("TIME", time_secs as f64)
            .push_constant("KEY_W", input.pressed(Key::W))
            .push_constant("KEY_A", input.pressed(Key::A))
            .push_constant("KEY_S", input.pressed(Key::S))
            .push_constant("KEY_D", input.pressed(Key::D))
            .push_constant("KEY_Q", input.pressed(Key::Q))
            .push_constant("KEY_E", input.pressed(Key::E))
            .push_constant("KEY_SPACE", input.pressed(Key::Space))
            .push_constant("KEY_SHIFT", input.pressed(Key::LeftShift))
            .push_constant("KEY_UP", input.pressed(Key::ArrowUp))
            .push_constant("KEY_DOWN", input.pressed(Key::ArrowDown))
            .push_constant("KEY_LEFT", input.pressed(Key::ArrowLeft))
            .push_constant("KEY_RIGHT", input.pressed(Key::ArrowRight))
            .push_constant("AXIS_X", input.axis(Key::A, Key::D) as f64)
            .push_constant("AXIS_Z", input.axis(Key::S, Key::W) as f64)
            .push_constant("AXIS_V", input.axis(Key::LeftShift, Key::Space) as f64);

        // Run. An error here leaves the transform untouched — a script
        // that blew up mid-frame shouldn't also destroy the entity's
        // pose by committing half-updated scope vars.
        let run_result = {
            let Some(cached) = self.cache.get(&abs_path) else {
                return;
            };
            self.engine.run_ast_with_scope(&mut scope, &cached.ast)
        };
        if let Err(err) = run_result {
            self.errors.push(ScriptError {
                path:    source_path.to_string(),
                message: err.to_string(),
            });
            return;
        }

        // Read back. Fallbacks to the prior value mean a script can
        // `#[reflect(skip)]`-style ignore a field by simply not writing
        // it — the scope still carried the original.
        let read = |scope: &rhai::Scope, name: &str, fallback: f32| -> f32 {
            scope
                .get_value::<f64>(name)
                .map(|v| v as f32)
                .unwrap_or(fallback)
        };
        let new_t = Vec3::new(
            read(&scope, "pos_x", current.translation.x),
            read(&scope, "pos_y", current.translation.y),
            read(&scope, "pos_z", current.translation.z),
        );
        let new_r = Quat::from_euler(
            glam::EulerRot::XYZ,
            read(&scope, "rot_x", rx),
            read(&scope, "rot_y", ry),
            read(&scope, "rot_z", rz),
        );
        let new_s = Vec3::new(
            read(&scope, "scl_x", current.scale.x),
            read(&scope, "scl_y", current.scale.y),
            read(&scope, "scl_z", current.scale.z),
        );

        if let Ok(mut transform) = world.ecs_mut().get::<&mut Transform>(entity) {
            transform.translation = new_t;
            transform.rotation = new_r;
            transform.scale = new_s;
        }
    }

    /// Returns `true` if a valid compiled AST is in-cache afterward.
    /// Re-compiles when the on-disk mtime changed, providing
    /// save-to-reload without bouncing Play mode.
    fn ensure_compiled(&mut self, abs_path: &Path, source_path: &str) -> bool {
        let mtime = std::fs::metadata(abs_path)
            .ok()
            .and_then(|m| m.modified().ok());
        let needs_compile = match self.cache.get(abs_path) {
            Some(cached) => cached.source_mtime != mtime,
            None => true,
        };
        if !needs_compile {
            return true;
        }

        let source = match std::fs::read_to_string(abs_path) {
            Ok(s) => s,
            Err(err) => {
                self.errors.push(ScriptError {
                    path:    source_path.to_string(),
                    message: format!("failed to read script: {err}"),
                });
                // Purge any stale cache entry so we retry from scratch
                // the moment the file appears.
                self.cache.remove(abs_path);
                return false;
            }
        };
        match self.engine.compile(&source) {
            Ok(ast) => {
                self.cache
                    .insert(abs_path.to_path_buf(), CachedScript { ast, source_mtime: mtime });
                true
            }
            Err(err) => {
                self.errors.push(ScriptError {
                    path:    source_path.to_string(),
                    message: format!("compile error: {err}"),
                });
                // Drop any previously-good AST so we don't keep running
                // stale bytecode after the user saves a broken file.
                self.cache.remove(abs_path);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;
    use crate::scene::{ComponentData, IdAllocator, PrimitiveValue, SceneDocument, SceneEntity};

    fn fixture_dir() -> PathBuf {
        // Per-test fixture directory under the OS temp dir. Using a
        // unique nanoseconds-derived subdir avoids cross-test collisions
        // without pulling in `tempfile`.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("rustforge-scripting-{pid}-{n}"));
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        dir
    }

    fn world_with_scripted_cube(source: &str) -> (World, hecs::Entity) {
        // Build a scene doc with a scripted cube, instantiate, and
        // return the runtime entity handle so tests can inspect the
        // Transform after tick_world.
        let mut ids = IdAllocator::default();
        let doc = SceneDocument::new("t").with_root(
            SceneEntity::new(ids.next(), "Cube")
                .with_component(ComponentData::new("Transform"))
                .with_component(ComponentData::new("Script").with_field(
                    "source",
                    PrimitiveValue::String(source.to_string()),
                )),
        );
        let mut world = World::new();
        let mapping = world.instantiate_scene(&doc);
        let scene_id = doc.root_entities[0].id;
        let entity = mapping.entity(scene_id).unwrap();
        (world, entity)
    }

    #[test]
    fn new_host_has_no_errors() {
        let mut host = ScriptHost::new();
        assert!(host.drain_errors().is_empty());
    }

    #[test]
    fn clear_cache_is_idempotent() {
        let mut host = ScriptHost::new();
        host.clear_cache();
        host.clear_cache();
    }

    #[test]
    fn tick_world_writes_pos_y() {
        // A script that unconditionally assigns pos_y proves the
        // round-trip scope → transform works end-to-end.
        let root = fixture_dir();
        std::fs::write(root.join("scripts/bump.rhai"), "pos_y = 7.0;").unwrap();
        let (mut world, entity) = world_with_scripted_cube("scripts/bump.rhai");
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &Input::new(), 0.016, &root);
        let t = *world.ecs().get::<&Transform>(entity).unwrap();
        assert!((t.translation.y - 7.0).abs() < 1e-5, "got y = {}", t.translation.y);
        assert!(host.drain_errors().is_empty());
    }

    #[test]
    fn tick_world_accumulates_dt_into_rotation() {
        // Two ticks of `rot_y += DT` at dt=0.5 should add up to ~1.0 rad.
        let root = fixture_dir();
        std::fs::write(root.join("scripts/spin.rhai"), "rot_y = rot_y + DT;").unwrap();
        let (mut world, entity) = world_with_scripted_cube("scripts/spin.rhai");
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &Input::new(), 0.5, &root);
        host.tick_world(&mut world, &Input::new(), 0.5, &root);
        let t = *world.ecs().get::<&Transform>(entity).unwrap();
        let (_, y, _) = t.rotation.to_euler(glam::EulerRot::XYZ);
        assert!((y - 1.0).abs() < 1e-4, "got rot_y = {y}");
    }

    #[test]
    fn tick_world_respects_input_axis() {
        // AXIS_X is the same signed-pair helper `Mover` uses; a script
        // that integrates AXIS_X * DT should drift +X when D is held.
        let root = fixture_dir();
        std::fs::write(
            root.join("scripts/slide.rhai"),
            "pos_x = pos_x + AXIS_X * DT;",
        )
        .unwrap();
        let (mut world, entity) = world_with_scripted_cube("scripts/slide.rhai");
        let mut input = Input::new();
        input.press(Key::D);
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &input, 0.25, &root);
        let t = *world.ecs().get::<&Transform>(entity).unwrap();
        assert!((t.translation.x - 0.25).abs() < 1e-5, "got x = {}", t.translation.x);
    }

    #[test]
    fn tick_world_records_compile_error_once() {
        // A syntactically broken script should surface a ScriptError
        // and leave the transform untouched. Re-running without fixing
        // the file must not push the same error every tick forever —
        // we expect one entry per run_ast failure though, so that
        // hot-reload from broken-to-fixed shows progress in the console.
        let root = fixture_dir();
        std::fs::write(root.join("scripts/bad.rhai"), "let x = ;").unwrap();
        let (mut world, _) = world_with_scripted_cube("scripts/bad.rhai");
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &Input::new(), 0.016, &root);
        let errs = host.drain_errors();
        assert_eq!(errs.len(), 1);
        assert!(errs[0].message.contains("compile error"));
        assert_eq!(errs[0].path, "scripts/bad.rhai");
    }

    #[test]
    fn tick_world_records_runtime_error() {
        // `throw` is the idiomatic Rhai trigger for a runtime error —
        // parses fine, explodes at eval. Exactly the case we want to
        // surface without aborting the rest of the scene.
        let root = fixture_dir();
        std::fs::write(root.join("scripts/boom.rhai"), "throw \"nope\";").unwrap();
        let (mut world, _) = world_with_scripted_cube("scripts/boom.rhai");
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &Input::new(), 0.016, &root);
        let errs = host.drain_errors();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].path, "scripts/boom.rhai");
    }

    #[test]
    fn tick_world_missing_file_records_error_and_retries() {
        // Pointing at a non-existent file records an error; creating
        // the file afterwards should let the next tick pick it up
        // without needing a clear_cache call.
        let root = fixture_dir();
        let (mut world, entity) = world_with_scripted_cube("scripts/late.rhai");
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &Input::new(), 0.016, &root);
        assert_eq!(host.drain_errors().len(), 1);
        // Create the file; next tick should succeed and mutate y.
        std::fs::write(root.join("scripts/late.rhai"), "pos_y = 2.0;").unwrap();
        host.tick_world(&mut world, &Input::new(), 0.016, &root);
        let t = *world.ecs().get::<&Transform>(entity).unwrap();
        assert!((t.translation.y - 2.0).abs() < 1e-5);
        assert!(host.drain_errors().is_empty());
    }

    #[test]
    fn runaway_script_trips_operation_budget() {
        // Infinite loop must surface as a ScriptError (thanks to
        // `set_max_operations`) instead of freezing the host.
        let root = fixture_dir();
        std::fs::write(root.join("scripts/hang.rhai"), "loop {}").unwrap();
        let (mut world, _) = world_with_scripted_cube("scripts/hang.rhai");
        let mut host = ScriptHost::new();
        host.tick_world(&mut world, &Input::new(), 0.016, &root);
        let errs = host.drain_errors();
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn reset_play_time_zeros_clock() {
        let mut host = ScriptHost::new();
        host.elapsed_secs = 42.0;
        host.reset_play_time();
        assert_eq!(host.elapsed_secs(), 0.0);
    }
}
