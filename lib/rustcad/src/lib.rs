//! # RustCAD
//!
//! *A framework-agnostic toolkit for game tooling — and the CAD
//! operations layer that sits on top of it.*
//!
//! Curated primitives for game editors, content tools, gameplay
//! plumbing, and parametric CAD. No windowing, no renderer, no ECS.
//! Shaped after [`egui`]: one crate, several curated modules, a
//! single [`prelude`] that pulls the common types into scope with
//! one `use`.
//!
//! Nothing in here assumes a particular engine, renderer, or windowing
//! stack — every module is pure Rust + (optionally) `serde`, so dropping
//! `rustcad` into an unrelated project compiles without dragging
//! surface-area in from the wider GameCAD workspace.
//!
//! ```no_run
//! use rustcad::prelude::*;
//!
//! let mut input = Input::new();
//! input.press(Key::W);
//! assert!(input.pressed(Key::W));
//!
//! let mut bus: EventBus<EditorEvent> = EventBus::default();
//! bus.publish(EditorEvent::SceneOpened("sandbox.scene.ron".into()));
//! let drained = bus.drain();
//! assert_eq!(drained.len(), 1);
//! ```
//!
//! ## Foundational modules
//!
//! | Module        | What it gives you                                                   |
//! | ------------- | ------------------------------------------------------------------- |
//! | [`input`]     | Typed keyboard snapshot ([`Key`] enum + [`Input`] state).           |
//! | [`events`]    | Tiny publish/drain [`events::EventBus`] + canonical [`EditorEvent`].|
//! | [`math`]      | Pure-[`glam`] ray, AABB, and ray-plane intersection helpers.        |
//! | [`undo`]      | Generic [`undo::Command`] trait + [`undo::CommandStack`].           |
//! | [`id`]        | Typed monotonic [`id::IdAllocator<T>`] (newtype ids over `u64`).    |
//!
//! ## CAD layer ([`cad`])
//!
//! A modular CAD operations stack layered on the foundations above.
//! Each submodule is separately usable:
//!
//! - [`cad::core`] — entity ids + dependency graph.
//! - [`cad::math`] — CAD-specific math (planes, lines) on top of [`math`].
//! - [`cad::constraint`] — nonlinear constraint solver (SolveSpace-style).
//! - [`cad::sketch`] — 2D sketch primitives + profile extraction.
//! - [`cad::kernel`] — B-Rep topology + curves/surfaces (FreeCAD/OCCT layer).
//! - [`cad::parametric`] — feature tree (extrude, revolve, sweep, …).
//! - [`cad::boolean`] — boolean-kernel trait + stub implementations.
//! - [`cad::mesh`] — triangle-mesh container + fundamental ops.
//! - [`cad::modifier`] — Blender-style modifier stack.
//! - [`cad::tessellation`] — B-Rep → mesh bridge.
//! - [`cad::render`] — renderer-agnostic draw data + ray/mesh picking.
//! - [`cad::command`] — CAD-shaped undo/redo over [`undo`].
//!
//! Each CAD module documents its implementation status: the
//! research-scope pieces (NURBS evaluators, full boolean kernel) are
//! scaffolded with stub returns and clear TODOs; everything else is
//! real, tested code.
//!
//! ## Features
//!
//! - `serde` (default on) — derives `Serialize`/`Deserialize` for the
//!   [`events`] types so consumers can persist them to telemetry /
//!   IPC without a conversion layer. Turn off to shrink the
//!   dependency tree if you only need the non-serde pieces.
//!
//! [`Input`]: crate::input::Input
//! [`Key`]: crate::input::Key
//! [`EditorEvent`]: crate::events::EditorEvent

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod cad;
pub mod events;
pub mod id;
pub mod input;
pub mod math;
pub mod undo;

/// One-stop import of the most commonly used types. Glob-importing
/// `rustcad::prelude::*` should cover ~80% of call sites in downstream
/// code; reach past the prelude for less common types like
/// [`events::PlayModeState`] or the CAD layer's specialized traits
/// on demand.
pub mod prelude {
    pub use crate::events::{EditorEvent, EventBus, PlayModeState};
    pub use crate::id::IdAllocator;
    pub use crate::input::{Input, Key};
    pub use crate::math::{Aabb, Ray, ray_plane_hit};
    pub use crate::undo::{Command, CommandStack};

    // CAD layer — the types downstream CAD code will reach for most
    // often. More specialized items (constraint variants, kernel
    // topology ids, modifier structs) stay opt-in to keep the
    // prelude lean.
    pub use crate::cad::core::{EntityId, EntityIdAllocator};
    pub use crate::cad::mesh::Mesh;
    pub use crate::cad::parametric::{Feature, FeatureTree};
    pub use crate::cad::sketch::{Profile, Sketch, SketchElement};
}
