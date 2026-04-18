use std::any::TypeId;
use std::collections::{BTreeMap, HashMap};

/// Re-export of the `#[derive(Reflect)]` proc-macro so downstream
/// crates can `use engine::reflection::Reflect;` and get both
/// the trait and the derive from one path.
pub use reflect_derive::Reflect as ReflectDerive;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    I64,
    F64,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDescriptor {
    pub name: &'static str,
    pub kind: ValueKind,
}

pub trait Reflect: Send + Sync + 'static {
    fn type_name() -> &'static str
    where
        Self: Sized;

    fn fields() -> &'static [FieldDescriptor]
    where
        Self: Sized;
}

#[derive(Debug, Clone)]
pub struct RegisteredComponent {
    pub type_id: TypeId,
    pub type_name: &'static str,
    pub fields: &'static [FieldDescriptor],
}

#[derive(Debug, Default)]
pub struct ComponentRegistry {
    by_type: HashMap<TypeId, RegisteredComponent>,
    by_name: BTreeMap<&'static str, TypeId>,
}

impl ComponentRegistry {
    pub fn register<T: Reflect>(&mut self) -> &RegisteredComponent {
        let type_id = TypeId::of::<T>();
        let entry = self.by_type.entry(type_id).or_insert_with(|| RegisteredComponent {
            type_id,
            type_name: T::type_name(),
            fields: T::fields(),
        });
        self.by_name.insert(entry.type_name, type_id);
        entry
    }

    pub fn get<T: Reflect>(&self) -> Option<&RegisteredComponent> {
        self.by_type.get(&TypeId::of::<T>())
    }

    pub fn get_by_name(&self, type_name: &str) -> Option<&RegisteredComponent> {
        let type_id = self.by_name.get(type_name)?;
        self.by_type.get(type_id)
    }

    pub fn list(&self) -> Vec<&RegisteredComponent> {
        let mut components = self.by_type.values().collect::<Vec<_>>();
        components.sort_by_key(|component| component.type_name);
        components
    }
}

#[cfg(test)]
mod tests {
    use super::{ComponentRegistry, FieldDescriptor, Reflect, ValueKind};

    struct Transform;

    impl Reflect for Transform {
        fn type_name() -> &'static str {
            "Transform"
        }

        fn fields() -> &'static [FieldDescriptor] {
            static FIELDS: [FieldDescriptor; 3] = [
                FieldDescriptor {
                    name: "translation_x",
                    kind: ValueKind::F64,
                },
                FieldDescriptor {
                    name: "translation_y",
                    kind: ValueKind::F64,
                },
                FieldDescriptor {
                    name: "translation_z",
                    kind: ValueKind::F64,
                },
            ];
            &FIELDS
        }
    }

    #[test]
    fn registry_tracks_components_by_type_and_name() {
        let mut registry = ComponentRegistry::default();
        registry.register::<Transform>();

        let registered = registry.get::<Transform>().unwrap();
        assert_eq!(registered.type_name, "Transform");
        assert_eq!(registered.fields.len(), 3);
        assert_eq!(registry.get_by_name("Transform").unwrap().type_name, "Transform");
    }

    // ---- I-6: #[derive(Reflect)] coverage -------------------------------
    //
    // The derive crate is bound to this trait; keep the tests here so a
    // broken derive fails the same `cargo test -p engine` run
    // that catches manual impl regressions.

    use crate::reflection::ReflectDerive;

    #[derive(ReflectDerive)]
    #[allow(dead_code)]
    struct DerivedTag {
        label:      String,
        priority:   i32,
        visible:    bool,
        pan_speed:  f32,
        zoom_speed: f64,
        #[reflect(skip)]
        _internal:  std::marker::PhantomData<()>,
        #[reflect(rename = "display_name")]
        raw_name:   String,
    }

    #[test]
    fn derive_reflect_emits_expected_descriptors() {
        assert_eq!(DerivedTag::type_name(), "DerivedTag");
        let fields = DerivedTag::fields();
        // 5 primitives + renamed raw_name, _internal is skipped.
        assert_eq!(fields.len(), 6);

        let by_name: std::collections::HashMap<_, _> = fields
            .iter()
            .map(|f| (f.name, f.kind))
            .collect();
        assert_eq!(by_name.get("label"), Some(&ValueKind::String));
        assert_eq!(by_name.get("priority"), Some(&ValueKind::I64));
        assert_eq!(by_name.get("visible"), Some(&ValueKind::Bool));
        assert_eq!(by_name.get("pan_speed"), Some(&ValueKind::F64));
        assert_eq!(by_name.get("zoom_speed"), Some(&ValueKind::F64));
        assert_eq!(by_name.get("display_name"), Some(&ValueKind::String));
        assert!(by_name.get("raw_name").is_none());
        assert!(by_name.get("_internal").is_none());
    }

    #[test]
    fn derive_reflect_registers_cleanly() {
        let mut registry = ComponentRegistry::default();
        registry.register::<DerivedTag>();
        let entry = registry.get_by_name("DerivedTag").unwrap();
        assert_eq!(entry.fields.len(), 6);
    }
}
