//! Tiny publish/drain event bus used to tunnel editor / gameplay
//! notifications out to UI panels, telemetry sinks, and scripting
//! hooks without binding publisher and subscriber at compile time.
//!
//! Originally a standalone `lib/events` crate; folded into
//! [`rustcad`](crate) as a module during the rename so the umbrella
//! library stays egui-shaped (one crate, curated modules).
//!
//! The design goal here is *frugality*: there is exactly one API
//! surface ([`EventBus::publish`] + [`EventBus::drain`]) and zero
//! runtime dispatch. Consumers that need multiple independent streams
//! instantiate multiple buses — one per event family — rather than
//! sharing a single heterogeneous channel. Type-erasing
//! [`EditorEvent`] variants into a single bus is fine today; if
//! cross-cutting plumbing ever needs dynamic subscribe / unsubscribe,
//! a thin `Callbacks<E>` type can be layered on top.
//!
//! `serde` derives are gated behind the crate-level `serde` feature
//! (default on). Turn it off to shed `serde` entirely when only the
//! in-memory bus is needed.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Tri-state Play mode machine shared between the editor shell and
/// anything listening to [`EditorEvent::PlayModeChanged`].
///
/// Kept serializable (under the `serde` feature) so telemetry / crash
/// logs can embed the last observed state without a conversion layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum PlayModeState {
    /// The editor is authoring the scene; simulation is off.
    Editing,
    /// The scene is running — physics / scripting / play-mode systems
    /// tick every frame.
    Playing,
    /// The scene was entered into Play mode and then paused. Systems
    /// don't tick, but the Play-mode scene snapshot is preserved so
    /// resuming doesn't reset gameplay state.
    Paused,
}

/// Canonical editor-wide event stream. Publishers live wherever
/// interesting things happen — scene save/load, asset import, play
/// toggles — and the editor's Console/Telemetry panels drain from one
/// `EventBus<EditorEvent>`. New variants are cheap because serde
/// handles the forward-compat story.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum EditorEvent {
    /// A scene file was opened; the string is the path (relative to
    /// the project root, forward-slash separators).
    SceneOpened(String),
    /// A scene file was written to disk.
    SceneSaved(String),
    /// An asset import completed successfully.
    AssetImported(String),
    /// Play mode transitioned; payload is the new state.
    PlayModeChanged(PlayModeState),
    /// A headless tick advanced the simulation by this many seconds.
    /// Emitted from CLI / batch runs that don't draw a viewport.
    HeadlessTicked {
        /// Simulation time advanced by this tick, in seconds.
        delta_seconds: f32,
    },
    /// A viewport frame was rendered; the `u64` is a monotonically
    /// increasing frame id.
    ViewportRendered(u64),
}

/// In-memory pending-queue with `publish` / `drain` semantics. Generic
/// over `E` so one bus type can carry any event family — editor
/// notifications, gameplay effects, script-driven signals — without
/// forking the plumbing.
#[derive(Debug, Clone)]
pub struct EventBus<E> {
    pending: Vec<E>,
}

impl<E> Default for EventBus<E> {
    fn default() -> Self {
        Self { pending: Vec::new() }
    }
}

impl<E> EventBus<E> {
    /// Push an event onto the pending queue. O(1) amortized.
    pub fn publish(&mut self, event: E) {
        self.pending.push(event);
    }

    /// Take all pending events, leaving the bus empty. Insertion order
    /// is preserved so consumers can rely on the drained `Vec` being a
    /// faithful replay of `publish` calls.
    pub fn drain(&mut self) -> Vec<E> {
        std::mem::take(&mut self.pending)
    }

    /// Number of events currently queued and not yet drained.
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// `true` when the bus has no pending events.
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::{EditorEvent, EventBus};

    #[test]
    fn event_bus_drains_events_in_order() {
        let mut bus = EventBus::default();
        bus.publish(EditorEvent::SceneOpened("sandbox.scene.ron".into()));
        bus.publish(EditorEvent::SceneSaved("sandbox.scene.ron".into()));

        assert_eq!(bus.len(), 2);
        assert_eq!(
            bus.drain(),
            vec![
                EditorEvent::SceneOpened("sandbox.scene.ron".into()),
                EditorEvent::SceneSaved("sandbox.scene.ron".into()),
            ]
        );
        assert!(bus.is_empty());
    }
}
