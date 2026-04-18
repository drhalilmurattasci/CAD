# RustCAD

*A framework-agnostic toolkit for game tooling.*

Curated primitives for game editors, content tools, and gameplay
plumbing. No windowing, no renderer, no ECS — just small, independent
data types and utilities that drop into any Rust project without
dragging a framework along.

Shaped after [`egui`](https://github.com/emilk/egui): one crate,
curated modules, a single `prelude` that pulls the common types into
scope with one `use`.

## Install

```toml
[dependencies]
rustcad = "0.1"
```

Skip `serde` if all you want is the in-memory types:

```toml
[dependencies]
rustcad = { version = "0.1", default-features = false }
```

MSRV: Rust 1.85 (edition 2024).

## Use

```rust
use rustcad::prelude::*;

// Typed keyboard snapshot — build up, query, hand to gameplay code.
let mut input = Input::new();
input.press(Key::W);
assert_eq!(input.axis(Key::S, Key::W), 1.0);

// Publish/drain event bus, generic over any event family.
let mut bus: EventBus<EditorEvent> = EventBus::default();
bus.publish(EditorEvent::SceneOpened("sandbox.scene.ron".into()));
for event in bus.drain() {
    // route to your UI, telemetry sink, script host, …
    println!("{event:?}");
}
```

More end-to-end examples live under [`examples/`](examples/):

```bash
cargo run --example input_snapshot
cargo run --example event_bus
cargo run --example undo_stack
cargo run --example id_alloc
```

## Modules

| Module                               | What                                                                                  |
| ------------------------------------ | ------------------------------------------------------------------------------------- |
| [`rustcad::input`](src/input.rs)     | Typed `Key` enum + `Input` snapshot. `std`-only.                                      |
| [`rustcad::events`](src/events.rs)   | `EventBus<E>` with `publish` / `drain`, plus a canonical `EditorEvent` stream.        |
| [`rustcad::math`](src/math.rs)       | Pure-`glam` `Ray`, `Aabb`, and `ray_plane_hit` — the picker-math starter kit.         |
| [`rustcad::undo`](src/undo.rs)       | Generic `Command<T, E>` trait + `CommandStack<T, E>` for linear undo/redo.            |
| [`rustcad::id`](src/id.rs)           | `IdAllocator<T>` — monotonic newtype ids backed by a shared `u64`.                    |
| [`rustcad::prelude`](src/lib.rs)     | Glob import of the common types across all modules above.                             |

### CAD layer — [`rustcad::cad`](src/cad/)

Modular CAD operations stack. Each submodule is independently usable —
drop the ones you don't need. Everything in the table below is real,
tested code: NURBS curves + surfaces evaluate via Cox-de Boor, the
`CsgEngine` performs mesh booleans on closed manifolds, curved wire
edges are tessellated per options, and `SnapshotCommand` does a deep
B-Rep round-trip.

| Module                                                        | Role                                                                             |
| ------------------------------------------------------------- | -------------------------------------------------------------------------------- |
| [`cad::core`](src/cad/core.rs)                                | `EntityId` + DAG for the parametric history.                                     |
| [`cad::math`](src/cad/math.rs)                                | `Plane`, `Line2`, `Line3` on top of `rustcad::math`.                             |
| [`cad::constraint`](src/cad/constraint.rs)                    | Gauss-Newton nonlinear constraint solver (SolveSpace-style).                     |
| [`cad::sketch`](src/cad/sketch.rs)                            | 2D sketch primitives + closed-profile extraction.                                |
| [`cad::kernel`](src/cad/kernel.rs)                            | B-Rep topology (Vertex/Edge/Wire/Face/Shell/Solid) + curves & surfaces.          |
| [`cad::parametric`](src/cad/parametric.rs)                    | Feature tree (Extrude / Cut / Revolve / Sweep / Loft / Fillet / Chamfer).        |
| [`cad::boolean`](src/cad/boolean.rs)                          | `BooleanOp` + pluggable `BooleanEngine` trait.                                   |
| [`cad::mesh`](src/cad/mesh.rs)                                | Triangle mesh + subdivide / bounds / normals / merge.                            |
| [`cad::modifier`](src/cad/modifier.rs)                        | Blender-style modifier stack (Translate, Scale, Mirror, Array, Subdivide).       |
| [`cad::tessellation`](src/cad/tessellation.rs)                | B-Rep → Mesh bridge with ear-clip polygon triangulation.                         |
| [`cad::render`](src/cad/render.rs)                            | Renderer-agnostic `DrawData` + Möller-Trumbore ray/mesh picking.                 |
| [`cad::command`](src/cad/command.rs)                          | CAD-shaped undo/redo layered on `rustcad::undo`.                                 |

New modules land as sibling files in this same crate, not as sibling
workspace members — keeping the "one `cargo add rustcad`" install
surface.

## Features

| Feature | Default | Effect                                                                                                                              |
| ------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `serde` | on      | `Serialize` / `Deserialize` derives for `EditorEvent` and `PlayModeState`, so consumers can persist events without a conversion layer. |

## Safety & lints

- `#![forbid(unsafe_code)]` — the entire crate is safe Rust.
- `#![warn(missing_docs)]` — every public item carries rustdoc.

## License

Licensed under either of

- [Apache License, Version 2.0](LICENSE-APACHE)
- [MIT license](LICENSE-MIT)

at your option.
