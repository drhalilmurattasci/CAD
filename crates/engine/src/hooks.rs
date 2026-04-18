use crate::events::{EditorEvent, EventBus};
use crate::scene::{SceneDocument, SceneEntity, SceneId};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderRequest {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderOutput {
    pub color_attachment_id: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PickRequest {
    pub x: f32,
    pub y: f32,
}

pub trait EngineHooks {
    fn tick_headless(&mut self, delta_seconds: f32);
    fn render_to_texture(&mut self, request: RenderRequest) -> RenderOutput;
    fn pick_entity(&self, request: PickRequest) -> Option<SceneId>;
    fn events(&mut self) -> &mut EventBus<EditorEvent>;
}

#[derive(Default)]
pub struct MockEngine {
    last_delta_seconds: f32,
    next_attachment_id: u64,
    picked_entity: Option<SceneId>,
    pick_slots: Vec<SceneId>,
    last_render_request: Option<RenderRequest>,
    events: EventBus<EditorEvent>,
}

impl MockEngine {
    pub fn set_picked_entity(&mut self, entity: Option<SceneId>) {
        self.picked_entity = entity;
    }

    pub fn sync_scene(&mut self, scene: &SceneDocument) {
        self.pick_slots.clear();
        for entity in &scene.root_entities {
            collect_ids(entity, &mut self.pick_slots);
        }
    }

    pub fn set_pick_slots(&mut self, slots: Vec<SceneId>) {
        self.pick_slots = slots;
    }

    pub fn last_delta_seconds(&self) -> f32 {
        self.last_delta_seconds
    }
}

impl EngineHooks for MockEngine {
    fn tick_headless(&mut self, delta_seconds: f32) {
        self.last_delta_seconds = delta_seconds;
        self.events
            .publish(EditorEvent::HeadlessTicked { delta_seconds });
    }

    fn render_to_texture(&mut self, _request: RenderRequest) -> RenderOutput {
        self.last_render_request = Some(_request);
        self.next_attachment_id += 1;
        let output = RenderOutput {
            color_attachment_id: self.next_attachment_id,
        };
        self.events
            .publish(EditorEvent::ViewportRendered(output.color_attachment_id));
        output
    }

    fn pick_entity(&self, request: PickRequest) -> Option<SceneId> {
        if let Some(render_request) = self.last_render_request
            && !self.pick_slots.is_empty()
            && render_request.width > 0
        {
            let normalized_x = (request.x / render_request.width as f32).clamp(0.0, 0.999_999);
            let index = (normalized_x * self.pick_slots.len() as f32) as usize;
            return self.pick_slots.get(index).copied();
        }

        self.picked_entity
    }

    fn events(&mut self) -> &mut EventBus<EditorEvent> {
        &mut self.events
    }
}

fn collect_ids(entity: &SceneEntity, slots: &mut Vec<SceneId>) {
    slots.push(entity.id);
    for child in &entity.children {
        collect_ids(child, slots);
    }
}

#[cfg(test)]
mod tests {
    use super::{EngineHooks, MockEngine, PickRequest, RenderRequest};
    use crate::events::EditorEvent;
    use crate::scene::{ComponentData, PrimitiveValue, SceneDocument, SceneEntity, SceneId};

    #[test]
    fn mock_engine_exposes_headless_render_and_pick_hooks() {
        let mut engine = MockEngine::default();
        engine.set_picked_entity(Some(SceneId::new(7)));

        engine.tick_headless(1.0 / 60.0);
        let output = engine.render_to_texture(RenderRequest {
            width: 1280,
            height: 720,
        });
        let picked = engine.pick_entity(PickRequest { x: 10.0, y: 20.0 });

        let events = engine.events().drain();
        assert_eq!(engine.last_delta_seconds(), 1.0 / 60.0);
        assert_eq!(output.color_attachment_id, 1);
        assert_eq!(picked, Some(SceneId::new(7)));
        assert_eq!(
            events,
            vec![
                EditorEvent::HeadlessTicked {
                    delta_seconds: 1.0 / 60.0
                },
                EditorEvent::ViewportRendered(1),
            ]
        );
    }

    #[test]
    fn mock_engine_can_pick_entities_from_synced_scene_slots() {
        let mut engine = MockEngine::default();
        let scene = SceneDocument::new("Sandbox")
            .with_root(
                SceneEntity::new(SceneId::new(1), "Camera").with_component(
                    ComponentData::new("Transform")
                        .with_field("x", PrimitiveValue::F64(0.0)),
                ),
            )
            .with_root(SceneEntity::new(SceneId::new(2), "Player"));
        engine.sync_scene(&scene);
        engine.render_to_texture(RenderRequest {
            width: 1000,
            height: 600,
        });

        assert_eq!(engine.pick_entity(PickRequest { x: 100.0, y: 5.0 }), Some(SceneId::new(1)));
        assert_eq!(engine.pick_entity(PickRequest { x: 750.0, y: 5.0 }), Some(SceneId::new(2)));
    }
}
