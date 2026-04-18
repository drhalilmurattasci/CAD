mod component;
mod entity;
mod prefab;
mod stack;
mod transform;

pub use component::SetComponentFieldCommand;
pub use entity::{RenameEntityCommand, SpawnEntityCommand};
pub use prefab::SpawnPrefabCommand;
pub use stack::{Command, CommandError, CommandStack};
pub use transform::{NudgeTransformCommand, RotateTransformCommand, ScaleTransformCommand};

#[cfg(test)]
mod tests {
    use super::{
        CommandStack, NudgeTransformCommand, RenameEntityCommand, RotateTransformCommand,
        ScaleTransformCommand, SetComponentFieldCommand, SpawnEntityCommand, SpawnPrefabCommand,
    };
    use crate::scene::{
        ComponentData, IdAllocator, PrefabDocument, PrimitiveValue, SceneDocument, SceneEntity,
    };

    #[test]
    fn rename_command_roundtrips_through_undo_redo() {
        let mut ids = IdAllocator::default();
        let entity = SceneEntity::new(ids.next(), "Cube").with_component(ComponentData::new("Transform"));
        let entity_id = entity.id;
        let mut scene = SceneDocument::new("Test").with_root(entity);
        let mut stack = CommandStack::default();

        stack
            .execute(
                &mut scene,
                Box::new(RenameEntityCommand::new(entity_id, "Renamed Cube")),
            )
            .unwrap();

        assert_eq!(scene.find_entity(entity_id).unwrap().name, "Renamed Cube");
        assert_eq!(stack.undo_len(), 1);
        assert_eq!(stack.redo_len(), 0);

        assert!(stack.undo(&mut scene).unwrap());
        assert_eq!(scene.find_entity(entity_id).unwrap().name, "Cube");

        assert!(stack.redo(&mut scene).unwrap());
        assert_eq!(scene.find_entity(entity_id).unwrap().name, "Renamed Cube");
    }

    #[test]
    fn spawn_command_adds_and_removes_root_entities() {
        let mut ids = IdAllocator::default();
        let mut scene = SceneDocument::new("Test");
        let mut stack = CommandStack::default();
        let spawned = SceneEntity::new(ids.next(), "Spawned");
        let spawned_id = spawned.id;

        stack
            .execute(&mut scene, Box::new(SpawnEntityCommand::new(None, spawned)))
            .unwrap();

        assert!(scene.find_entity(spawned_id).is_some());
        assert!(stack.undo(&mut scene).unwrap());
        assert!(scene.find_entity(spawned_id).is_none());
    }

    #[test]
    fn component_field_command_roundtrips_values() {
        let mut ids = IdAllocator::default();
        let entity = SceneEntity::new(ids.next(), "Light").with_component(
            ComponentData::new("Light").with_field("intensity", PrimitiveValue::F64(4500.0)),
        );
        let entity_id = entity.id;
        let mut scene = SceneDocument::new("Test").with_root(entity);
        let mut stack = CommandStack::default();

        stack
            .execute(
                &mut scene,
                Box::new(SetComponentFieldCommand::new(
                    entity_id,
                    "Light",
                    "intensity",
                    PrimitiveValue::F64(9000.0),
                )),
            )
            .unwrap();

        let value = scene.find_entity(entity_id).unwrap().components[0]
            .fields
            .get("intensity")
            .unwrap();
        assert_eq!(value, &PrimitiveValue::F64(9000.0));

        assert!(stack.undo(&mut scene).unwrap());
        let value = scene.find_entity(entity_id).unwrap().components[0]
            .fields
            .get("intensity")
            .unwrap();
        assert_eq!(value, &PrimitiveValue::F64(4500.0));
    }

    #[test]
    fn transform_nudge_command_roundtrips_position() {
        let mut ids = IdAllocator::default();
        let entity = SceneEntity::new(ids.next(), "Camera").with_component(
            ComponentData::new("Transform")
                .with_field("x", PrimitiveValue::F64(0.0))
                .with_field("y", PrimitiveValue::F64(1.0))
                .with_field("z", PrimitiveValue::F64(-3.0)),
        );
        let entity_id = entity.id;
        let mut scene = SceneDocument::new("Test").with_root(entity);
        let mut stack = CommandStack::default();

        stack
            .execute(
                &mut scene,
                Box::new(NudgeTransformCommand::new(entity_id, 1.5, 0.0, -0.5)),
            )
            .unwrap();

        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        assert_eq!(fields.get("x"), Some(&PrimitiveValue::F64(1.5)));
        assert_eq!(fields.get("z"), Some(&PrimitiveValue::F64(-3.5)));

        assert!(stack.undo(&mut scene).unwrap());
        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        assert_eq!(fields.get("x"), Some(&PrimitiveValue::F64(0.0)));
        assert_eq!(fields.get("z"), Some(&PrimitiveValue::F64(-3.0)));
    }

    #[test]
    fn transform_rotate_command_roundtrips_euler_fields() {
        let mut ids = IdAllocator::default();
        let entity = SceneEntity::new(ids.next(), "Spinner").with_component(
            ComponentData::new("Transform")
                .with_field("rot_y", PrimitiveValue::F64(0.25)),
        );
        let entity_id = entity.id;
        let mut scene = SceneDocument::new("Test").with_root(entity);
        let mut stack = CommandStack::default();

        stack
            .execute(
                &mut scene,
                Box::new(RotateTransformCommand::new(entity_id, 0.1, 0.2, 0.0)),
            )
            .unwrap();

        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        assert_eq!(fields.get("rot_x"), Some(&PrimitiveValue::F64(0.1)));
        assert_eq!(fields.get("rot_y"), Some(&PrimitiveValue::F64(0.45)));
        // rot_z started absent, rotate-by-zero keeps it materialized
        // at 0.0 — that's fine, it's the neutral element.
        assert_eq!(fields.get("rot_z"), Some(&PrimitiveValue::F64(0.0)));

        assert!(stack.undo(&mut scene).unwrap());
        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        // rot_x / rot_z were absent before apply — undo removes them
        // rather than leaving zero-valued noise behind.
        assert!(fields.get("rot_x").is_none());
        assert_eq!(fields.get("rot_y"), Some(&PrimitiveValue::F64(0.25)));
        assert!(fields.get("rot_z").is_none());

        assert!(stack.redo(&mut scene).unwrap());
        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        assert_eq!(fields.get("rot_y"), Some(&PrimitiveValue::F64(0.45)));
    }

    #[test]
    fn transform_scale_command_roundtrips_uniform_factor() {
        let mut ids = IdAllocator::default();
        let entity = SceneEntity::new(ids.next(), "Box").with_component(
            ComponentData::new("Transform").with_field("scale", PrimitiveValue::F64(2.0)),
        );
        let entity_id = entity.id;
        let mut scene = SceneDocument::new("Test").with_root(entity);
        let mut stack = CommandStack::default();

        stack
            .execute(
                &mut scene,
                Box::new(ScaleTransformCommand::new(entity_id, 1.5)),
            )
            .unwrap();

        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        assert_eq!(fields.get("scale"), Some(&PrimitiveValue::F64(3.0)));

        assert!(stack.undo(&mut scene).unwrap());
        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        // Undo snapshots the original, so we get the exact 2.0 back
        // — no `3.0 / 1.5` float drift.
        assert_eq!(fields.get("scale"), Some(&PrimitiveValue::F64(2.0)));

        assert!(stack.redo(&mut scene).unwrap());
        let fields = &scene.find_entity(entity_id).unwrap().components[0].fields;
        assert_eq!(fields.get("scale"), Some(&PrimitiveValue::F64(3.0)));
    }

    #[test]
    fn prefab_spawn_command_adds_tree_and_undo_removes_it() {
        // Build a small prefab with a child so we cover the
        // recursive-id-remap path too.
        let mut template_ids = IdAllocator::default();
        let prefab = PrefabDocument::new(
            SceneEntity::new(template_ids.next(), "Enemy")
                .with_component(ComponentData::new("Transform"))
                .with_child(SceneEntity::new(template_ids.next(), "Sword")),
        );

        let mut ids = IdAllocator::new(100);
        let mut scene = SceneDocument::new("Stage");
        let mut stack = CommandStack::default();

        let cmd = SpawnPrefabCommand::new(None, &prefab, &mut ids, Some(" (1)"));
        let root_id = cmd.root_id();
        stack.execute(&mut scene, Box::new(cmd)).unwrap();

        let inserted = scene.find_entity(root_id).expect("prefab instance lives");
        assert_eq!(inserted.name, "Enemy (1)");
        assert_eq!(inserted.children.len(), 1);
        assert_eq!(inserted.children[0].name, "Sword");
        // Child must have a scene-space id, distinct from root.
        let child_id = inserted.children[0].id;
        assert!(child_id.0 > 100);
        assert_ne!(child_id, root_id);

        // Undo removes the whole subtree.
        assert!(stack.undo(&mut scene).unwrap());
        assert!(scene.find_entity(root_id).is_none());
        assert!(scene.find_entity(child_id).is_none());

        // Redo restores with identical ids (not re-allocated).
        assert!(stack.redo(&mut scene).unwrap());
        let restored = scene.find_entity(root_id).expect("redo restores root");
        assert_eq!(restored.children[0].id, child_id);
    }

    #[test]
    fn transform_scale_command_rejects_zero_factor() {
        let mut ids = IdAllocator::default();
        let entity = SceneEntity::new(ids.next(), "Box").with_component(
            ComponentData::new("Transform").with_field("scale", PrimitiveValue::F64(1.0)),
        );
        let entity_id = entity.id;
        let mut scene = SceneDocument::new("Test").with_root(entity);
        let mut stack = CommandStack::default();

        // Zero scale would be non-invertible; the command must reject
        // it up-front instead of silently destroying undo state.
        let err = stack.execute(
            &mut scene,
            Box::new(ScaleTransformCommand::new(entity_id, 0.0)),
        );
        assert!(err.is_err(), "zero scale factor must not execute");
        assert_eq!(stack.undo_len(), 0);
    }
}
