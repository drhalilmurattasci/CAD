//! Minimal `Input` / `Key` roundtrip.
//!
//! Shows the shape a gameplay tick sees: press some keys, query the
//! snapshot (via `axis` for WASD-style movement), and clear on
//! mode-change. No host/windowing layer — the editor would normally
//! fill `Input` from whatever its UI toolkit surfaces.
//!
//! Run with:
//!
//! ```bash
//! cargo run --example input_snapshot
//! ```

use rustcad::prelude::*;

fn main() {
    let mut input = Input::new();

    // Pretend the host forwarded "W + D held" this frame.
    input.press(Key::W);
    input.press(Key::D);

    // Gameplay code reads through the typed snapshot.
    let forward = input.axis(Key::S, Key::W);
    let right = input.axis(Key::A, Key::D);
    println!("forward axis: {forward:+.0}");
    println!("right axis:   {right:+.0}");

    // On mode-change (e.g. leaving Play mode) drop every held key so
    // the next session starts clean.
    input.clear();
    assert!(!input.pressed(Key::W));
    println!("cleared, W still held? {}", input.pressed(Key::W));
}
