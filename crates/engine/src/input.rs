//! Re-export of [`rustcad::input`] — the standalone umbrella library
//! under `lib/rustcad`.
//!
//! Before the lib/ refactor this module owned the full `Key` + `Input`
//! implementation directly. The types were moved out to the
//! framework-agnostic `rustcad` crate (egui-shaped: one crate, many
//! modules) so downstream projects can reuse them without pulling in
//! the rest of the engine. This shim preserves the legacy
//! `engine::input::*` import path so existing call sites keep
//! compiling without per-file migrations. Downstream crates that only
//! want [`Input`] (without the rest of `engine`) can depend directly
//! on `rustcad` and import from `rustcad::input` or `rustcad::prelude`
//! instead.

pub use rustcad::input::{Input, Key};
