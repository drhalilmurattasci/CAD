# Changelog

All notable changes to `rustcad` land here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this
project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- `cad` module — full CAD operations stack, broken into independently
  usable submodules: `core` (entity DAG), `math` (planes / lines on
  top of `rustcad::math`), `constraint` (Gauss-Newton nonlinear
  solver + built-in constraints), `sketch` (2D primitives + closed-
  profile extraction), `kernel` (B-Rep topology + curves / surfaces),
  `parametric` (feature tree over Extrude / Cut / Revolve / Sweep /
  Loft / Fillet / Chamfer), `boolean` (pluggable kernel trait +
  three shipping engines), `mesh` (triangle mesh + subdivide /
  bounds / normals / merge), `modifier` (Blender-style stack:
  Translate / Scale / Mirror / Array / Subdivide), `tessellation`
  (B-Rep → Mesh via ear-clip triangulation), `render` (renderer-
  agnostic `DrawData` + Möller-Trumbore picking), and `command`
  (CAD-shaped undo/redo layered on `rustcad::undo`).
- `cad::kernel` — Cox-de Boor NURBS curve + tensor-product surface
  evaluators. `Curve::Ellipse` + `Curve::Nurbs` + `Surface::Nurbs`
  all evaluate; no more `unimplemented!` panics.
- `cad::boolean::CsgEngine` — polygon-BSP mesh boolean engine,
  shaped after csg.js. Handles Union / Difference / Intersection on
  arbitrary closed triangle meshes.
- `cad::tessellation::tessellate_cylindrical_face` + curved-edge
  discretization in `walk_wire` (Circle / Ellipse / NURBS edges are
  now sampled per `TessellationOptions`).
- `cad::command::SnapshotCommand` now performs a full `Brep` clone
  on capture, so undo/redo round-trips any B-Rep mutation the
  operation made (not just the feature tree).
- `Brep` is now `Clone` + `Debug`.
- `thiserror` added as a required dep for CAD-module error types.

### Added (earlier)
- `math` module — `Ray`, `Aabb`, `ray_plane_hit`. Pure-`glam` primitives
  lifted out of the GameCAD editor's picker so downstream tools can
  reuse them without pulling the ECS along.
- `undo` module — generic `Command<T, E>` trait + `CommandStack<T, E>`.
  Both type parameters are load-bearing: consumers pick their own
  document type and error family rather than being stuck with a
  hardcoded `Box<dyn Error>`.
- `id` module — `IdAllocator<T>` issuing typed newtype ids via
  `T: From<u64>`. Never recycles; `new(start_at)` resumes after
  deserialization.
- Examples: `input_snapshot`, `event_bus`, `undo_stack`, `id_alloc`.
- MSRV pinned at Rust 1.85 (edition 2024). Crate-wide
  `#![forbid(unsafe_code)]` and `#![warn(missing_docs)]`.

### Changed
- `prelude` now re-exports `Aabb`, `Ray`, `ray_plane_hit`, `Command`,
  `CommandStack`, and `IdAllocator` alongside the input / events types.

## [0.1.0]

### Added
- Initial crate carving out `input` (typed keyboard snapshot) and
  `events` (publish/drain `EventBus` + canonical `EditorEvent`) from
  the sibling `lib/input` and `lib/events` crates. Single `rustcad`
  umbrella crate, egui-shaped — one install, curated modules,
  `rustcad::prelude` for quick imports.
- `serde` feature (default on) for persisting `EditorEvent` /
  `PlayModeState` into telemetry + IPC fixtures.
