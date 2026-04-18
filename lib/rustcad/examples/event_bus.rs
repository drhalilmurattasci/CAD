//! End-to-end shape of the publish / drain bus.
//!
//! Publishers fire typed [`EditorEvent`]s and a consumer drains them
//! in insertion order. The bus is generic — swap `EditorEvent` for
//! any type to carry gameplay effects, script signals, etc.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example event_bus
//! ```

use rustcad::prelude::*;

fn main() {
    let mut bus: EventBus<EditorEvent> = EventBus::default();

    bus.publish(EditorEvent::SceneOpened("sandbox.scene.ron".into()));
    bus.publish(EditorEvent::PlayModeChanged(PlayModeState::Playing));
    bus.publish(EditorEvent::ViewportRendered(1));
    bus.publish(EditorEvent::ViewportRendered(2));

    println!("pending: {}", bus.len());

    for event in bus.drain() {
        println!("  drained: {event:?}");
    }

    assert!(bus.is_empty());
}
