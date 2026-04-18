//! Re-export of [`rustcad::events`] — the standalone umbrella library
//! under `lib/rustcad`.
//!
//! The bus + event-enum originally lived here. Moved into `rustcad`
//! during the lib/ rebrand (egui-shaped: one crate, many modules);
//! this file is the compatibility shim so `engine::events::*`
//! imports keep resolving for the editor and downstream test
//! fixtures. Downstream code that only wants the bus can depend on
//! `rustcad` directly and import from `rustcad::events` or
//! `rustcad::prelude`.

pub use rustcad::events::{EditorEvent, EventBus, PlayModeState};
