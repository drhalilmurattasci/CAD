# RustForge Master Implementation Plan

## Intent

This document turns the phase series into an executable program of work and records the first bootstrap implementation delivered in the repository.

## Program Waves

### Wave 1: Editor 1.0 foundation

- Split the workspace into `rustforge-core`, `rustforge-editor`, and `rustforge-editor-ui`.
- Build the core editor contracts first: reflection registry, scene IDs and text serialization, command-routed mutations, engine hooks, event bus, and capability manifests.
- Keep reusable editor surface areas in componentized files so panels and tools can grow without collapsing into one giant module.

### Wave 2: Runtime breadth

- Add authorable runtime systems for networking, UI, input, audio, VFX, sequencer, materials, rendering, and platform expansion.
- Extend existing primitives instead of creating parallel systems.

### Wave 3: Depth and team infrastructure

- Add animation graph runtime, advanced physics authoring, AI/navigation, weather, XR, and shared build/DDC infrastructure.
- Review the architecture before starting the parity wave.

### Wave 4: AAA-parity program

- Add world partition, advanced terrain/foliage, replay, multiplayer services, advanced animation, ray tracing/path tracing, multi-user live editing, and modding/live ops.

### Wave 5: Differentiation and interop

- Add ML-assisted tooling, procedural systems, advanced simulation suites, deep profiling, procedural characters, narrative/quest tooling, and migration pipelines.

## Bootstrap Delivered

The current codebase implements the first reusable foundation slice:

- A Cargo workspace rooted at `A:\GAMECAD`.
- `rustforge-core` with reusable modules for reflection, scenes, commands, engine hooks, events, assets, and capability manifests.
- `rustforge-editor-ui` with a reusable windowed component library for menus, toolbars, status bars, dock regions, and Wave 1 panels.
- A JSON-driven editor chrome definition at `config/editor-chrome.json` for labels, icons, tabs, and regions.
- A file-backed sample project at `projects/sandbox` with a real startup scene and scanned assets for the content browser.
- `rustforge-editor` as a windowed `eframe` launcher with a smoke-test startup path.
- Unit tests for scene serialization, command undo/redo, mock engine hooks, event bus ordering, manifest validation, and editor shell defaults.

## Immediate Next Steps

1. Expand the command stack so all authoring operations are routed through reusable command types.
2. Introduce preferences, keybindings, and accessibility settings as serializable editor resources.
3. Replace the placeholder viewport with a real render surface and picking bridge from `rustforge-core`.
4. Add a true project picker and asset import pipeline on top of the sample-project filesystem layer.
