use crate::scene::{PrimitiveValue, SceneDocument, SceneId};

use super::{Command, CommandError};

pub struct SetComponentFieldCommand {
    entity_id: SceneId,
    component_type: String,
    field_name: String,
    new_value: PrimitiveValue,
    previous_value: Option<Option<PrimitiveValue>>,
}

impl SetComponentFieldCommand {
    pub fn new(
        entity_id: SceneId,
        component_type: impl Into<String>,
        field_name: impl Into<String>,
        new_value: PrimitiveValue,
    ) -> Self {
        Self {
            entity_id,
            component_type: component_type.into(),
            field_name: field_name.into(),
            new_value,
            previous_value: None,
        }
    }
}

impl Command<SceneDocument, CommandError> for SetComponentFieldCommand {
    fn label(&self) -> &'static str {
        "component.set_field"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let entity = scene
            .find_entity_mut(self.entity_id)
            .ok_or(CommandError::MissingEntity(self.entity_id))?;
        let component = entity
            .components
            .iter_mut()
            .find(|component| component.type_name == self.component_type)
            .ok_or(CommandError::InvalidState("component was not found"))?;

        if self.previous_value.is_none() {
            self.previous_value = Some(component.fields.get(&self.field_name).cloned());
        }

        component
            .fields
            .insert(self.field_name.clone(), self.new_value.clone());
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let entity = scene
            .find_entity_mut(self.entity_id)
            .ok_or(CommandError::MissingEntity(self.entity_id))?;
        let component = entity
            .components
            .iter_mut()
            .find(|component| component.type_name == self.component_type)
            .ok_or(CommandError::InvalidState("component was not found"))?;
        let previous = self
            .previous_value
            .clone()
            .ok_or(CommandError::InvalidState("component command was never applied"))?;

        match previous {
            Some(previous_value) => {
                component
                    .fields
                    .insert(self.field_name.clone(), previous_value);
            }
            None => {
                component.fields.remove(&self.field_name);
            }
        }

        Ok(())
    }
}
