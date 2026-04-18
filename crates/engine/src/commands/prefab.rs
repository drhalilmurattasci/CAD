//! I-19: undoable prefab instantiation.
//!
//! `SpawnPrefabCommand` is the counterpart to `SpawnEntityCommand` for
//! prefab assets — it materialises a `PrefabDocument` into a fresh
//! `SceneEntity` subtree (with scene-space ids) at construction, then
//! applies/undoes that subtree just like a regular spawn.
//!
//! Materialising up-front rather than per-apply is deliberate:
//! * Redo must land the entity at the same ids it originally had, so
//!   references from other commands (selection, transform edits) stay
//!   valid across undo/redo cycles.
//! * The `IdAllocator` lives on the editor shell — we don't want to
//!   re-enter it at redo time.

use crate::scene::{IdAllocator, PrefabDocument, SceneDocument, SceneEntity, SceneId};

use super::{Command, CommandError};

pub struct SpawnPrefabCommand {
    parent_id: Option<SceneId>,
    instance:  SceneEntity,
    spawned:   bool,
}

impl SpawnPrefabCommand {
    /// Materialise `prefab` into a fresh instance using `ids`, ready
    /// to be executed against a scene. `name_suffix` is appended to
    /// the root name (useful for "Enemy (2)" auto-numbering).
    pub fn new(
        parent_id: Option<SceneId>,
        prefab: &PrefabDocument,
        ids: &mut IdAllocator,
        name_suffix: Option<&str>,
    ) -> Self {
        Self {
            parent_id,
            instance: prefab.instantiate(ids, name_suffix),
            spawned:  false,
        }
    }

    /// The id of the root of the materialised instance. Useful for the
    /// caller to grab so it can auto-select the spawned prefab.
    pub fn root_id(&self) -> SceneId {
        self.instance.id
    }
}

impl Command<SceneDocument, CommandError> for SpawnPrefabCommand {
    fn label(&self) -> &'static str {
        "prefab.spawn"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        if self.spawned {
            return Err(CommandError::InvalidState(
                "prefab spawn cannot be applied twice without undo",
            ));
        }

        if let Some(parent_id) = self.parent_id {
            let parent = scene
                .find_entity_mut(parent_id)
                .ok_or(CommandError::MissingEntity(parent_id))?;
            parent.children.push(self.instance.clone());
        } else {
            scene.root_entities.push(self.instance.clone());
        }
        self.spawned = true;
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        if !self.spawned {
            return Err(CommandError::InvalidState(
                "prefab spawn was never applied",
            ));
        }

        let removed = if let Some(parent_id) = self.parent_id {
            let parent = scene
                .find_entity_mut(parent_id)
                .ok_or(CommandError::MissingEntity(parent_id))?;
            remove_entity(&mut parent.children, self.instance.id)
        } else {
            remove_entity(&mut scene.root_entities, self.instance.id)
        };

        if !removed {
            return Err(CommandError::MissingEntity(self.instance.id));
        }
        self.spawned = false;
        Ok(())
    }
}

fn remove_entity(entities: &mut Vec<SceneEntity>, id: SceneId) -> bool {
    if let Some(index) = entities.iter().position(|e| e.id == id) {
        entities.remove(index);
        return true;
    }
    for entity in entities {
        if remove_entity(&mut entity.children, id) {
            return true;
        }
    }
    false
}
