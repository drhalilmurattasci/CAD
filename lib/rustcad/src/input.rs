//! Typed keyboard snapshot used to bridge editor input into game
//! systems. Originally a standalone `lib/input` crate; folded into
//! [`rustcad`](crate) during the rename so the umbrella library ships
//! input + events + friends as one install.
//!
//! The editor samples the host UI's keyboard state each frame, fills
//! an [`Input`], and hands it to gameplay systems. The ECS side reads
//! through [`Input::pressed`] — no direct dependency on egui or any
//! other windowing layer, so headless tests can exercise the same
//! gameplay systems by constructing [`Input`] by hand.
//!
//! The key set is intentionally small. Gameplay code that needs richer
//! input (mouse deltas, analog sticks, axes) can layer on top; for the
//! minimal keyboard loop this is enough to prove end-to-end plumbing:
//!
//! ```text
//! keyboard → Input → Mover / Script systems → Transform → render
//! ```

use std::collections::HashSet;

/// Typed keyboard symbol. Stringly-typed lookups are tempting but
/// fragile — a typo in a gameplay script silently stops working
/// without a compile error. An enum forces every consumer to
/// acknowledge the closed set.
///
/// Variants map to the host framework's equivalent keys where the
/// names match; additions arrive as systems need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Key {
    W,
    A,
    S,
    D,
    Q,
    E,
    Space,
    LeftShift,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
}

/// Snapshot of keyboard state for one gameplay tick.
///
/// Edge events (pressed/released this frame) intentionally aren't
/// tracked here — gameplay code that needs them can diff two
/// consecutive `Input` snapshots. Continuous polling (`is W held
/// now?`) is the 95% case and that's what the API optimizes for.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Input {
    pressed: HashSet<Key>,
}

impl Input {
    /// Empty input — no keys held. Useful for tests.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mark `key` as held. Idempotent.
    pub fn press(&mut self, key: Key) {
        self.pressed.insert(key);
    }

    /// Mark `key` as not held. Idempotent.
    pub fn release(&mut self, key: Key) {
        self.pressed.remove(&key);
    }

    /// True if `key` is currently held.
    pub fn pressed(&self, key: Key) -> bool {
        self.pressed.contains(&key)
    }

    /// Convenience: signed axis from a pair of opposing keys.
    /// Returns `+1.0` if `positive` is held, `-1.0` if `negative` is
    /// held, `0.0` if neither or both.
    pub fn axis(&self, negative: Key, positive: Key) -> f32 {
        let pos = self.pressed(positive);
        let neg = self.pressed(negative);
        match (pos, neg) {
            (true, false) => 1.0,
            (false, true) => -1.0,
            _ => 0.0,
        }
    }

    /// Drop every held key. The editor calls this when exiting Play
    /// mode so the next play session starts with a clean slate.
    pub fn clear(&mut self) {
        self.pressed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::{Input, Key};

    #[test]
    fn press_and_release_roundtrip() {
        let mut input = Input::new();
        assert!(!input.pressed(Key::W));
        input.press(Key::W);
        assert!(input.pressed(Key::W));
        input.release(Key::W);
        assert!(!input.pressed(Key::W));
    }

    #[test]
    fn axis_resolves_directional_pair() {
        let mut input = Input::new();
        assert_eq!(input.axis(Key::A, Key::D), 0.0);

        input.press(Key::D);
        assert_eq!(input.axis(Key::A, Key::D), 1.0);

        input.press(Key::A);
        // Both held → canceled.
        assert_eq!(input.axis(Key::A, Key::D), 0.0);

        input.release(Key::D);
        assert_eq!(input.axis(Key::A, Key::D), -1.0);
    }

    #[test]
    fn clear_drops_all_keys() {
        let mut input = Input::new();
        input.press(Key::W);
        input.press(Key::Space);
        input.clear();
        assert!(!input.pressed(Key::W));
        assert!(!input.pressed(Key::Space));
    }
}
