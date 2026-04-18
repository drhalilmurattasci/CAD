//! CAD-RS — a modular CAD operations layer for `rustcad`.
//!
//! Each submodule is a discrete, separately-consumable piece of the
//! stack. The taxonomy mirrors production CAD systems — SolveSpace
//! for constraints, FreeCAD for parametric + B-Rep, Blender for
//! mesh / modifier workflows, Unreal for the realtime-render bridge.
//!
//! ```text
//! [ UI / API ]
//!       ↓
//! [ command  ]  — undoable CAD operations, layered on rustcad::undo
//!       ↓
//! [ parametric ] — feature tree (extrude, revolve, sweep, loft, …)
//!       ↓
//! [ constraint ] — nonlinear constraint solver (SolveSpace-style)
//!       ↓
//! [ sketch   ]  — 2D parametric sketches → closed profiles
//!       ↓
//! [ kernel   ]  — B-Rep topology + NURBS-ready curves / surfaces
//!   ├─ [ boolean       ]  union / difference / intersection
//!   └─ [ tessellation  ]  B-Rep → Mesh
//!       ↓
//! [ mesh     ]  — triangle-soup container + basic ops
//!       ↓
//! [ modifier ]  — Blender-style stack (mirror, array, subdiv, …)
//!       ↓
//! [ render   ]  — renderer-agnostic draw buffers + ray picking
//! ```
//!
//! ## Reuse
//!
//! Every module is usable on its own. `cad::mesh` doesn't pull in
//! `cad::kernel`; `cad::constraint` doesn't know about sketches or
//! parametric features. Cross-module glue (e.g. "tessellate a B-Rep
//! face into a mesh") lives in one designated bridge module
//! ([`tessellation`]) so the other layers stay decoupled.
//!
//! ## Status
//!
//! Several layers here are scaffolding: the public API is stable
//! but the bodies of the hardest operations (full NURBS evaluation,
//! the boolean kernel, general triangulation) are stubs with clear
//! TODO docs. See each module's header for the implementation
//! status.

pub mod boolean;
pub mod command;
pub mod constraint;
pub mod core;
pub mod kernel;
pub mod math;
pub mod mesh;
pub mod modifier;
pub mod parametric;
pub mod render;
pub mod sketch;
pub mod tessellation;
