use crate::scene::{SceneDocument, SceneEntity, SceneId};

use super::{Command, CommandError};

pub struct RenameEntityCommand {
    entity_id: SceneId,
    new_name: String,
    previous_name: Option<String>,
}

impl RenameEntityCommand {
    pub fn new(entity_id: SceneId, new_name: impl Into<String>) -> Self {
        Self {
            entity_id,
            new_name: new_name.into(),
            previous_name: None,
        }
    }
}

impl Command<SceneDocument, CommandError> for RenameEntityCommand {
    fn label(&self) -> &'static str {
        "entity.rename"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let entity = scene
            .find_entity_mut(self.entity_id)
            .ok_or(CommandError::MissingEntity(self.entity_id))?;

        if self.previous_name.is_none() {
            self.previous_name = Some(entity.name.clone());
        }

        entity.name = self.new_name.clone();
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let entity = scene
            .find_entity_mut(self.entity_id)
            .ok_or(CommandError::MissingEntity(self.entity_id))?;
        let previous = self
            .previous_name
            .clone()
            .ok_or(CommandError::InvalidState("rename command was never applied"))?;
        entity.name = previous;
        Ok(())
    }
}

pub struct SpawnEntityCommand {
    parent_id: Option<SceneId>,
    entity: SceneEntity,
    spawned: bool,
}

impl SpawnEntityCommand {
    pub fn new(parent_id: Option<SceneId>, entity: SceneEntity) -> Self {
        Self {
            parent_id,
            entity,
            spawned: false,
        }
    }
}

impl Command<SceneDocument, CommandError> for SpawnEntityCommand {
    fn label(&self) -> &'static str {
        "entity.spawn"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        if self.spawned {
            return Err(CommandError::InvalidState(
                "spawn command cannot be applied twice without undo",
            ));
        }

        if let Some(parent_id) = self.parent_id {
            let parent = scene
                .find_entity_mut(parent_id)
                .ok_or(CommandError::MissingEntity(parent_id))?;
            parent.children.push(self.entity.clone());
        } else {
            scene.root_entities.push(self.entity.clone());
        }
        self.spawned = true;
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        if !self.spawned {
            return Err(CommandError::InvalidState("spawn command was never applied"));
        }

        let removed = if let Some(parent_id) = self.parent_id {
            let parent = scene
                .find_entity_mut(parent_id)
                .ok_or(CommandError::MissingEntity(parent_id))?;
            remove_entity(&mut parent.children, self.entity.id)
        } else {
            remove_entity(&mut scene.root_entities, self.entity.id)
        };

        if !removed {
            return Err(CommandError::MissingEntity(self.entity.id));
        }

        self.spawned = false;
        Ok(())
    }
}

fn remove_entity(entities: &mut Vec<SceneEntity>, id: SceneId) -> bool {
    if let Some(index) = entities.iter().position(|entity| entity.id == id) {
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
