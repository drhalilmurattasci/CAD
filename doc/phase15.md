# Phase 15 — Game UI Framework

Phases 1–13 built the editor, and the editor's UI is `egui`. That choice was right for the editor — immediate-mode, zero designer tooling, pure Rust, a menu bar and a property sheet land in a weekend — and completely wrong for games. Games need a retained-mode widget tree that artists can lay out without writing Rust, data-bound to reflected fields, animatable, localizable, gamepad-navigable, and shipped without the editor's dependencies. Phase 15 builds that framework. Call it the Unreal Motion Graphics (UMG) equivalent, minus the node-graph; a Rust-native, data-driven, designer-authored runtime UI layer distinct from the editor chrome.

The critical distinction that shapes every decision in this phase: **`egui` is editor UI only, and never ships in a game binary.** A title screen, a pause menu, an RPG inventory, a HUD — none of these belong in immediate-mode code written by a gameplay programmer. They belong in `.rui` assets authored by UI designers in a WYSIWYG panel, bound to game state through the Phase 2 reflection registry, and drawn by a dedicated retained-mode renderer. Phase 15 is the machinery that makes that possible.

## Goals

By end of Phase 15:

1. A **retained widget tree** with `taffy` flexbox layout, fully independent of `egui`, shipping in `rustforge-core` under a `ui` feature.
2. **`.rui` UI assets** (RON) describing widget hierarchies, styles, bindings, and state machines.
3. A **UI Designer panel** in the editor — WYSIWYG editing of `.rui` files following the Phase 8 `AssetEditor` pattern.
4. **Named styles** in `.rstyle` assets, hot-reloadable via Phase 5's file watcher.
5. **Data binding** to reflected fields and ECS queries via the Phase 2 registry; property-change notifications drive redraw.
6. **State-transition animation** (enter, hover, pressed, focus) with simple tween curves.
7. **Directional focus navigation** for gamepads and keyboards, plus pointer input routing that consumes events when the UI is focused.
8. **Localization** through Phase 13's `t!()` macro for every user-visible string.
9. **Accessibility** via `AccessKit` integration, mirroring the editor's Phase 13 accessibility baseline.
10. **Zero `egui` in shipped game binaries** — verified by a build check.

## 1. Why not ship `egui` for games

`egui` is excellent. It is also immediate-mode: every frame, game code says `if ui.button("Start").clicked()` and the widget tree is reconstructed from scratch. This is wonderful for a programmer authoring an editor panel, and wrong for a UI designer authoring a pause menu.

Concrete reasons `egui` does not belong in game UI:

- **No serialized layout.** A pause menu is code, not an asset. A non-programmer cannot edit it; `git diff` on a layout change is a wall of Rust rather than a structural diff.
- **No WYSIWYG tooling.** Immediate-mode UI cannot be designed visually — the layout only exists while the code runs. Any designer tool would have to code-generate, which nobody does well.
- **Bindings are ad-hoc.** Every data binding is hand-written. A complex HUD threads dozens of `&mut` references through the draw call. Reflection doesn't help because there's no declared structure to reflect against.
- **Animations are manual.** Every hover glow is a boolean plus a lerp the gameplay programmer wrote. No timeline, no state machine, no reuse.
- **Text and localization.** `egui` has text rendering; it does not have a localization pipeline, per-language font fallback, or BiDi awareness beyond basics. Games ship in many languages; the editor ships in English.
- **Gamepad focus.** `egui` added focus support, but directional nav — "pressing Down moves to the widget beneath this one" — requires a spatial graph the framework can't infer from an immediate-mode call tree.
- **Binary size and dependency.** Shipping `egui` + `egui-wgpu` + `egui_dock` in a game just to draw three menus pulls in the whole editor UI stack.

The decision is final and load-bearing for the rest of the phase: **game UI is its own framework.** Phase 15 builds it.

## 2. Widget tree and layout

```
crates/rustforge-core/src/ui/
├── mod.rs                   # Ui context: root widget, event dispatch, render pass
├── tree.rs                  # Widget trait, WidgetId, WidgetTree
├── widgets/
│   ├── mod.rs
│   ├── panel.rs             # container with children
│   ├── stack.rs             # vertical / horizontal linear layout
│   ├── grid.rs              # row/col grid
│   ├── text.rs              # label; localized string source
│   ├── image.rs             # textured rect
│   ├── button.rs
│   ├── toggle.rs
│   ├── slider.rs
│   ├── progress.rs
│   ├── list.rs              # virtualized item list bound to iterable
│   └── scroll.rs
├── layout.rs                # taffy integration
├── style.rs                 # Style struct, resolver, .rstyle loader
├── binding.rs               # Binding<T>: path into a reflected value
├── anim.rs                  # state transitions and tweens
├── focus.rs                 # directional focus graph
├── input.rs                 # pointer + gamepad input routing
├── render.rs                # draw list → wgpu command encoder
└── accessibility.rs         # AccessKit bridge
```

Widgets are a trait, not an enum, so user-authored widgets extend the set:

```rust
pub trait Widget: Reflect + Any + Send + Sync {
    fn layout(&self, ctx: &mut LayoutCtx) -> taffy::Style;
    fn draw(&self, ctx: &mut DrawCtx);
    fn children(&self) -> &[WidgetId] { &[] }
    fn on_event(&mut self, ev: &UiEvent, ctx: &mut EventCtx) -> EventResponse {
        EventResponse::Unhandled
    }
}
```

The tree itself lives in a `WidgetTree` resource, not in the ECS world. UI entities are a separate arena — they outlive scene reloads, they don't need the ECS's parallel iteration, and mixing them into the scene graph ties UI lifetimes to entities in ways that hurt more than they help.

### 2.1 Layout: `taffy`, not CSS

Layout is `taffy`'s flexbox and grid. Nothing else. **No CSS.** The reasoning:

- CSS is enormous. Implementing `margin: auto` correctly is a multi-month project; implementing selectors, specificity, cascade, and inheritance on top of it is a second multi-month project; then grid, float, and positioning each own their own weirdness. Nobody on this team is going to out-engineer Blink.
- CSS's cascade model doesn't match reflection. Reflected fields are named, typed, and bound. Styles in Phase 15 resolve by widget type and named style key — no descendant selectors, no specificity, no `!important`.
- Designers shipping game UI on Unity (uGUI, UI Toolkit) or Unreal (UMG) do not use CSS syntax. They use property panels. That's what Phase 15 gives them.

`taffy` handles the math. The style struct holds flex properties (`flex_direction`, `justify_content`, `align_items`, `gap`, `padding`, `margin`, etc.) plus visual properties (background, border, corner radius, text style). That's it.

## 3. `.rui` asset format

A `.rui` file is a RON document describing a widget subtree, styles it references, bindings it expects, and state transitions it plays:

```ron
// assets/ui/pause_menu.rui
(
    root: Panel(
        style: "pause.root",
        children: [
            Text(text: Loc("menu.pause.title"), style: "pause.title"),
            Stack(
                direction: Vertical,
                gap: 12.0,
                children: [
                    Button(
                        text: Loc("menu.pause.resume"),
                        on_click: Action("pause.resume"),
                        style: "pause.button",
                    ),
                    Button(
                        text: Loc("menu.pause.settings"),
                        on_click: Action("pause.open_settings"),
                        style: "pause.button",
                    ),
                    Button(
                        text: Loc("menu.pause.quit"),
                        on_click: Action("pause.quit"),
                        style: "pause.button.danger",
                    ),
                ],
            ),
        ],
    ),
    bindings: [
        (path: "game.player.hp", target: "hp_bar.value"),
    ],
    states: [
        (name: "enter", duration: 0.2, curve: EaseOut,
         target: "root", property: Opacity, from: 0.0, to: 1.0),
    ],
)
```

RON because scenes are RON (Phase 1) and the editor team already knows the format. The schema is a reflected struct tree — the same reflection registry that powers the editor inspector reads `.rui` files at load time, which means a new widget type is available to the designer the moment it's `#[derive(Reflect)]`-ed.

Loading a `.rui` asset yields a `UiPrefab`: an immutable description cloned into live widget instances on spawn. This mirrors scene prefabs in Phase 5 — edit the prefab, every instance updates.

## 4. UI Designer panel

A WYSIWYG `.rui` editor following the Phase 8 `AssetEditor` pattern. Double-click a `.rui` file in the Content Browser; the designer opens. This panel uses `egui` (it's editor-only); the widget tree it previews is the real runtime framework from §2.

```
crates/rustforge-editor/src/asset_editors/ui_designer/
├── mod.rs                   # UiDesigner : AssetEditor
├── canvas.rs                # WYSIWYG widget-tree preview
├── hierarchy.rs             # left pane: tree view + palette
├── inspector.rs             # right pane: reflected widget properties
├── bindings.rs              # bindings editor tab
├── states.rs                # state-transition editor tab
└── live_preview.rs          # play-mode preview with fake data
```

ASCII mockup:

```
┌─ UI Designer: pause_menu.rui ─────────────────────────────────────────────────┐
│ ┌─ Palette ─────┐  ┌─ Canvas (1920 x 1080) ────────────┐  ┌─ Inspector ─────┐ │
│ │ Panel          │  │                                   │  │ Button          │ │
│ │ Stack          │  │        ┌───────────────┐          │  │  id: resume_btn │ │
│ │ Grid           │  │        │  PAUSED       │          │  │  text: menu.p…  │ │
│ │ Text           │  │        │               │          │  │  style: pause…  │ │
│ │ Image          │  │        │  [ Resume  ]  │ ← sel    │  │  on_click:      │ │
│ │ Button         │  │        │  [ Settings]  │          │  │   Action pause  │ │
│ │ Toggle         │  │        │  [ Quit    ]  │          │  │  focus.up:      │ │
│ │ Slider         │  │        └───────────────┘          │  │   (auto)        │ │
│ │ Progress       │  │                                   │  │  focus.down:    │ │
│ │ List           │  │  Resolution: [1920x1080 ▼]        │  │   settings_btn  │ │
│ │ Scroll         │  │  Scale:      [1.0x       ▼]       │  │                 │ │
│ │ (custom…)      │  │  Locale:     [en-US      ▼]       │  │ Style override  │ │
│ ├───────────────┤  │                                   │  │  padding: 12    │ │
│ │ Hierarchy      │  │  [▶ Preview] [Stop] [Reset anim]  │  │  bg: accent     │ │
│ │  root (Panel)  │  └───────────────────────────────────┘  └─────────────────┘ │
│ │   title Text   │  ┌─ Bindings ──┬─ States ──┬─ Loc ─────┐                    │
│ │   menu Stack   │  │ hp_bar.val ← game.player.hp          │                    │
│ │    Resume ◀    │  │ name.text  ← game.player.name        │                    │
│ │    Settings    │  │                                      │                    │
│ │    Quit        │  └──────────────────────────────────────┘                    │
│ └───────────────┘                                                               │
└───────────────────────────────────────────────────────────────────────────────┘
```

Left palette drops widgets onto the canvas. Hierarchy shows the tree. Inspector is reflection-driven (Phase 3 §inspector pattern). Canvas simulates at a chosen resolution / scale / locale so designers can spot overflow at 4K with German strings. Edits route through Phase 6's command stack.

### 4.1 Preview vs edit modes

- **Edit mode**: widgets don't respond to input; clicking selects; dragging moves (with layout constraints respected); handles resize margin/padding.
- **Preview mode**: the subtree runs for real with fake binding data (`Inspector` can populate a binding with a dummy value for preview). Animations play. Gamepad nav can be tested with the actual framework.

This is the same split Phase 7 has for Play-in-Editor; the UI designer is a miniature PIE with scope restricted to one `UiPrefab`.

## 5. Data binding

A binding is a path from a widget property to a reflected value somewhere else:

```rust
pub struct Binding {
    pub source: BindingSource,   // reflected field path, ECS query, or Script value
    pub target: WidgetPath,      // "button.label.text"
    pub mode: BindingMode,       // OneWay, OneWayToSource, TwoWay
    pub converter: Option<ConverterId>,  // f32 → percent string, etc.
}
```

Sources:

- `Reflect("game.player.hp")` — a path resolved against the reflected world via Phase 2's registry. Read per frame or via property-change notifications.
- `Ecs(query)` — an ECS query (e.g. `Query<&Name, With<Enemy>>`) feeding a `List` widget. The list is virtualized so 10k enemies on a minimap list don't allocate 10k widgets.
- `Script(expr)` — a WASM-evaluated expression from Phase 7's scripting, for cases where a raw field path isn't enough. Gate this behind a deliberate opt-in: most bindings shouldn't need script.

Change detection drives redraw. A widget subscribes to its binding sources; when a source ticks its reflection-change counter, the subtree is marked dirty. No dirty widgets → no redraw that frame. This is the retained-mode payoff: a stable HUD with nothing changing costs zero CPU.

### 5.1 Binding in the designer

In the Inspector, every property has a binding drop target. Drag a reflected field from the inspector on any entity, or any component shown in the scene view, into a widget property slot → a binding is created. The designer validates types (trying to bind a `Transform` into a `Text.text` is rejected with a specific error).

## 6. Styles

Styles are reusable named bundles of visual properties. They live in `.rstyle` assets:

```ron
// assets/ui/game.rstyle
(
    styles: {
        "pause.root": (
            padding: (24, 24, 24, 24),
            background: Color("#0a0a0aee"),
            corner_radius: 12.0,
            align_items: Center,
            justify_content: Center,
        ),
        "pause.button": (
            padding: (12, 24, 12, 24),
            background: Color("#1a1a1a"),
            background_hover: Color("#2a2a2a"),
            background_pressed: Color("#101010"),
            background_focus: Color("#2a2a4a"),
            text_color: Color("#e0e0e0"),
            font: "ui.body",
            corner_radius: 6.0,
        ),
        "pause.button.danger": (
            inherit: "pause.button",
            background_hover: Color("#7a2a2a"),
        ),
    },
)
```

Rules:

- **No selectors, no cascade.** `inherit: "name"` is the only inheritance mechanism — single-parent, no multiple. Resolves at load time.
- **State variants** (`background_hover`, `background_pressed`, `background_focus`, `background_disabled`) live in the style itself, not via selectors. The widget's state machine picks among them.
- **Hot-reload.** Phase 5's file watcher fires on `.rstyle` edits; styles re-resolve without restarting Play-in-Editor. A designer can tweak colors while the game runs.

## 7. Animation

State-transition animations: widgets have states (`default`, `hover`, `pressed`, `focus`, `disabled`), and transitioning between states tweens properties over a duration with a curve.

```rust
pub struct StateTransition {
    pub from: StateName,         // "default"
    pub to: StateName,           // "hover"
    pub duration: Duration,
    pub curve: EaseCurve,        // Linear | EaseIn | EaseOut | EaseInOut | Spring(k, d)
    pub property: StyleProperty, // Opacity, Scale, Background, TextColor, Margin, ...
}
```

The runtime interpolates between the `from` and `to` style values over the duration. Multiple properties can animate simultaneously; all transitions on a state entry start in the same frame.

This covers ~90% of game UI animation needs. What it deliberately does not cover:

- Keyframed timelines with multiple tracks, retargeting, dopesheets.
- Skeletal animation of UI nodes.
- Per-character text reveal effects beyond a single property fade.

Those belong in **Phase 19 (Complex Animation Timeline)**, which is where the animation editor proper lives. Phase 15's tweens handle buttons, panel transitions, and notification slides. Anything fancier is scripted or waits for Phase 19.

## 8. Focus navigation

Gamepads and keyboards need directional nav: pressing Down on a button moves focus to the button below it. This requires a spatial graph the framework builds from the laid-out widget tree.

```rust
pub struct FocusGraph {
    edges: HashMap<WidgetId, FocusNeighbors>,
}
pub struct FocusNeighbors {
    pub up: Option<WidgetId>,
    pub down: Option<WidgetId>,
    pub left: Option<WidgetId>,
    pub right: Option<WidgetId>,
    pub next_tab: Option<WidgetId>,
    pub prev_tab: Option<WidgetId>,
}
```

After layout each frame (only when the tree changed), walk focusable widgets, compute edges by nearest-widget-in-direction from each widget's rect. Designers can override any edge manually in the Inspector (the `focus.up / down / left / right` slots in the mockup above).

Input mapping:

- Gamepad D-pad / left stick → directional.
- Keyboard arrows → directional.
- Keyboard Tab / Shift+Tab → tab order (default: left-to-right, top-to-bottom).
- Gamepad A (South button) / keyboard Enter / Space → activate focused widget.
- Gamepad B (East button) / keyboard Escape → "back" action, emits a configurable event.

Focus is always visible when navigated by keyboard or gamepad; the focus style variant is applied. When the last input was mouse, focus style is suppressed unless a widget is hovered. This matches every shipped console UI convention.

## 9. Input routing

UI and game share input. The rule: **UI consumes input when a focusable UI element has focus and the event is one UI knows how to handle.**

```rust
pub enum EventResponse {
    Handled,         // widget consumed; stop propagation
    Unhandled,       // bubble up
    PassThrough,     // explicitly route to gameplay (e.g. a HUD should never eat WASD)
}
```

Practical routing:

- Pause menu is open → UI focus root is the menu; WASD and mouse clicks are consumed. Gameplay gets nothing.
- HUD is visible but nothing is focused → HUD widgets return `PassThrough` for movement keys; gameplay gets them. Mouse clicks on a HUD button still consume (buttons are always focusable on hover).
- Text input field is focused → UI consumes all keyboard until blur, including the key that would otherwise pause.

The gameplay side reads input through `Input` resources; the UI inserts its own layer that sits above in the dispatch chain and can mark events consumed. No magic — it's a stack with an explicit `consumed` flag.

## 10. Localization

Every user-visible string routes through Phase 13's `t!()` macro. In `.rui` files, any `text:` field supports three forms:

```
text: Literal("OK")                     // debug only; CI forbids in shipped assets
text: Loc("menu.pause.resume")          // i18n key lookup
text: Bind("game.player.name")          // binding source
```

Text rendering uses the same font-stack resolver as the editor (Phase 13 §5) so CJK, Cyrillic, and Arabic glyphs fall through to language-specific fonts. BiDi in game UI: for 1.0, support LTR only; RTL game UI is a Phase 16+ item, mirroring Phase 13's decision on the editor.

The designer's canvas has a locale dropdown so designers can preview German (longest words) and Japanese (different line heights) without changing the system locale.

## 11. Accessibility via AccessKit

The editor uses `AccessKit` for screen-reader support (Phase 13 §4). The game UI framework does the same, exposing a parallel accessibility tree:

- Every widget emits an `accesskit::Node` with role, label (resolved localized string), value, state, and bounds.
- Focus changes in the UI propagate to AccessKit; a screen reader announces them.
- `aria-like` properties: `Button` has `Role::Button`, `Slider` has `Role::Slider` with min/max/value, `Toggle` has `Role::CheckBox` with `Toggled`.

Game developers rarely think about this; shipping it on by default catches the 80% case without asking them to. The CI enforcement from Phase 13 (no unlabeled interactive widget) extends: `.rui` assets fail validation if a `Button` has no `text:` or no `accessibility_label:` override.

## 12. Runtime: zero `egui` in shipped binary

The `rustforge-core` crate gains a `ui` feature. The editor uses `editor` feature, which depends on `egui`. Game builds use `ui` but not `editor`; therefore no `egui`.

Enforcement:

- `cargo bloat --release --crates` run on a stripped sample game binary must show no symbols from `egui`, `egui-wgpu`, or `egui_dock`.
- CI runs this check on every PR as part of the Phase 13 quality gates.
- Widget implementations in `rustforge-core/src/ui/` never import `egui`. A grep-based check fails the build if they do.

The editor's UI Designer does use `egui` — that's fine; the panel itself is editor-only. The preview it shows is drawn with the real `ui` framework's renderer, inside an offscreen target sampled into an `egui::Image`. The designer sees the same pixels the game will.

## 13. Build order within Phase 15

Each step is independently shippable.

1. **Widget tree + `taffy` layout + a fixed set of core widgets** (§2) — `Panel`, `Stack`, `Text`, `Image`, `Button`. Rendered via wgpu through the existing draw-list path. No styles, no bindings, no designer yet.
2. **Style resolution + `.rstyle` loader + state variants** (§6). Built-in styles first, hot-reload second.
3. **`.rui` asset format + loader + `UiPrefab` spawn** (§3). Runtime can render a `.rui` file with styles.
4. **Input routing + focus graph + gamepad nav** (§§8–9). Pause-menu scenario works end-to-end with a controller.
5. **Data bindings** (§5) — reflected field paths first, ECS queries second, script bindings deferred until proven needed.
6. **Animations** (§7) — state transitions with tween curves.
7. **Localization plumbing** (§10) — `Loc(...)` in `.rui`, `t!()` at runtime, CI checks.
8. **Accessibility bridge** (§11) — AccessKit tree, focus announcements.
9. **UI Designer panel scaffolding** (§4) — `AssetEditor` registration, canvas, hierarchy, inspector on a read-only view.
10. **Designer edit operations** — widget add/remove/reparent, property edits through the Phase 6 command stack.
11. **Designer bindings + states editors** — drag-to-bind, state-machine editor.
12. **Designer preview mode** — fake binding data, live animation.
13. **Runtime size enforcement** (§12) — `cargo bloat` gate in CI.
14. **Extended widgets** — `Grid`, `Toggle`, `Slider`, `Progress`, `List` (virtualized), `Scroll`.

## 14. Scope boundaries — what's NOT in Phase 15

- ❌ **Full CSS.** Styles are named bundles with single-parent inheritance. No selectors, cascade, specificity, `!important`, media queries, or `calc()`.
- ❌ **Visual scripting of UI logic.** On-click actions dispatch to named handlers (ECS systems or script functions). No node-graph "on click, do these ten things" editor.
- ❌ **3D widgets / worldspace UI.** All UI in Phase 15 is screen-space. Worldspace UI (billboarded menus, VR) is a separate future phase. The widget tree can render to an offscreen texture a 3D mesh samples, but that's the integration; Phase 15 does not ship worldspace layout or focus.
- ❌ **Complex animation timeline.** State transitions only. Multi-track timelines, keyframe authoring, retargeting → **Phase 19**.
- ❌ **RTL / BiDi text in game UI.** Infrastructure allows it; Phase 15 ships LTR only.
- ❌ **In-game UI debugger / live inspector.** The Designer is the authoring tool. A live runtime inspector is a plausible future phase, not this one.
- ❌ **HTML / web rendering.** No `<div>`, no WebView. If a game needs to embed a browser, that's a platform-specific integration outside Phase 15's scope.
- ❌ **Vector / SVG assets.** Images are raster textures. Vector support is a Phase 16+ item.
- ❌ **Rich text / inline markup beyond the basics.** Paragraph text with one font, one color, one size, and line-wrap. Mixed inline styles (bold spans, inline icons, clickable links) are a future extension on `Text`.
- ❌ **`egui` interop for game builds.** Not "avoided where possible" — not shipped. Period.

## 15. Risks & gotchas

- **Widget trait ergonomics drift.** Making `Widget` too generic invites boilerplate in every widget; too specific and extension is painful. The trait above is the compromise. Resist growing it. If a widget needs something unusual, hang a side-channel resource off the `UiContext` rather than adding a trait method everyone has to implement.
- **`taffy` version churn.** `taffy` is the layout engine; upstream has shipped breaking changes historically. Pin a version, gate upgrades behind the integration test suite, and do not expose `taffy` types in `rustforge-core`'s public API — always wrap.
- **Binding source liveness.** A binding points at `game.player.hp`; the player entity despawns. The widget must render something (zero? previous value? empty?) rather than panic. Rule: every binding resolves through an `Option<ReflectValue>`; widgets display their widget-specific default on `None`.
- **`.rui` schema evolution.** Adding a widget type or renaming a property invalidates on-disk assets. Use the Phase 1 migration pattern: each `.rui` file carries a schema version; migration shims upgrade on load; a CLI command `rustforge ui migrate` runs them eagerly across a project.
- **Style hot-reload races.** Designer is editing `.rstyle`; Play-in-Editor is running; style reloads mid-frame. Phase 5's file-watcher already defers to the frame boundary — respect that. Never swap a style dict mid-widget-draw.
- **Focus graph cost.** Rebuilding the graph every layout is fine for small menus, quadratic for huge lists. Virtualized lists (`List` widget with item recycling) register a single focusable host and manage internal item focus themselves; don't emit 10k widgets to the graph.
- **Animation & layout interaction.** Animating `margin` triggers a full layout pass each frame of the animation — expensive on deep trees. Prefer animating `opacity`, `scale` (transform-level), and `background` color, which don't invalidate layout. Document this and surface a Designer warning when a layout-affecting property is picked as an animated target.
- **Input swallowing surprises.** A text input steals every keystroke including Escape. Convention: Escape always routes to the "back" action first, text fields included; a text field that wants to consume Escape (rare) must opt in explicitly. Otherwise a user gets trapped in a field unable to close the menu.
- **Localization string drift in `.rui` assets.** A `.rui` file references `menu.pause.resume`; someone renames the key in `en-US.ftl` without updating the asset. Phase 13's "new key on change" rule helps; in addition, a CI check cross-references every `Loc(...)` in every `.rui` asset against `en-US.ftl` and fails on unknowns.
- **AccessKit platform coverage.** Same caveat as Phase 13 §4: Linux coverage is thinnest. Label every widget anyway; the data is portable even if the backend isn't.
- **Designer drawing a framework that's drawing itself.** The preview uses the real UI framework inside the editor. A crash in the UI runtime brings down the designer. Mitigate: sandbox the preview render in a catchable scope, treat any panic as "preview failed" and show the error in-place rather than unwinding the editor.
- **`ui` feature without `editor` feature does not compile the designer.** Obvious but worth codifying: the designer lives in `rustforge-editor`, never leaks into `rustforge-core`. Build the game-only target and assert success in CI.
- **Font atlas pressure from many locales.** Loading CJK + Latin + Cyrillic glyph ranges at runtime is megabytes of texture. Use the editor's font-stack resolver to lazily rasterize glyphs on demand, not the full range upfront.
- **Mixed mouse + gamepad sessions.** A player plays with a controller, occasionally alt-tabs, moves the mouse. The focus highlight flickers if the "suppress focus when last input is mouse" rule is applied per-event. Debounce: suppress focus highlight only after N ms of mouse-only input; restore immediately on any gamepad / keyboard nav event.

## 16. Exit criteria

Phase 15 is done when all of these are true:

- [ ] `rustforge-core/src/ui/` ships behind a `ui` feature; core widget set (`Panel`, `Stack`, `Grid`, `Text`, `Image`, `Button`, `Toggle`, `Slider`, `Progress`, `List`, `Scroll`) renders and unit-tests green.
- [ ] `taffy` drives all layout; no CSS parser or selector engine exists in the crate.
- [ ] `.rui` assets load, validate schema version, and migrate on version bumps.
- [ ] `.rstyle` assets load, hot-reload through Phase 5's watcher, and support single-parent `inherit`.
- [ ] UI Designer panel opens on `.rui` double-click, shows canvas / hierarchy / inspector, supports drag-to-bind, and routes edits through the Phase 6 command stack.
- [ ] Designer preview mode runs animations and bindings with fake data; editing a style in a `.rstyle` hot-reloads into the preview.
- [ ] Reflected-field bindings update the UI when the source value changes; no redraw happens when nothing is dirty.
- [ ] State transitions animate `opacity`, `scale`, `background_color`, and `text_color` via named ease curves.
- [ ] Gamepad directional focus navigation works on a sample pause menu with no manually-authored focus edges; manual overrides respected when present.
- [ ] UI consumes input when focused; a HUD without focus passes movement keys through to gameplay.
- [ ] Every user-visible string in every shipped `.rui` asset routes through `t!()` (CI check on `Loc` vs `en-US.ftl`).
- [ ] AccessKit tree is emitted for the runtime widget tree; focus changes announce; every interactive widget has a label (CI enforced).
- [ ] A sample game binary built with `ui` but without `editor` contains no `egui`, `egui-wgpu`, or `egui_dock` symbols (CI `cargo bloat` check).
- [ ] `rustforge-core` still builds with neither `ui` nor `editor` for server / headless scenarios.
- [ ] Integration test: launch the editor, open a sample `.rui` file, drop a Button onto the canvas, bind its `text` to a reflected field, save, spawn the prefab in a running game, assert the button renders with the bound value.
