//! I-20: play-in-editor state isolation.
//!
//! Entering Play mode forks the authoring `SceneDocument` so that
//! gameplay mutations (spawned bullets, moved enemies, destroyed
//! props) never leak back into the saved project. A `PlayModeSession`
//! holds the snapshot taken at play-start; dropping it via `end()`
//! restores the authoring scene verbatim.
//!
//! Scope for this landing:
//! * Only the scene document is snapshotted. Command history is
//!   cleared on entry — game-time edits must not be undoable into
//!   the edit-time timeline (that would silently re-apply spawned
//!   bullets onto the authored scene after stop, corrupting it).
//! * Runtime ECS state (transient components, light flags, etc.) is
//!   rebuilt by the caller via `World::resync_transforms_from_scene`
//!   after `begin`/`end` — the session doesn't own the world.
//!
//! Future layers (deterministic replay, step-by-step debugger) can
//! stack on top: the session would gain a serialized frame log and a
//! rewind API, but the snapshot-and-restore contract stays.

use crate::scene::SceneDocument;

#[derive(Debug, Clone, PartialEq)]
pub struct PlayModeSession {
    authoring_scene: SceneDocument,
}

impl PlayModeSession {
    /// Take an isolated snapshot of `scene`. The caller now owns both
    /// the (mutable) live scene and this session; gameplay freely
    /// mutates the former, then calls `end` to restore.
    pub fn begin(scene: &SceneDocument) -> Self {
        Self {
            authoring_scene: scene.clone(),
        }
    }

    /// Consume the session, writing the authoring snapshot back into
    /// `scene`. After this call, `scene` is byte-identical to what it
    /// was when `begin` ran — gameplay mutations are gone, authored
    /// state is whole.
    pub fn end(self, scene: &mut SceneDocument) {
        *scene = self.authoring_scene;
    }

    /// Peek at the stored snapshot. Useful for diagnostics / tests;
    /// the live scene is always the source of truth for the UI.
    pub fn authoring_scene(&self) -> &SceneDocument {
        &self.authoring_scene
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{ComponentData, IdAllocator, PrimitiveValue, SceneEntity};

    fn sandbox() -> SceneDocument {
        let mut ids = IdAllocator::default();
        SceneDocument::new("Sandbox").with_root(
            SceneEntity::new(ids.next(), "Player").with_component(
                ComponentData::new("Transform")
                    .with_field("x", PrimitiveValue::F64(0.0))
                    .with_field("y", PrimitiveValue::F64(0.0)),
            ),
        )
    }

    #[test]
    fn session_end_restores_scene_verbatim() {
        let mut scene = sandbox();
        let session = PlayModeSession::begin(&scene);

        // Simulate gameplay: rename the player and spawn a bullet
        // entity. Both are pure scene-document edits (no commands).
        scene.root_entities[0].name = "Player (ingame)".into();
        let mut ids = IdAllocator::new(50);
        scene
            .root_entities
            .push(SceneEntity::new(ids.next(), "Bullet"));

        // Mid-session the snapshot is untouched.
        assert_eq!(session.authoring_scene().root_entities.len(), 1);
        assert_eq!(session.authoring_scene().root_entities[0].name, "Player");

        // After end() the live scene matches the snapshot exactly.
        session.end(&mut scene);
        assert_eq!(scene, sandbox());
    }

    #[test]
    fn session_snapshot_is_independent_clone() {
        // Mutating the live scene after begin() must not reflect in
        // the snapshot — otherwise the restore contract is broken.
        let mut scene = sandbox();
        let session = PlayModeSession::begin(&scene);
        scene.root_entities[0].name = "Mutated".into();
        assert_eq!(session.authoring_scene().root_entities[0].name, "Player");
    }
}
