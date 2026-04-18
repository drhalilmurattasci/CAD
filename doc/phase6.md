# Phase 6 — Undo/Redo Command System

Up to now, every edit has been destructive: the inspector writes directly, gizmos write directly, hierarchy mutations write directly. Ctrl+Z doesn't exist. Phase 6 retrofits a proper command system across every mutation path in the editor.

This phase is smaller in surface area than 4 or 5, but it touches **every panel** you've already built. That's the hard part.

## Goals

By end of Phase 6:

1. Every scene-mutating operation goes through a `Command` trait with `execute` / `undo` / `redo`.
2. A global `CommandStack` supports unbounded undo (bounded by memory), redo-after-undo, and branch truncation on new commands.
3. Ctrl+Z / Ctrl+Y (and Ctrl+Shift+Z) work from anywhere in the editor.
4. Continuous edits (gizmo drag, slider drag, color picker drag) coalesce into a single undoable command.
5. A History panel shows the stack and lets you jump to any point.

## 1. The central design choice — what is a Command?

Two viable approaches. The choice ripples through every decision below.

### Option A: Full-snapshot commands

Each command stores "world state before" and "world state after" (for the affected subset of entities) and restores by overwriting. Simple. Works for everything. Expensive in memory for large ops.

### Option B: Inverse-operation commands

Each command stores the *operation* and computes its inverse. `TranslateCommand { entity, delta }` inverts to `TranslateCommand { entity, -delta }`. Compact. Fragile — every operation needs a hand-written inverse, and composite operations are tricky.

### Recommendation: **hybrid, leaning on snapshots**

- For most edits (transform, component field change, add/remove component, spawn/despawn entity, reparent): snapshot the **affected entity's serialized state** before and after. Reuse Phase 4's serialization — you already have it.
- For selection changes, panel state, camera position: lightweight structural commands (not undoable by default — see §5).
- For bulk operations (delete 100 entities): snapshot once as a scene fragment, restore by re-deserializing.

Snapshots use the reflection/serialization path already built. No new invariants to maintain. Memory cost is manageable because each snapshot is a small `ron::Value` or serialized bytes per affected entity, not the whole world.

## 2. Command trait

```
crates/rustforge-editor/src/commands/
├── mod.rs                # Command trait, CommandStack, CommandContext
├── transform.rs          # TransformCommand (gizmo drag)
├── entity.rs             # SpawnEntity, DespawnEntity, ReparentEntity
├── component.rs          # AddComponent, RemoveComponent, EditComponentField
├── asset.rs              # (stubs — asset ops stay unversioned per Phase 5 decision)
├── composite.rs          # CompositeCommand (bundles N commands as one undo unit)
├── coalesce.rs           # Coalescer trait — merge consecutive commands of same kind
└── stack.rs              # CommandStack internals: undo/redo vecs, transaction state
```

```rust
pub trait Command: Send + Sync + 'static {
    fn label(&self) -> Cow<'static, str>;
    fn execute(&mut self, ctx: &mut CommandContext) -> Result<()>;
    fn undo(&mut self, ctx: &mut CommandContext) -> Result<()>;

    /// Optional: merge `other` into `self` if they're the same kind and
    /// occurred close in time. Return true if merged (other is dropped).
    fn try_coalesce(&mut self, _other: &dyn Command) -> bool { false }
}

pub struct CommandContext<'a> {
    pub world: &'a mut World,
    pub selection: &'a mut SelectionSet,
    pub registry: &'a ComponentRegistry,
    pub assets: &'a AssetRegistry,
}
```

Key design points:

- **Mutable `execute`/`undo`** — commands can lazily compute data on first execute (e.g., capture before-state that wasn't known at construction time).
- **`CommandContext` bundles everything a command needs.** Never smuggle state through globals. This makes commands testable.
- **Errors are real.** A command can fail (entity gone, asset gone). Undo stack must handle failed commands gracefully — log, remove from stack, don't corrupt.

## 3. CommandStack

```rust
pub struct CommandStack {
    undo: Vec<Box<dyn Command>>,
    redo: Vec<Box<dyn Command>>,
    transaction: Option<TransactionState>,
    max_memory_bytes: usize,    // optional cap for eviction
    max_commands: usize,        // hard cap, default 1000
}

impl CommandStack {
    pub fn push(&mut self, cmd: Box<dyn Command>);   // execute + record
    pub fn undo(&mut self, ctx: &mut CommandContext) -> Result<()>;
    pub fn redo(&mut self, ctx: &mut CommandContext) -> Result<()>;

    pub fn begin_transaction(&mut self, label: &str);
    pub fn end_transaction(&mut self);
    pub fn abort_transaction(&mut self);

    pub fn clear(&mut self);   // on scene load/close
}
```

Rules:
- `push` clears the redo stack (standard branch-cutoff behavior).
- Pushing during a transaction appends to the transaction, not to the main stack.
- `end_transaction` wraps the transaction's commands in a `CompositeCommand` and pushes it as one undo unit.
- On scene load / new scene: `clear()` — can't undo across scenes.

### 3.1 Memory eviction

Keep two caps: max command count (1000 default) and max memory (500 MB default). Evict from the *old* end when either is exceeded. Prefer count-based as primary; memory-based as backstop for commands that store big snapshots.

## 4. Coalescing — the UX problem

Without coalescing, dragging a gizmo for 1 second creates 60 commands. User hits Ctrl+Z once, entity moves 1/60th of a second back. Useless.

Three coalescing patterns, each for a different situation:

### 4.1 Explicit transaction (best)

Drag start → `begin_transaction("Move entity")`. Drag updates mutate the world directly, no command pushed. Drag end → capture before/after, push one `TransformCommand`, `end_transaction()`.

This is the Phase 3 gizmo design ("before/after pattern") becoming real.

**Use for:** gizmo drags, slider drags in inspector, color picker drags, multi-step wizard flows, any "compound operation with clear start/end events."

### 4.2 Time-window coalescing

For cases without clear start/end events (e.g., typing in a name field):

```rust
pub struct EditStringCommand {
    entity: Entity,
    component: TypeId,
    field_path: String,
    before: String,
    after: String,
    timestamp: Instant,
}

impl Command for EditStringCommand {
    fn try_coalesce(&mut self, other: &dyn Command) -> bool {
        if let Some(o) = other.as_any().downcast_ref::<EditStringCommand>() {
            if o.entity == self.entity
                && o.field_path == self.field_path
                && self.timestamp.elapsed() < Duration::from_millis(500)
            {
                self.after = o.after.clone();
                self.timestamp = o.timestamp;
                return true;
            }
        }
        false
    }
}
```

`CommandStack::push` calls `try_coalesce` on the top of the stack first; only pushes a new command if coalesce returns false.

**Use for:** text field typing, numeric field arrow-up/arrow-down, any rapid-fire small edits.

### 4.3 No coalescing

`SpawnEntity`, `DespawnEntity`, `AddComponent`, `RemoveComponent` — each is discrete and should be its own undo unit.

### 4.4 Rule of thumb

**Transactions** when the UI event boundary is obvious (drag start/end, wizard begin/end). **Time-window coalescing** when the UI is edit-as-you-type. Otherwise, no coalescing.

## 5. What's undoable, what isn't

This is contentious. Different editors disagree. Pick a policy and be consistent.

### Undoable (goes through commands):
- Transform changes (position/rotation/scale).
- Component field edits.
- Component add/remove.
- Entity spawn/despawn.
- Reparenting.
- Rename.
- Prefab instantiate / unpack.
- Bulk operations (duplicate, delete selected).

### Not undoable (direct mutation):
- Selection changes.
- Viewport camera movement.
- Gizmo mode switching (W/E/R).
- Panel layout / dock state.
- Play/pause/step (Phase 7).
- Anything in the Content Browser that affects disk (delete, rename, import) — disk operations are the user's responsibility; don't pretend to undo them.
- Any asset .meta edit — reimports happen externally, outside the command model.

### Unity's selection-is-undoable approach is wrong

Having Ctrl+Z undo your selection is annoying and confuses the mental model. Don't do it. If the user wants "go back to what I had selected," they can rely on visual memory. This is a non-negotiable opinion — the small convenience isn't worth the UX muddle.

## 6. Retrofit — touching every existing panel

This is the actual work of Phase 6. Every place in Phases 3-5 that mutates the world needs to route through a command.

Touchpoints:

### Inspector (Phase 3)
- Each field edit → `EditComponentFieldCommand` with time-window coalescing.
- "Add Component" button → `AddComponentCommand`.
- "Remove Component" button → `RemoveComponentCommand`.

### Hierarchy (Phase 3)
- Context menu: Create / Duplicate / Delete → commands.
- Drag-drop reparent → `ReparentCommand`.
- Rename → `RenameCommand` (time-window coalescing on the text field).

### Gizmos (Phase 3)
- Drag start → `begin_transaction`.
- Drag end → push `TransformCommand { before, after }`, `end_transaction`.

### Scene I/O (Phase 4)
- New/Open scene → `stack.clear()`.
- Save doesn't touch the stack.

### Content Browser (Phase 5)
- Drop mesh into viewport → `SpawnEntityCommand`.
- Instantiate prefab → `InstantiatePrefabCommand` (which is really a composite: spawn N entities with known IDs).
- Assign `AssetRef` by drop → `EditComponentFieldCommand`.

Do this retrofit systematically, one panel at a time. After each, verify: can you Ctrl+Z every operation in that panel?

## 7. Keyboard shortcuts

```
Ctrl+Z          — undo
Ctrl+Y          — redo  (Windows/Linux)
Ctrl+Shift+Z    — redo  (macOS + universal fallback)
```

Register these globally in `input/shortcuts.rs` (stubbed back in Phase 3). Shortcuts fire regardless of panel focus *except* inside text fields — egui's text input swallows Ctrl+Z for its own per-widget undo. That's fine; most editors do this.

Edit menu (new):
```
Edit
  Undo [Move Entity]       Ctrl+Z
  Redo                     Ctrl+Y
  ─────
  Cut                      Ctrl+X
  Copy                     Ctrl+C
  Paste                    Ctrl+V
  Duplicate                Ctrl+D
  Delete                   Delete
```

Undo/Redo menu items show the label of the next command (`Undo [Move Entity]`). This is a small detail that makes the feature feel polished.

Cut/Copy/Paste/Duplicate for entities is worth doing now since it's essentially command composition:
- Copy: serialize selected subtrees to internal clipboard (reuse scene serializer).
- Paste: deserialize into world → `CompositeCommand { SpawnEntity, ... }`.
- Duplicate: Copy + Paste in one step.

## 8. History panel

```
crates/rustforge-editor/src/panels/
└── history.rs
```

Optional-feeling but high-value panel. Shows the undo stack top-down:

```
┌─ History ──────────────┐
│  ▸ Move Player          │
│  ▸ Add RigidBody        │
│  ▸ Set albedo color     │
│  ▸ Set albedo color     │  ← 3 coalesced here
│  ▸ Spawn Light ★        │  ← current position
│    (redo: Despawn Box)  │
│    (redo: Move Box)     │
└─────────────────────────┘
```

Clicking any row undoes or redoes up to that point. Shows coalesced groups expanded if user clicks.

Implementation is trivial once the stack exists — just a panel that reads `stack.undo` and `stack.redo`.

## 9. Testing

Undo systems fail silently. A bug in `TransformCommand::undo` that subtly drifts the rotation won't crash — it'll just make user edits unreliable over time. Test aggressively.

Patterns:

### 9.1 Round-trip property test

```rust
#[test]
fn transform_cmd_roundtrip() {
    let mut world = test_world();
    let e = world.spawn((Transform::default(),));
    let before = world.get::<Transform>(e).unwrap().clone();

    let mut cmd = TransformCommand::new(e, before, moved_transform);
    cmd.execute(&mut ctx(&mut world));
    cmd.undo(&mut ctx(&mut world));

    assert_eq!(*world.get::<Transform>(e).unwrap(), before);
}
```

Do this for every command type.

### 9.2 Random-walk test

Spawn 50 entities, apply 1000 random commands, then undo all 1000, then redo all 1000. Final state must equal post-execute state. Initial state (after all undos) must equal pre-execute state. This catches ordering bugs, coalescing bugs, and lifecycle bugs that unit tests miss.

### 9.3 Cross-command invariants

Some combinations are tricky:
- Spawn entity A → add component to A → undo spawn → component snapshot is for a now-dead entity. Redo spawn → recreate entity, replay component add. Does the component reach the right entity?
- Reparent A under B → despawn B → undo despawn of B. Is A still B's child?

Write specific tests for these.

## 10. Build order within Phase 6

1. **`Command` trait + `CommandStack`** — infrastructure only, no integrations. Unit tests with a dummy `MockCommand`.
2. **`TransformCommand`** — simplest real command. Wire into gizmo drag end.
3. **Transaction API** — `begin/end_transaction`, `CompositeCommand`. Wire gizmo drags through it.
4. **`SpawnEntityCommand` + `DespawnEntityCommand`** — with full snapshot for undo.
5. **`ReparentCommand`** — invariant-preserving.
6. **`EditComponentFieldCommand`** + time-window coalescing. Wire into inspector primitives.
7. **`AddComponentCommand` + `RemoveComponentCommand`** — snapshot component bytes on remove for undo.
8. **`RenameCommand`** — trivial; good coalescing test case.
9. **Keyboard shortcuts + Edit menu** — Ctrl+Z/Y/Shift+Z, menu with live labels.
10. **Content Browser integrations** — drag-drop spawn, prefab instantiate, asset-ref assign.
11. **Cut/Copy/Paste/Duplicate** — clipboard infrastructure, composite commands.
12. **History panel** — read-only view first, then clickable jump.
13. **Random-walk test** — exhaustive validation.

## 11. Scope boundaries — what's NOT in Phase 6

- ❌ Per-panel undo stacks. One global stack only. Per-panel undo (e.g., undo in script editor separate from scene undo) is complex and low-value.
- ❌ Collaborative / distributed undo (OT, CRDT). Single-user only.
- ❌ Persistent undo across editor restarts. Stack clears on close. Saving undo history to disk would tie disk format to internal command representation — brittle.
- ❌ Asset operation undo (delete asset, rename asset). Already excluded in Phase 5. Keep that way.
- ❌ Play-mode undo. Phase 7 handles play/pause; command stack is disabled in play mode (see §12).
- ❌ Undo of source control operations, external file edits, or anything outside the editor's writes.

## 12. Interaction with Play-in-Editor (forward-looking)

Phase 7 is PIE. The command stack and play mode need to agree on behavior:

- Entering play mode: snapshot world state, **freeze command stack** (don't record runtime changes).
- Exiting play mode: restore snapshot, command stack unchanged.
- Result: Ctrl+Z in play mode does nothing (or undoes last edit-mode change, clarify in Phase 7).

Don't build this in Phase 6. Just make sure the design doesn't prevent it — `CommandStack::set_enabled(bool)` is enough.

## 13. Risks & gotchas

- **Entity IDs change across undo/redo.** You despawn entity `Entity(5)`, then respawn it — hecs might give you `Entity(6)`. If any *other* undoable command references `Entity(5)`, it now points to nothing. Solution: commands store `SceneId` (Phase 4), not `Entity`. Resolve to `Entity` at execute/undo time. Commands issued before SceneId exists on an entity fail cleanly.
- **Snapshot explosion.** Add-component-to-100-entities at once → 100 snapshots in one `CompositeCommand`. Memory spikes. Real but rare; revisit only if measured.
- **Coalescing races.** Field edit coalesces with previous, but a selection change happened in between. Is the coalesce valid? Rule: coalescing checks `entity + field_path + time`, nothing about selection. Selection isn't part of command state.
- **Listener invalidation.** Dirty-tracking from Phase 4 marks scene dirty on mutation. Commands mutate. Make sure the dirty flag gets set exactly once per command (execute OR redo, NOT both via double-counting).
- **Inspector displays stale data during undo.** egui polls each frame from the world; should naturally refresh. But if inspector caches anything (e.g., a text buffer for mid-edit), undo bypasses it. Rule: inspectors must read from world every frame unless actively being edited.
- **Gizmo middle-of-drag undo.** User holds Ctrl+Z during a gizmo drag. Transaction isn't ended yet. Rule: ignore undo input while a transaction is open. Or: abort transaction on undo. Go with ignore — less surprising.
- **Clipboard format evolution.** Cut/paste uses scene serialization. A command stored on the undo stack as a serialized snapshot will fail to deserialize after a format version bump. Clear stack on version mismatch; don't try to migrate.
- **Hecs iteration-while-mutating.** Some commands iterate entities while spawning/despawning (e.g., DuplicateCommand). hecs borrow rules bite here. Collect entities first, then mutate. Same discipline as Phase 3 inspector.

## 14. Exit criteria

Phase 6 is done when all of these are true:

- [ ] Ctrl+Z / Ctrl+Y work throughout the editor and undo/redo the *last logical operation* (not per-frame deltas).
- [ ] Gizmo drag produces exactly one undo entry, regardless of drag duration.
- [ ] Slider and text field drags coalesce within a time window.
- [ ] Every panel (Inspector, Hierarchy, Content Browser) routes mutations through commands.
- [ ] Cut / Copy / Paste / Duplicate work on entity selections and are undoable.
- [ ] History panel lists operations and can jump to arbitrary points.
- [ ] Edit menu shows the label of the next undo/redo operation.
- [ ] Round-trip tests pass for every command type.
- [ ] Random-walk test (1000 random ops, undo all, redo all) passes.
- [ ] Command stack clears on New Scene / Open Scene.
- [ ] Failed commands (e.g., target entity despawned) don't corrupt the stack.
- [ ] Memory and command-count caps evict old commands cleanly.
- [ ] `rustforge-core` still builds without the `editor` feature (command system is editor-only).
