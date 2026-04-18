use crate::scene::{PrimitiveValue, SceneDocument, SceneId};

use super::{Command, CommandError};

pub struct NudgeTransformCommand {
    entity_id: SceneId,
    dx: f64,
    dy: f64,
    dz: f64,
    previous: Option<TranslationSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct TranslationSnapshot {
    x: Option<PrimitiveValue>,
    y: Option<PrimitiveValue>,
    z: Option<PrimitiveValue>,
}

impl NudgeTransformCommand {
    pub fn new(entity_id: SceneId, dx: f64, dy: f64, dz: f64) -> Self {
        Self {
            entity_id,
            dx,
            dy,
            dz,
            previous: None,
        }
    }
}

impl Command<SceneDocument, CommandError> for NudgeTransformCommand {
    fn label(&self) -> &'static str {
        "transform.nudge"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let transform = transform_component_mut(scene, self.entity_id)?;

        if self.previous.is_none() {
            self.previous = Some(TranslationSnapshot {
                x: transform.fields.get("x").cloned(),
                y: transform.fields.get("y").cloned(),
                z: transform.fields.get("z").cloned(),
            });
        }

        apply_delta(&mut transform.fields, "x", self.dx);
        apply_delta(&mut transform.fields, "y", self.dy);
        apply_delta(&mut transform.fields, "z", self.dz);
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let transform = transform_component_mut(scene, self.entity_id)?;
        let previous = self
            .previous
            .clone()
            .ok_or(CommandError::InvalidState("transform command was never applied"))?;

        restore_field(&mut transform.fields, "x", previous.x);
        restore_field(&mut transform.fields, "y", previous.y);
        restore_field(&mut transform.fields, "z", previous.z);
        Ok(())
    }
}

/// I-16: rotate an entity around one or more Euler axes (radians).
///
/// We store deltas on each Euler field (`rot_x`, `rot_y`, `rot_z`) and
/// let the runtime re-compose the quaternion in `extract_transform`.
/// This keeps the scene document purely additive: a rotate-by-15°
/// button only touches the axis it cares about, and repeated presses
/// accumulate the same way translation nudges do.
pub struct RotateTransformCommand {
    entity_id: SceneId,
    d_rot_x: f64,
    d_rot_y: f64,
    d_rot_z: f64,
    previous: Option<RotationSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct RotationSnapshot {
    rot_x: Option<PrimitiveValue>,
    rot_y: Option<PrimitiveValue>,
    rot_z: Option<PrimitiveValue>,
}

impl RotateTransformCommand {
    pub fn new(entity_id: SceneId, d_rot_x: f64, d_rot_y: f64, d_rot_z: f64) -> Self {
        Self {
            entity_id,
            d_rot_x,
            d_rot_y,
            d_rot_z,
            previous: None,
        }
    }
}

impl Command<SceneDocument, CommandError> for RotateTransformCommand {
    fn label(&self) -> &'static str {
        "transform.rotate"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let transform = transform_component_mut(scene, self.entity_id)?;

        if self.previous.is_none() {
            self.previous = Some(RotationSnapshot {
                rot_x: transform.fields.get("rot_x").cloned(),
                rot_y: transform.fields.get("rot_y").cloned(),
                rot_z: transform.fields.get("rot_z").cloned(),
            });
        }

        apply_delta(&mut transform.fields, "rot_x", self.d_rot_x);
        apply_delta(&mut transform.fields, "rot_y", self.d_rot_y);
        apply_delta(&mut transform.fields, "rot_z", self.d_rot_z);
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let transform = transform_component_mut(scene, self.entity_id)?;
        let previous = self
            .previous
            .clone()
            .ok_or(CommandError::InvalidState("rotate command was never applied"))?;

        restore_field(&mut transform.fields, "rot_x", previous.rot_x);
        restore_field(&mut transform.fields, "rot_y", previous.rot_y);
        restore_field(&mut transform.fields, "rot_z", previous.rot_z);
        Ok(())
    }
}

/// I-17: multiplicative uniform scale command.
///
/// The Transform component currently exposes a single uniform `scale`
/// field (vector scale arrives with the reflection derive in I-6), so
/// we apply a factor rather than a per-axis delta. `factor = 1.0` is
/// the identity. Undo divides by the same factor — we snapshot the
/// original value to avoid accumulating float drift across undo/redo.
pub struct ScaleTransformCommand {
    entity_id: SceneId,
    factor: f64,
    previous: Option<ScaleSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct ScaleSnapshot {
    scale: Option<PrimitiveValue>,
}

impl ScaleTransformCommand {
    pub fn new(entity_id: SceneId, factor: f64) -> Self {
        Self {
            entity_id,
            factor,
            previous: None,
        }
    }
}

impl Command<SceneDocument, CommandError> for ScaleTransformCommand {
    fn label(&self) -> &'static str {
        "transform.scale"
    }

    fn apply(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        if self.factor == 0.0 {
            return Err(CommandError::InvalidState(
                "scale factor must be non-zero to stay invertible",
            ));
        }

        let transform = transform_component_mut(scene, self.entity_id)?;

        if self.previous.is_none() {
            self.previous = Some(ScaleSnapshot {
                scale: transform.fields.get("scale").cloned(),
            });
        }

        apply_factor(&mut transform.fields, "scale", self.factor, 1.0);
        Ok(())
    }

    fn undo(&mut self, scene: &mut SceneDocument) -> Result<(), CommandError> {
        let transform = transform_component_mut(scene, self.entity_id)?;
        let previous = self
            .previous
            .clone()
            .ok_or(CommandError::InvalidState("scale command was never applied"))?;

        restore_field(&mut transform.fields, "scale", previous.scale);
        Ok(())
    }
}

fn transform_component_mut(
    scene: &mut SceneDocument,
    entity_id: SceneId,
) -> Result<&mut crate::scene::ComponentData, CommandError> {
    let entity = scene
        .find_entity_mut(entity_id)
        .ok_or(CommandError::MissingEntity(entity_id))?;
    entity
        .components
        .iter_mut()
        .find(|component| component.type_name == "Transform")
        .ok_or(CommandError::InvalidState("transform component was not found"))
}

fn apply_delta(
    fields: &mut std::collections::BTreeMap<String, PrimitiveValue>,
    field_name: &str,
    delta: f64,
) {
    let current = match fields.get(field_name) {
        Some(PrimitiveValue::F64(value)) => *value,
        Some(PrimitiveValue::I64(value)) => *value as f64,
        _ => 0.0,
    };
    fields.insert(field_name.to_owned(), PrimitiveValue::F64(current + delta));
}

fn apply_factor(
    fields: &mut std::collections::BTreeMap<String, PrimitiveValue>,
    field_name: &str,
    factor: f64,
    default: f64,
) {
    let current = match fields.get(field_name) {
        Some(PrimitiveValue::F64(value)) => *value,
        Some(PrimitiveValue::I64(value)) => *value as f64,
        _ => default,
    };
    fields.insert(field_name.to_owned(), PrimitiveValue::F64(current * factor));
}

fn restore_field(
    fields: &mut std::collections::BTreeMap<String, PrimitiveValue>,
    field_name: &str,
    previous: Option<PrimitiveValue>,
) {
    match previous {
        Some(value) => {
            fields.insert(field_name.to_owned(), value);
        }
        None => {
            fields.remove(field_name);
        }
    }
}
