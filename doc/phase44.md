# Phase 44 — Dialogue, Narrative & Quest Systems

Unreal ships a scripting language, a sequencer, a behavior tree, and a UMG widget system — and then tells you to go buy Yarn Spinner or Articy Draft when you want to actually write a conversation. Unity is worse: you pick between a half-dozen asset-store dialogue plugins, each with its own file format, its own editor, its own opinion about localization. RustForge refuses to punt. Phase 44 bakes dialogue authoring, story-variable state, quest graphs, and a journal UI straight into the engine, as first-party crates on top of the node-graph widget (Phase 20), the UI framework (Phase 15), the behavior-tree blackboard (Phase 26), the facial-animation pipeline (Phase 35), and the localization macro (Phase 13).

The premise: narrative is not a plugin. It's a core shipping concern for ~60% of games, and treating it as optional is how you end up with three incompatible dialogue systems in one studio. RF gives you one, it integrates with everything else in the engine, and the export/import path for translators is part of the tool, not a scraper script someone wrote at 2 AM before localization due date.

## Goals

1. **Node-graph dialogue authoring** reusing the Phase 20 widget — conversations as visual graphs, not YAML.
2. **Typed persistent story variables** readable from BT services, scripts, and data-bound UI.
3. **First-class localization**: every line passes through `t!()`, keys auto-generated and locked after translation.
4. **Voice-over + lip-sync**: attach an audio clip per line, preview with Phase 35 phonemes in-editor.
5. **Quest graphs** as a separate but tightly-coupled asset — state-machine driven, event-listening, bidirectional with dialogue.
6. **Default journal UI template** built on Phase 15, users restyle or replace.
7. **Translator workflow** that doesn't suck: CSV/XLIFF export, import with diff, untranslated-key highlighting.
8. **Optional LLM line nodes** (Phase 39) with deterministic fallback when the model is unavailable.
9. **Runtime save/restore** mid-conversation so save systems don't have to special-case dialogue.
10. **Ship the boring plumbing**, leave authorial ambition to designers.

---

## 1. Crate layout

```
rustforge-narrative/
  rustforge-story-vars/       # typed KV store, save hooks
  rustforge-dialogue-core/    # graph model, runtime, .rdialog serde
  rustforge-dialogue-editor/  # Phase 20 graph wrapper + preview panel
  rustforge-quest-core/       # quest graph, conditions, event listeners
  rustforge-quest-editor/     # Phase 20 graph wrapper for quests
  rustforge-journal-ui/       # Phase 15 widget template
  rustforge-narrative-i18n/   # key gen, CSV/XLIFF export/import
  rustforge-narrative-llm/    # optional Phase 39 bridge (feature-gated)
```

Opinion: `story-vars` is a separate crate because BT, UI, and scripts all depend on it but none of them should pull in the dialogue graph just to read a bool.

---

## 2. Story variables

Typed KV, globally accessible, persistent. No stringly-typed float sludge — each variable is declared in a project-wide `story_vars.ron` with a declared type.

```rust
pub enum StoryValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Enum { ty: EnumId, variant: u32 },
}

pub struct StoryVarKey(pub SmolStr);   // "chapter2.met_blacksmith"

pub trait StoryVarStore: Send + Sync {
    fn get(&self, key: &StoryVarKey) -> Option<StoryValue>;
    fn set(&self, key: &StoryVarKey, value: StoryValue) -> Result<(), TypeMismatch>;
    fn subscribe(&self, key: &StoryVarKey) -> StoryVarWatch;
}
```

Readers: BT services (Phase 26 blackboard has a `StoryVar` node that mirrors a key), UI data-binding (Phase 15 binds widget props to a `StoryVarKey`), Rhai/Lua scripts.

Saves: the store is a single resource, serialized as part of the save blob. Unknown keys (from older saves) are kept verbatim and warned; missing keys default per declaration.

Opinion: no unbounded bag of keys at runtime. The `.ron` declaration is authoritative and the editor validates every graph reference against it. Typos are a compile-time-ish error, not a shipped bug.

---

## 3. Dialogue graph model

Nodes:

| Node          | Purpose                                              |
| ------------- | ---------------------------------------------------- |
| `Line`        | A single spoken line (speaker + locale key + VO)     |
| `Choice`      | Player picks one of N options                        |
| `Branch`      | Conditional jump on story-var expression             |
| `SetVar`      | Assign a value to a story variable                   |
| `GetVar`      | Read into a local slot (for templated lines)         |
| `Condition`   | Gate on expression; true/false outputs               |
| `TriggerEvent`| Fire a named game event (kill-count, etc.)           |
| `Jump`        | Goto label / another dialogue asset                  |
| `End`         | Conversation terminates                              |
| `LlmLine`     | Phase 39 generative line with scripted fallback      |

`.rdialog` is RON, human-diffable, with stable node IDs (UUID v4) so localization keys don't drift when a node is moved on the canvas.

```ron
Dialogue(
    id: "convo.village.blacksmith.intro",
    nodes: {
        "n_01": Line(
            speaker: "blacksmith",
            key: "convo.village.blacksmith.intro.n_01",
            vo: Some("audio/vo/en/blacksmith_intro_01.ogg"),
            next: Some("n_02"),
        ),
        "n_02": Choice(options: [
            ChoiceOpt(key: "...greet", next: "n_03"),
            ChoiceOpt(key: "...leave", next: "n_end", gate: Some(Expr("!chapter2.met_blacksmith"))),
        ]),
        // ...
    },
    entry: "n_01",
)
```

---

## 4. Dialogue runtime

One `DialogueInstance` per active conversation. Driven by the game loop, not by its own thread.

```rust
pub struct DialogueInstance {
    asset: Handle<DialogueAsset>,
    cursor: NodeId,
    locals: SmallVec<[StoryValue; 4]>,
    vo_handle: Option<AudioHandle>,
    lip_sync: Option<LipSyncJob>,
}

pub enum DialogueStep {
    ShowLine { speaker: ActorId, text: String, vo: Option<AudioHandle> },
    OfferChoices(Vec<ChoiceView>),
    Done,
}

impl DialogueInstance {
    pub fn advance(&mut self, ctx: &mut NarrativeCtx, input: Option<ChoiceIdx>) -> DialogueStep;
    pub fn snapshot(&self) -> DialogueSnapshot; // for save system
    pub fn restore(snap: DialogueSnapshot, ctx: &NarrativeCtx) -> Self;
}
```

Save-mid-conversation is non-negotiable: `snapshot()` serializes cursor + locals + VO playback position. Players who save during a long monologue don't lose their place.

Runtime is single-threaded per instance but you may run many instances in parallel (overheard ambient NPC chatter).

---

## 5. Dialogue Editor panel

Reuses `rustforge-node-graph` (Phase 20). The right-hand inspector shows the selected node's fields; a bottom preview strip plays the VO and runs Phase 35 lip-sync on a bound face.

```
+-- Dialogue Editor — convo.village.blacksmith.intro -----------------+
| Nodes  | Canvas                                           | Inspect |
| Line   |  [n_01 Line]──────▶[n_02 Choice]                 | Node:   |
| Choice |    "Welcome.."         ├──▶[n_03 Line]           |  n_01   |
| Branch |                        └──▶[n_end End]           | Speaker |
| Set    |                                                  |  blk... |
| Get    |                                                  | Key     |
| Cond   |                                                  |  convo. |
| Event  |                                                  | VO clip |
| Jump   |                                                  |  [...]  |
| LLM    |                                                  | Locale: |
| End    |                                                  |  en ✓   |
+--------+--------------------------------------------------+---------+
| Preview:  [▶ Play VO]  [👄 off]  Lip-sync: phoneme track ok         |
|           "Welcome, traveler. You look like you've walked far."     |
+---------------------------------------------------------------------+
```

Opinion: no timeline view. Dialogue is a graph, not a Sequencer track — if you need cinematic timing, Phase 19 already does that and can invoke a dialogue asset at a cue point.

---

## 6. Localization integration (Phase 13)

Every `Line` and `ChoiceOpt` has a locale key derived from `{dialogue_id}.{node_id}.{slot}`. At first save, the key is registered into the project's localization catalog via the Phase 13 `t!()` macro registry. Once a translation exists in any non-source locale, the key is **locked** — renaming the node in the editor does not rename the key, only updates the display label. Hash-based drift detection compares the source-locale body against last-exported hash; mismatches mark the key "dirty" in the translator report.

Runtime call is plain `t!(key, params)` with the parameter slot map coming from the node's local variable captures (`{player_name}`, `{gold}`).

---

## 7. Voice-over + lip-sync

Per-line VO clip is an optional `Handle<AudioClip>` resolved through Phase 17. On `ShowLine`, the runtime plays the clip, gets back a `PlaybackHandle`, and pipes it into the Phase 35 phoneme extractor which drives the speaker's facial rig.

In the editor, the preview panel exposes:

- Play / scrub / loop VO
- Phoneme track visualizer (read-only, computed)
- Bind to a face rig for live lip-sync preview
- Mark clip "missing" / "WIP" / "final" per line

Missing-clip rendering at runtime: falls back to typewriter text advance at a configurable cps.

---

## 8. Quest graph model

Separate asset `.rquest`. Nodes represent **states** (not lines), edges represent **transitions** gated by conditions.

| Node           | Purpose                                            |
| -------------- | -------------------------------------------------- |
| `QuestState`   | Named state (`Locked`, `Active`, `Step1Done`, ...) |
| `Transition`   | Edge with condition expression                     |
| `Goal`         | Displayable subgoal (title + description key)      |
| `EventListen`  | External trigger binding (e.g. `EnemyKilled{id}`)  |
| `Reward`       | Payout on completion (xp, item, var set)           |
| `Terminal`     | `Complete` or `Failed` end state                   |

Default states always exist: `Locked → Active → Complete` and `Active → Failed` with user-authored transitions between.

```rust
pub enum QuestStatus { Locked, Active, Complete, Failed }

pub struct QuestInstance {
    asset: Handle<QuestAsset>,
    current: StateId,
    status: QuestStatus,
    goals: SmallVec<[GoalStatus; 4]>,
}
```

---

## 9. Quest conditions & event bus

Conditions are the same expression language used by dialogue branches, over:

- Story variables
- BT blackboard keys (read-only projection)
- Event counters (`events.enemy_killed["wolf"] >= 5`)
- Zone / time / custom user predicates

Event bus is a tiny in-process pub/sub (not ECS events, not the async channel bus from Phase 17 — a narrative-scoped fan-out). Listeners re-evaluate only the quests subscribed to that event kind; no whole-quest-list scans.

```rust
pub trait NarrativeEvent: 'static + Send + Sync {
    fn kind(&self) -> EventKind;
}

pub struct NarrativeBus { /* dashmap<EventKind, Vec<QuestHandle>> */ }
```

---

## 10. Quest ↔ dialogue handoff

Bidirectional and intentional:

- A dialogue `Branch` node can read `quest.status("rescue_blacksmith")`.
- A `SetVar` node can also do `quest.advance("rescue_blacksmith", "Step1Done")`.
- A quest `Transition` can specify "on entry, open dialogue `convo.blacksmith.thanks`".
- Rewards flow: completing a quest can set story variables that gate future dialogue.

No circular-callback hazards: dialogue drives quests synchronously; quest state transitions post events, they do not reach back into a running dialogue instance mid-step.

---

## 11. Quest Designer panel

```
+-- Quest Designer — quest.rescue_blacksmith -------------------------+
| Palette | Canvas                                      | Inspector   |
| State   |  [Locked]                                   | State:      |
| Goal    |    │ unlock: chapter2.met_blacksmith        |  Active     |
| Event   |    ▼                                        | Goals:      |
| Reward  |  [Active]──▶[Step1Done]──▶[Complete]        |  • talk to  |
| Term    |    │             ▲             │            |  • find key |
|         |    │ on timer    │ item.found  ▼            | Entry hook: |
|         |    └──▶[Failed]  │ "rusty_key" [Reward: xp] |  open dlg   |
|         |                                             |  convo.thx  |
+---------+---------------------------------------------+-------------+
| Events heard:  EnemyKilled, ItemPickedUp, ZoneEntered                |
| Preview:  [Simulate] [Fire event ▼]  current: Active                 |
+---------------------------------------------------------------------+
```

"Simulate" runs the quest against a scratch story-var store so designers can fire events without booting the game.

---

## 12. Journal UI template

A default Phase 15 widget tree shipped as `rustforge-journal-ui`. Three tabs: Active / Complete / Failed. Click a quest for detail: title, description, goals with checkboxes, reward preview. All strings go through `t!()`.

```
+-- Journal ---------------------------------------------------------+
| [Active] [Complete] [Failed]                                       |
|                                                                    |
|  > Rescue the blacksmith                                           |
|    The blacksmith was last seen near the western mine.             |
|      [x] Talk to the innkeeper                                     |
|      [ ] Find the rusty key                                        |
|      [ ] Enter the mine                                            |
|      Reward: 250 xp, Iron Dagger                                   |
|                                                                    |
|    Lost heirloom                                                   |
|    An old woman asked you to find her locket...                    |
|                                                                    |
+--------------------------------------------------------------------+
```

Users override via normal Phase 15 widget inheritance. The template is a reference, not a requirement.

---

## 13. Translator workflow

```
rf narrative export --format xliff --out loc/en.xlf
rf narrative export --format csv   --out loc/en.csv
rf narrative import --format xliff --in  loc/de.xlf
rf narrative diff   --base en --target de
```

- **Export**: all locked keys + current source text. Untranslated keys are included with empty target so translators see them.
- **Import**: validates key existence, warns on unknown keys, reports new / changed / stale per key.
- **Diff view** in the editor: a panel listing per-locale untranslated or stale keys with click-to-source-node.
- **Lockfile**: `loc/.keys.lock` records the hash per key so drift is explicit, not implicit.

XLIFF 1.2 is the export format for CAT tools; CSV is the escape hatch for designers who live in spreadsheets.

---

## 14. LLM integration (optional, feature-gated)

`LlmLine` node. Only active when `rustforge-narrative-llm` is compiled in and an `MlProvider` (Phase 39) is configured. Fields:

- Prompt template (`t!()`-localized)
- Context slots (which story vars / recent lines / character card to inject)
- Fallback line key (scripted text used if LLM fails, times out, or is disabled)
- Safety filters reuse Phase 39's provider-side moderation

```rust
async fn run_llm_line(node: &LlmLineNode, ctx: &NarrativeCtx) -> String {
    match ctx.llm.as_ref() {
        Some(provider) => provider
            .generate(node.prompt.render(ctx), node.budget)
            .await
            .unwrap_or_else(|_| t!(&node.fallback_key, ctx.params())),
        None => t!(&node.fallback_key, ctx.params()),
    }
}
```

Opinion: LLM lines are a last-step feature for a reason — the whole narrative stack must ship and be useful with zero ML dependency. LLM is the optional seasoning.

---

## 15. Build order

1. `rustforge-story-vars` — typed KV + save hook + subscribe.
2. Dialogue node-type definitions + `.rdialog` serde.
3. `DialogueInstance` runtime + snapshot/restore.
4. Dialogue Editor panel (Phase 20 graph + inspector + preview stub).
5. VO integration (Phase 17) and lip-sync preview (Phase 35).
6. Quest node-type definitions + `.rquest` serde.
7. `QuestInstance` runtime + narrative event bus + simulate command.
8. Quest Designer panel.
9. Default Journal UI template on Phase 15.
10. Translator workflow: export / import / lockfile / diff view.
11. LLM `LlmLine` node (last, feature-gated).

Each step ends with a narrative sample project (a village with two quests, one branching conversation, one VO'd line, one German translation round-trip) that must pass.

---

## Scope ❌

- ❌ AAA narrative design suite (scene planning, character arc analytics, Articy Draft replacement)
- ❌ Scripted cinematic conversations with camera cuts (Phase 19 Sequencer invokes dialogue assets)
- ❌ Voice-over **recording** tools or in-engine DAW (hand off to Reaper / Audition)
- ❌ AI-authored quest generation (Phase 39 speculative, not in this phase)
- ❌ Branching complexity analyzer with heatmaps and story-reachability graphs
- ❌ Natural-language authoring ("describe a quest and I'll build the graph")
- ❌ Voice cloning / TTS generation
- ❌ Translation memory server / cloud CAT integration

---

## Risks

- **Key-lock drift**: if translators work on an exported file while authors rename nodes, the lockfile + hash diff is the bulwark. Must be merge-conflict-friendly text.
- **Graph spaghetti**: large conversations visually explode. Mitigations: subgraph nodes, color-coded chapters, minimap reuse from Phase 20.
- **VO pipeline coupling**: if Phase 17 audio or Phase 35 phonemes are behind schedule, the editor still works without preview — runtime degrades to text-only gracefully.
- **BT ↔ Quest ↔ Dialogue dependency cycle**: story-vars crate must stay dependency-free of the graph crates so it can be pulled in anywhere without reaching narrative into the BT.
- **LLM determinism**: save/restore across an `LlmLine` will not re-roll the same text; we snapshot the generated string, not the prompt. Documented.
- **Event bus perf**: pathological quest counts could thrash listener re-eval. Pre-bucket by event kind, profile early.
- **Localization scope creep**: someone will want plural rules, gender agreement, right-to-left bidi — Phase 13 owns those; Phase 44 just passes them through.

---

## Exit criteria

1. A dialogue graph with line, choice, branch, set-var, and end nodes can be authored, saved as `.rdialog`, loaded at runtime, stepped to completion, and saved/restored mid-conversation.
2. A quest graph with locked → active → complete and one failure branch runs correctly against fired events, and the state is queryable from a dialogue branch.
3. Story variables persist across save/load round-trips and are readable from BT services and data-bound UI widgets.
4. Every dialogue line renders through `t!()`; exporting to XLIFF, translating in an external tool, and importing produces a playable German build with zero source-locale fallback warnings.
5. A line with an attached VO clip plays in the editor preview with a visible phoneme track and live lip-sync on a bound face rig.
6. The default Journal UI template renders active / complete / failed quests, tracks goal completion, and styles cleanly under a user theme.
7. Translator diff view identifies untranslated and stale keys with click-to-source navigation.
8. With the LLM feature disabled, every narrative system still compiles, runs, and ships a complete game — the feature is additive, never required.
9. A sample "village" project with two quests, one 30-node conversation, full VO on the English locale, and a German translation passes CI end-to-end under five minutes.
