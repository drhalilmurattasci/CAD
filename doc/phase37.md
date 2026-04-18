# Phase 37 — Multi-User Live Editing

Phase 12 looked at CRDTs and OT for real-time collaborative editing and said no. That was the right call for 1.0: a solo indie or a five-person team can move faster through Git than through a sync protocol, and the engineering cost of a live-edit stack would have strangled every other phase. Phase 37 reverses that decision because the 2.0 user base is different. At 2.0 scale — sixteen-person teams, level designers and technical artists sitting in the same scene, remote-first studios where "hop in the editor with me" replaces "jump on screenshare" — Git's commit granularity is the bottleneck, not the lubricant. This phase builds RustForge's answer to Unreal Concert / Multi-User Editing: a self-hosted session server, presence overlays, field-level sync, and a conflict model that admits last-writer-wins is usually fine and never pretends otherwise.

The guiding principle: **this is not a SaaS, not an identity system, and not a CRDT research project.** A lean relay server, a pragmatic operational model, and explicit policies for every edge case the rejected-in-Phase-12 design would have glossed over.

## Goals

By end of Phase 37:

1. A `rustforge-session` binary runs as either a sidecar to a host editor or a standing service, relaying edits to 2–8 connected editors.
2. Field-level edits propagate in <100 ms p99 on LAN, <250 ms on typical WAN, with last-writer-wins semantics and a toast on loss.
3. Hierarchy operations (reparent, spawn, despawn) use CRDT-lite identifiers to avoid duplicate-spawn and orphan-child races.
4. Presence overlay shows every connected user's selection, viewport camera, and cursor, with a stable per-user color.
5. Soft locks appear on entities under active drag or field edit and auto-release on commit or timeout.
6. Undo stays local per user; Phase 6's `CommandStack` is per-editor, never shared.
7. Voice chat from Phase 34 opts into the editor session on demand.
8. Disconnect, reconnect, and rebase work without data loss for the common case; a modal intervenes when conflicts exceed threshold.
9. One documented policy for how the session interacts with Phase 12 Git: shared working tree **or** per-user branch with auto-rebase, chosen once per session.

## 1. Framing — why now, and what we refuse to build

Phase 12 rejected CRDT for four reasons. Enumerating them and stating where 2.0 stands on each:

| Phase 12 objection                                   | 2.0 stance                                                                  |
| ---------------------------------------------------- | --------------------------------------------------------------------------- |
| "Single-user Git is enough for 1.0 teams"            | No longer true past ~6 concurrent editors on one scene                      |
| "CRDT libraries add a heavy dep and an operational burden" | Accepted. We do **not** adopt full OT/CRDT. We take a narrow subset    |
| "Server infra contradicts the self-hosted story"     | Session server is self-hosted, same model as Phase 34 lobby                 |
| "Git is the source of truth"                         | Still true. Session edits land in Git via the chosen integration policy     |

We are **not** writing a general-purpose collaborative text editor. The data model is entity-component, field-keyed, and structured. Most fields are scalars or small structs. The hard cases (strings, arrays, nested component trees) are small in count. Last-writer-wins with a visible loser notification is acceptable for scalar edits; CRDT-lite is reserved for hierarchy operations where silent divergence would corrupt the scene graph.

### 1.1 Scope boundary

The entire phase is a thin relay plus editor-side glue. The session server:

- Holds the latest known value of every synced field, keyed by `(SceneId, EntityId, ComponentTypeId, FieldPath)`.
- Forwards deltas to all other connected editors.
- Snapshots state on join and on explicit checkpoint.
- Knows nothing about Git, undo, assets, or PIE semantics.

Everything else is editor-side logic on top of the relay.

## 2. Session server — `rustforge-session`

```
crates/rustforge-session/
├── main.rs               # bind, serve, graceful shutdown
├── config.rs             # toml: bind addr, max clients, token, tls
├── transport/
│   ├── mod.rs
│   ├── ws.rs             # tokio-tungstenite, fallback path
│   └── quic.rs           # quinn, reuse Phase 14 cert plumbing
├── session.rs            # SessionState: field map, presence, locks
├── protocol.rs           # wire messages (see §4)
├── snapshot.rs           # full-state on join, checkpoint on demand
└── audit.rs              # append-only log per session, for postmortem
```

Binary runs in two modes, chosen by config:

- **Host-embedded.** One editor also runs the server in-process on a background tokio runtime. Zero extra deploys. The host's editor is just another client over localhost. Session dies when the host exits.
- **Standing.** Separate process, same binary, no editor attached. Survives any editor restart. Teams with a shared dev box run one of these.

Opinion: **default to host-embedded for small teams, document standing deployment for >4 users.** The standing server is half a page of systemd. No container image in-tree; that is a distribution concern, not a phase concern.

## 3. Session diagram

```
                           ┌────────────────────────────┐
                           │  rustforge-session (relay) │
                           │  ──────────────────────────│
                           │  field map  ◄── LWW        │
                           │  hier ops   ◄── CRDT-lite  │
                           │  presence   ◄── heartbeat  │
                           │  locks      ◄── lease+TTL  │
                           │  snapshot   ◄── on join    │
                           └──┬─────────┬─────────┬─────┘
                              │ QUIC/WS │ QUIC/WS │
              ┌───────────────┘         │         └───────────────┐
              │                         │                         │
    ┌─────────▼─────────┐     ┌─────────▼─────────┐     ┌─────────▼─────────┐
    │  Editor (Alice)   │     │  Editor (Bob)     │     │  Editor (Carol)   │
    │  ─────────────────│     │  ─────────────────│     │  ─────────────────│
    │  local CommandStk │     │  local CommandStk │     │  local CommandStk │
    │  outbound deltas  │     │  outbound deltas  │     │  outbound deltas  │
    │  inbound apply    │     │  inbound apply    │     │  inbound apply    │
    │  presence paint   │     │  presence paint   │     │  presence paint   │
    │  lock overlay     │     │  lock overlay     │     │  lock overlay     │
    └───────────────────┘     └───────────────────┘     └───────────────────┘
```

No editor talks to any other editor. The relay is the only authority on ordering.

## 4. Wire protocol

Binary `bincode` over a framed transport. Text debug dump is behind a `--log-protocol` flag on the server; never on by default.

```rust
#[derive(Serialize, Deserialize)]
pub enum ClientMsg {
    Hello { token: String, client: ClientInfo },
    FieldEdit { path: FieldPath, value: Bytes, lamport: u64 },
    HierOp(HierOp),
    LockAcquire { entity: EntityId, kind: LockKind },
    LockRelease { entity: EntityId },
    Presence(PresenceUpdate),
    Checkpoint,          // host-only, forces full snapshot broadcast
    Bye,
}

#[derive(Serialize, Deserialize)]
pub enum ServerMsg {
    Welcome { session_id: Uuid, you: ClientId, peers: Vec<Peer>, snapshot: Snapshot },
    PeerJoin(Peer),
    PeerLeave(ClientId),
    FieldApply { path: FieldPath, value: Bytes, from: ClientId, lamport: u64 },
    FieldReject { path: FieldPath, reason: RejectReason, your_lamport: u64 },
    HierApply(HierOp),
    LockGranted { entity: EntityId, holder: ClientId, ttl_ms: u32 },
    LockDenied { entity: EntityId, current_holder: ClientId },
    LockReleased { entity: EntityId },
    Presence { peer: ClientId, update: PresenceUpdate },
    Error(ProtocolError),
}

pub struct FieldPath {
    pub scene: SceneId,
    pub entity: EntityId,    // stable, CRDT-lite assigned (see §6)
    pub component: ComponentTypeId,
    pub field: u32,           // interned field index, resolved via Phase 3 reflection
}
```

Lamport clocks, not wall clocks. Every edit carries the sender's Lamport tick; the server tracks the highest seen per field and only applies/forwards strictly-greater ticks. Ties break on `ClientId`. This is "LWW with a real clock", not wall-clock drift roulette.

## 5. Transport — QUIC first, WebSocket fallback

Reuse Phase 14's `quinn` stack. Same ALPN registration, same self-signed-dev / studio-CA cert handling. WebSocket via `tokio-tungstenite` is the compatibility path for corporate NATs that block UDP.

Opinion: **QUIC is the default.** Head-of-line blocking on WebSocket during a large snapshot is noticeable; QUIC's per-stream semantics let presence heartbeats interleave cleanly with a bulk snapshot transfer.

Ports: default `17720` UDP and TCP. Configurable. `rustforge-session --discover` advertises over the same mDNS mechanism as Phase 34 lobby discovery for LAN sessions.

## 6. Hierarchy ops — CRDT-lite

Naïve LWW breaks the scene graph. Two users simultaneously spawning a child under the same parent with LWW produces one child, not two. Two users reparenting the same entity produces a flip. The fix is small and local to hierarchy ops only:

- **Entity IDs are opaque ULIDs** minted client-side, not server-assigned sequence numbers. Simultaneous spawns never collide on ID.
- **Reparent is a CAS.** `HierOp::Reparent { entity, expected_parent, new_parent }`. Server rejects if `expected_parent` mismatches the relay's current view. Loser gets a toast and chooses whether to retry.
- **Despawn wins over edit.** If user A deletes entity E while user B edits a field on E, B's inbound edit is dropped and B sees a modal: "Alice deleted this entity while you were editing it. Your local edit is preserved in the clipboard." The entity goes to a recoverable graveyard for 60 seconds.

This is not real CRDT. It's three carefully chosen operations with carefully chosen semantics. Everything else is LWW.

## 7. Field sync — delta encoding

Phase 3's reflection already produces `ron`-compatible values. We do not send full `ron::Value` over the wire; instead:

```rust
pub enum FieldDelta {
    Whole(Bytes),                 // serialized value via Phase 4 binary codec
    VecElem { index: u32, value: Bytes },
    VecInsert { index: u32, value: Bytes },
    VecRemove { index: u32 },
    MapSet { key: Bytes, value: Bytes },
    MapRemove { key: Bytes },
    StrSplice { start: u32, end: u32, replacement: String }, // rare
}
```

For transforms (the common case — 90%+ of edit traffic during gizmo drag) `Whole` is smaller than any diff would be. Vec/map deltas exist only because a 10k-entry `Vec<WeaponStat>` shipping as whole-value every edit would blow the 5 KB/s budget.

Outbound deltas coalesce in a 16 ms window before send (one tick at 60 Hz). Inbound deltas apply immediately but batch into a single ECS mutation per frame so the renderer sees a consistent state.

## 8. Snapshots and bandwidth

On join: server sends `Snapshot` containing every live field in the session. Target: **<1 MB initial sync for a 10k-entity scene.** Achieved via:

- Component-type-keyed RLE for default values (don't ship what matches `Default::default()`).
- `zstd -3` over the frame.
- Skip fields tagged `#[reflect(no_sync)]` (local view state, editor-only gizmo flags).

Steady state: 5 KB/s per user average, 40 KB/s burst during active gizmo drag. Monitored via a `session_bytes_sent` metric surfaced in the editor status bar during debug builds.

## 9. Presence overlay

Each peer carries:

```rust
pub struct PresenceUpdate {
    pub selection: Vec<EntityId>,
    pub viewport_camera: Option<(Vec3, Quat)>,
    pub cursor_ray: Option<(Vec3, Vec3)>,   // origin, dir; None when mouse is in a panel
    pub active_tool: ToolKind,
}
```

Heartbeat at 20 Hz when active, 2 Hz when idle. Render:

- **Selection outline** in peer's color, slightly thinner than the local selection outline.
- **Viewport frustum ghost** drawn at 30% opacity when hovering a peer's avatar in the presence panel.
- **Floating avatar billboard** at the peer's viewport camera position in the 3D scene, labeled with peer name.
- **Name tag on hovered entity** when the peer's cursor ray intersects it.

Colors: fixed palette of 8 distinguishable hues (deuteranopia-safe), assigned on join in FCFS. Avatars fade to gray 5 s after last heartbeat, vanish after 30 s.

## 10. Lock-on-edit

Soft locks, not hard locks. The goal is communication, not enforcement.

```rust
pub enum LockKind {
    GizmoDrag,          // acquired on drag start, released on drag end
    FieldEdit,          // acquired on inspector focus, released on blur
    ComponentStructural, // acquired on add/remove component
}
```

Flow:

1. Alice begins a transform drag. Editor sends `LockAcquire { entity, GizmoDrag }`.
2. Server grants with TTL 3000 ms, broadcasts `LockGranted`.
3. Other editors render a badge on the entity: "Alice is editing" in her color.
4. Alice's drag continues; editor renews the lease every 1000 ms.
5. On drag end, `LockRelease`. Server broadcasts `LockReleased`.

If Bob tries to edit the same entity while Alice holds the lock:

- Bob's inspector shows the badge and disables the affected field (dimmed, not hidden).
- Bob can still read, copy, and view history. He cannot commit an edit until the lock clears.
- If Alice disconnects mid-drag, TTL expires, lock clears automatically.

Hard enforcement was rejected. The lock is a hint layered on LWW, not a mutex. If Bob's editor has a stale view of the lock state and sends an edit, LWW still applies.

## 11. Undo — local, not global

Global undo was proposed and rejected. Reasoning, to commit to the doc:

- **User agency.** If Alice can undo Bob's work without consent, the tool is hostile. If Alice's undo reverts across Bob's subsequent dependent edits, the history is incoherent.
- **Semantic drift.** Bob's "move the cube" and Alice's "rename the cube" happened on the same entity; undoing Bob's move in isolation is well-defined, but weaving both into one linear stack requires operational transform, which we refused in §1.
- **Phase 6 compatibility.** The existing `CommandStack` is per-editor. Commands synthesize wire edits as a side effect of execute; undo replays the inverse, which produces new wire edits. Bob sees "Alice undid her move" as a normal edit, not as a stack operation.

Ctrl+Z undoes only commands issued from the current editor. Foreign edits are invisible to the local undo stack. This is exactly how Unreal Concert handles it. Shipped with a one-line tooltip on the History panel: "Undo affects only your edits. Peers' edits are part of the shared scene."

## 12. Conflict resolution

Field-level:

- LWW applies. Loser gets a toast: "Your change to `Transform.translation` on `Turret_07` was overwritten by Alice." Toast includes an inline "Re-apply my value" button that issues a fresh edit with a newer Lamport tick (i.e. the user explicitly decides to fight).
- Toast auto-dismiss after 6 s, with the last 20 accumulated in a Session Log panel for postmortem.

Critical conflicts:

- **Delete-vs-edit** (covered in §6) → modal.
- **Component-removed-while-editing** → modal, same pattern: preserved to clipboard.
- **Reparent CAS failure** → inline toast with "Retry" button; the local operation is rolled back locally first.

No three-way merge UI. That's Phase 12's job, and it runs at commit time, not live. Live conflicts are resolved the moment they happen.

## 13. Voice chat integration

Phase 34 shipped a WebRTC-backed voice stack for in-game VOIP. Phase 37 reuses it verbatim:

- Session server carries SDP/ICE signaling as opaque frames (`ClientMsg::Voice(Bytes)` wrapping Phase 34 messages).
- Editor exposes a mic toggle in the presence panel. Opt-in per session — joining a session does **not** enable mic.
- Spatial audio is off; editor voice is 2D. No reason for Alice's voice to pan left because her avatar is on the west side of the level.
- No recording. No transcription in-phase; if a team wants that, they self-host a bot against the signaling channel.

## 14. Asset edits

Phase 5's reimport pipeline is the single authority on asset state. When Alice reimports `sword.png`:

1. Her file watcher fires locally → AssetRegistry reimports → bumps asset version.
2. Editor emits a `ClientMsg::AssetTouch { guid, version }`.
3. Server forwards. Bob's editor receives, notes the version bump, invalidates its cached asset.
4. Bob's file watcher also fires because the file changed on his disk — but only if the working tree is shared (see §15). If not, the message is the sole signal.

Asset binary contents do **not** flow over the session. Assets live in Git or the shared working tree. The session propagates only the fact of a change. A scenario where Alice and Bob have divergent working trees is a configuration error the session surfaces but does not paper over.

## 15. Git integration — one policy per session

Choose at session creation, locked for the life of the session:

### Policy A: Shared working tree

Every editor points at the same checkout on a shared filesystem (SMB, NFS, a dev VM). Asset changes propagate via filesystem, not the session. Simplest, fragile on flaky networks, incompatible with Windows case-insensitive locking if Alice and Bob are on different OSes.

### Policy B: Per-user branch with auto-rebase

Each editor works on its own branch (`session/<user>/<session_id>`). Commits happen silently on a 2-minute timer or on explicit checkpoint. Session server coordinates a lightweight rebase: at checkpoint, all branches fast-forward onto a `session/head`. Phase 12's structural merge driver handles scene conflicts; other conflicts fall back to last-writer-wins within the session (the merge will always resolve because the session already agreed on values in memory).

Opinion: **Policy B is the recommended default.** Policy A exists only for teams that already work on a shared VM. Policy B composes with Phase 12's conflict UI and leaves a real Git history instead of a squashed "session" commit.

## 16. Play mode

PIE (Phase 7) is expensive and state-mutating. Two behaviors, selectable per session:

- **Exclusive PIE.** One user enters PIE. Other users' editor becomes view-only for the duration: they see the PIE state streamed at 10 Hz (low-fi, camera-position + selection only, no gameplay sim). They cannot edit. Exit PIE → normal editing resumes. Simplest, matches Unreal Concert default.
- **Per-user PIE.** Each user independently launches PIE from the current shared scene. Each PIE is isolated; edits made during PIE do not sync to peers; exiting PIE reverts to the pre-PIE shared state. More expensive, less surprising.

Default: **Exclusive PIE.** It is what level designers expect and what keeps CPU cost bounded.

## 17. Network partition handling

Disconnect:

- Local edits queue in an outbound buffer.
- Editor shows a "Disconnected — reconnecting" banner.
- Undo/redo still works locally; no PIE state changes.
- Lock-on-edit is visually replaced with "connection lost" badge on all entities.

Reconnect:

- Client re-handshakes, sends its outbound queue tagged with original Lamport ticks.
- Server applies using normal LWW rules. Some edits lose because peers wrote newer values.
- Loser toasts accumulate in a single "Reconciliation summary" modal when the reconciled count exceeds 5 edits. User can review and selectively re-apply.
- If the server restarted and lost state, client is told via `ServerMsg::Welcome` with a new `session_id`. Local editor saves a rescue `.ron` dump beside the project and prompts the user to manually reconcile.

Partition longer than 10 minutes auto-disconnects and requires a fresh join.

## 18. Security

Same trust model as Phase 29 DDC:

- **Session token** in `ClientMsg::Hello`. HMAC-SHA256 over session id, generated when session is created, shared out-of-band (copied URL, Slack). Tokens rotate on session restart.
- **TLS** mandatory for non-loopback. Phase 14 certificate plumbing applies.
- **No identity.** The session has no concept of "user accounts". Peers self-report a display name, which is purely cosmetic. Teams needing real auth run `rustforge-session` behind a reverse proxy with SSO-terminating middleware (Tailscale, Cloudflare Access, Authelia). Documented in the deployment page, not built in.
- **Audit log.** Server writes an append-only JSONL to `sessions/<id>.log` for postmortem. Rotates at 100 MB.

Explicitly not a threat model for hostile peers inside a session. A peer with the token is trusted. Stolen-token mitigation is the reverse proxy's job.

## 19. Build order

1. Wire protocol + bincode codec + stub server that echoes hello.
2. QUIC transport (reuse Phase 14), WebSocket fallback.
3. Presence overlay — lowest risk to ship, highest visible payoff, exercises roundtrip.
4. Field-sync for scalar fields only, LWW, Lamport clocks, toast on loss.
5. Hierarchy ops (CRDT-lite ULID entities, CAS reparent, despawn-vs-edit modal).
6. Lock-on-edit badges and lease protocol.
7. Conflict resolution UI (toast, session log panel, modals).
8. Voice integration (Phase 34 reuse).
9. PIE policy (exclusive, then per-user behind a flag).
10. Git integration — Policy A first (trivial), then Policy B with auto-rebase.
11. Reconnect/rebase flow and rescue-dump path.
12. Snapshot compression and bandwidth budgeting.
13. Standing-server deployment docs and mDNS discovery.

## Scope ❌

- MMO-scale editing (>50 users in one session). Server state becomes quadratic in presence updates; out of scope.
- Offline merge of two concurrent multi-hour sessions against each other. Git with Phase 12 merge driver handles this at commit time, not live.
- Visual diff between user edits in real time (a la Figma "compare changes"). Future phase.
- Full OT implementation from scratch. Explicitly rejected in §1 and §11.
- Hosted SaaS / billing / tenancy. Self-hosted only.
- Voice spatial audio in editor.
- Identity and SSO built into the session server.
- Shared undo stack.
- Hard locks / pessimistic locking.
- Streaming PIE simulation state at full fidelity to viewers.

## Risks

- **Lamport clock drift under partition.** A long-disconnected client reconnects with a Lamport tick wildly behind the server's. Mitigation: on reconnect, client adopts `max(local_lamport, server_lamport) + 1` before draining its outbound queue. Its queued edits are all "behind" and mostly lose; that is acceptable and matches user expectation after a long disconnect.
- **CRDT-lite hierarchy is a sharp knife.** Three operations, but subtle. Reparent CAS must account for the target parent itself having been reparented or deleted in the interim. Unit tests must cover the full matrix.
- **Soft locks are racy.** Bob's editor may show "Alice is editing" 50 ms after Alice's drag ended. LWW catches the resulting conflict, but the UX is jarring. Mitigation: render locks with a 200 ms fade so quick acquires don't flicker.
- **Policy B auto-rebase conflicts with Phase 12 hooks.** The structural merge driver was built assuming interactive use. Running it on a 2-minute timer under session control may trigger hook loops. Mitigation: session-driven merges pass `GIT_EDITOR=true` and a skip-hooks env var; hooks the user set up for manual commits do not fire during session commits.
- **Voice opt-in defaults.** Accidentally hot mic when joining a session is a real risk. Mitigation: mic is hard-disabled until the user clicks the icon. No auto-join of voice on session join.
- **Standing server becomes a SPOF.** If the session server crashes, all editors disconnect. Mitigation: audit log replay on server restart rebuilds state from the last snapshot + edits; documented recovery procedure.
- **Bandwidth blowout on large vec edits.** Editing an element of a 10k-entry `Vec` currently ships `VecElem`, but if the Vec itself is replaced the whole blob ships. A runaway script that mutates a large Vec per frame can saturate the 40 KB/s burst budget. Mitigation: editor-side rate limit on per-field edit frequency (max 60 Hz), warn in status bar on sustained overrun.

## Exit criteria

- 8 concurrent editors in one session maintain <100 ms p99 edit latency on LAN, <250 ms on WAN with 40 ms RTT.
- Initial join completes in <2 seconds for a 10k-entity scene; snapshot transfer <1 MB compressed.
- Steady-state bandwidth per user <5 KB/s over a 10-minute recorded session with typical level-design edits.
- Kill-server-mid-session test: all editors disconnect, rescue dump is written, reconnect after server restart reconciles without data loss beyond the expected LWW losses.
- Simultaneous reparent on the same entity from two editors: both editors converge to the same final parent, loser sees toast, no orphan children.
- Simultaneous delete + edit on the same entity: editor holding the edit sees the delete modal, clipboard recovery works.
- Lock-on-edit badge appears within 200 ms of drag start on a peer editor; clears within 500 ms of drag end.
- Voice chat opt-in works without mic being live before the user clicks.
- Policy B session: two users edit disjoint entities for 30 minutes, auto-rebased commits land on `session/head`, Git log is linear and reviewable.
- Policy B session: two users edit the same entity, commits round-trip through Phase 12 structural merge without human intervention for LWW-compatible fields.
- Undo on editor A does not produce undo on editor B; editor B sees the effect as a normal edit in its scene.
- mDNS discovery locates a LAN-running session server within 5 seconds of editor startup on a 24-host subnet.
