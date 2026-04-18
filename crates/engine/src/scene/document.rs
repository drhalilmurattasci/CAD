use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::id::SceneId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PrimitiveValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ComponentData {
    pub type_name: String,
    pub fields: BTreeMap<String, PrimitiveValue>,
}

impl ComponentData {
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            fields: BTreeMap::new(),
        }
    }

    pub fn with_field(mut self, name: impl Into<String>, value: PrimitiveValue) -> Self {
        self.fields.insert(name.into(), value);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneEntity {
    pub id: SceneId,
    pub name: String,
    pub components: Vec<ComponentData>,
    pub children: Vec<SceneEntity>,
}

impl SceneEntity {
    pub fn new(id: SceneId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            components: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_component(mut self, component: ComponentData) -> Self {
        self.components.push(component);
        self
    }

    pub fn with_child(mut self, child: SceneEntity) -> Self {
        self.children.push(child);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SceneDocument {
    pub name: String,
    pub root_entities: Vec<SceneEntity>,
}

impl SceneDocument {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            root_entities: Vec::new(),
        }
    }

    pub fn with_root(mut self, root: SceneEntity) -> Self {
        self.root_entities.push(root);
        self
    }

    pub fn to_ron_string(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    pub fn from_ron_string(input: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(input)
    }

    pub fn find_entity(&self, id: SceneId) -> Option<&SceneEntity> {
        find_entity(&self.root_entities, id)
    }

    pub fn find_entity_mut(&mut self, id: SceneId) -> Option<&mut SceneEntity> {
        find_entity_mut(&mut self.root_entities, id)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PrefabDocument {
    pub root: SceneEntity,
}

impl PrefabDocument {
    pub fn new(root: SceneEntity) -> Self {
        Self { root }
    }

    pub fn to_ron_string(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    pub fn from_ron_string(input: &str) -> Result<Self, ron::error::SpannedError> {
        ron::from_str(input)
    }

    /// I-19: materialise the prefab into a fresh `SceneEntity` tree.
    ///
    /// Every entity (root and descendants) gets a brand-new `SceneId`
    /// pulled from `ids`. The prefab's own IDs are template-only —
    /// treating them as scene IDs would make a second instantiation of
    /// the same prefab collide with the first.
    ///
    /// The optional `name_suffix` is appended to the root name so the
    /// hierarchy panel can disambiguate multiple instances (e.g.
    /// `"Torch (2)"`). Children keep their authored names — those are
    /// part of the prefab's shape.
    pub fn instantiate(
        &self,
        ids: &mut super::id::IdAllocator,
        name_suffix: Option<&str>,
    ) -> SceneEntity {
        let mut instance = clone_with_fresh_ids(&self.root, ids);
        if let Some(suffix) = name_suffix {
            instance.name = format!("{}{}", instance.name, suffix);
        }
        instance
    }
}

/// Deep-clone a `SceneEntity`, swapping every `SceneId` for a new one
/// from `ids`. Components and their field maps are cloned as-is — the
/// id space we're remapping is the entity identity, not component
/// payload values (which may themselves be numeric but aren't scene
/// ids).
fn clone_with_fresh_ids(
    template: &SceneEntity,
    ids: &mut super::id::IdAllocator,
) -> SceneEntity {
    let mut cloned = SceneEntity::new(ids.next(), template.name.clone());
    cloned.components = template.components.clone();
    cloned.children = template
        .children
        .iter()
        .map(|child| clone_with_fresh_ids(child, ids))
        .collect();
    cloned
}

fn find_entity(entities: &[SceneEntity], id: SceneId) -> Option<&SceneEntity> {
    for entity in entities {
        if entity.id == id {
            return Some(entity);
        }

        if let Some(child) = find_entity(&entity.children, id) {
            return Some(child);
        }
    }

    None
}

fn find_entity_mut(entities: &mut [SceneEntity], id: SceneId) -> Option<&mut SceneEntity> {
    for entity in entities {
        if entity.id == id {
            return Some(entity);
        }

        if let Some(child) = find_entity_mut(&mut entity.children, id) {
            return Some(child);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{ComponentData, PrefabDocument, PrimitiveValue, SceneDocument, SceneEntity};
    use crate::scene::IdAllocator;

    #[test]
    fn scene_roundtrips_through_ron() {
        let mut ids = IdAllocator::default();
        let child = SceneEntity::new(ids.next(), "Camera");
        let scene = SceneDocument::new("Sandbox").with_root(
            SceneEntity::new(ids.next(), "Player")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(1.0))
                        .with_field("y", PrimitiveValue::F64(2.0)),
                )
                .with_child(child),
        );

        let serialized = scene.to_ron_string().unwrap();
        let deserialized = SceneDocument::from_ron_string(&serialized).unwrap();

        assert_eq!(scene, deserialized);
    }

    #[test]
    fn prefab_wraps_a_scene_entity_tree() {
        let mut ids = IdAllocator::new(100);
        let prefab = PrefabDocument::new(
            SceneEntity::new(ids.next(), "Torch").with_component(ComponentData::new("Light")),
        );

        assert_eq!(prefab.root.name, "Torch");
        assert_eq!(prefab.root.id.0, 101);
    }

    #[test]
    fn prefab_instantiate_remaps_every_id() {
        // Prefab template uses ids in the 1..=3 range; the live scene
        // has its own allocator at 100+. After instantiating, every
        // node in the instance must carry a fresh id from the scene's
        // allocator — zero overlap with the template's 1..=3.
        let mut template_ids = IdAllocator::default();
        let prefab = PrefabDocument::new(
            SceneEntity::new(template_ids.next(), "Torch")
                .with_component(ComponentData::new("Light"))
                .with_child(SceneEntity::new(template_ids.next(), "Flame"))
                .with_child(SceneEntity::new(template_ids.next(), "Smoke")),
        );

        let mut scene_ids = IdAllocator::new(100);
        let instance_a = prefab.instantiate(&mut scene_ids, None);
        let instance_b = prefab.instantiate(&mut scene_ids, Some(" (2)"));

        // All six ids are distinct and none collide with the template.
        let mut seen = std::collections::HashSet::new();
        for id in collect_ids(&instance_a) {
            assert!(id.0 > 100, "expected scene-space id, got {}", id.0);
            assert!(seen.insert(id), "duplicate id {}", id.0);
        }
        for id in collect_ids(&instance_b) {
            assert!(seen.insert(id), "duplicate id across instances {}", id.0);
        }

        // Component payload and child names survive intact.
        assert_eq!(instance_a.name, "Torch");
        assert_eq!(instance_b.name, "Torch (2)");
        assert_eq!(instance_a.components.len(), 1);
        assert_eq!(instance_a.components[0].type_name, "Light");
        assert_eq!(instance_a.children.len(), 2);
        assert_eq!(instance_a.children[0].name, "Flame");
    }

    #[test]
    fn prefab_roundtrips_through_ron() {
        let mut ids = IdAllocator::default();
        let prefab = PrefabDocument::new(
            SceneEntity::new(ids.next(), "Enemy")
                .with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(0.0)),
                )
                .with_child(SceneEntity::new(ids.next(), "Weapon")),
        );

        let ron = prefab.to_ron_string().unwrap();
        let parsed = PrefabDocument::from_ron_string(&ron).unwrap();
        assert_eq!(prefab, parsed);
    }

    fn collect_ids(entity: &SceneEntity) -> Vec<super::super::id::SceneId> {
        let mut out = vec![entity.id];
        for child in &entity.children {
            out.extend(collect_ids(child));
        }
        out
    }
}
