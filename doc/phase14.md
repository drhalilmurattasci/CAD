# Phase 14 — Networking & Replication

Thirteen phases shipped a 1.0 editor: an authoring tool with a tested tick loop, a reflection registry, deterministic Play-in-Editor, a hot-reloadable script host, and a plugin surface that doesn't break between releases. None of that assumed a second machine existed. Phase 14 is the first post-1.0 phase, and it's the one users asked for loudest: multiplayer. Not "two windows both running the scene" — *authoritative networking*, where a server owns the truth and clients see a shaped view of it.

This phase does not make RustForge a competitor to dedicated netcode middleware. It does make RustForge a plausible choice for the games the engine already fits — cooperative 2–16 player action games, tick-based strategy, small-session shooters. The deliverable is a replication layer that reuses the reflection registry, a QUIC transport, client-side prediction with server reconciliation, and a PIE mode that simulates two endpoints in one process so authors can debug networked gameplay without a LAN. Anti-cheat, matchmaking, lobbies, and dedicated-server orchestration are explicitly out of scope and noted in §13.

## Goals

By end of Phase 14:

1. A **client-server authoritative** architecture, with the server as the single source of truth and clients as predictive views.
2. A `#[replicate]` attribute that hooks into the Phase 2 reflection registry and marks component fields for network sync.
3. A wire identity scheme combining baked `SceneId` from Phase 4 with a new `NetId(u64)` for runtime-spawned entities.
4. Three **RPC channels** — reliable-ordered, reliable-unordered, unreliable — usable from both native Rust gameplay code and WASM scripts.
5. **Client-side prediction** for locally-owned entities and **server reconciliation** with rollback-and-replay when the server disagrees.
6. **Interpolation** for non-owned entities and **extrapolation** as a clamped fallback for dropped packets.
7. **QUIC transport** via `quinn`, with pluggable alternatives behind a trait.
8. **Bandwidth budget** per-connection with a priority heap to pick what to send when the pipe is full.
9. **PIE networking** — two `World`s in the editor, a virtual in-process transport, and the ability to drive either from the inspector.
10. An exit criteria checklist that can be run by one author on one machine without dedicated server hardware.

## 1. Architectural choice — authoritative client-server

Three topologies were considered. Only one survives.

### Rejected: peer-to-peer with shared authority

Every peer owns a slice of the world. Trivial to prototype. Impossible to ship. Shared authority means every peer is trusted, every peer can cheat, and conflict resolution between simultaneous writes to the same entity is an unbounded research problem. RustForge is a general-purpose engine — if we bake P2P in, users will ship competitive games on it, and we will be the root cause when those games are unplayable. Not doing it.

### Rejected: deterministic lockstep

Every peer runs the same simulation from the same inputs, only inputs cross the wire. This is how Age of Empires 2 and every fighting game works. It's also wildly incompatible with the RustForge tech stack. Lockstep requires bit-exact determinism across every system — Rapier physics (non-deterministic under parallelism), WASM scripts with floating-point (varies across hosts), even iteration order over `hecs` component storages. Making any one of those deterministic is a multi-month project. Making all of them deterministic forever is a permanent tax on every future engine change. For an RTS-focused engine this would be worth it; for a general engine it is not.

### Chosen: authoritative client-server

One endpoint is the server. It ticks the full simulation. Clients send inputs upstream, receive state deltas downstream, and render an interpolated view. Trust flows one way: the server can reject anything a client says.

This is boring. Boring is correct. It composes with Phase 7's PIE — a PIE session can host one server `World` and one or more client `World`s in the same process. It composes with Phase 2's tick split — `tick_play` is just the server's tick, while clients get a new `tick_net_client` variant. And it matches what every shipped multiplayer game in this engine's target niche actually does.

## 2. Module layout

New crate, because networking compiles slowly and we don't want game builds to pay for it when single-player.

```
crates/
├── rustforge-core/
├── rustforge-net/               # NEW — transport-agnostic replication
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs               # NetPlugin, feature gates
│       ├── identity.rs          # NetId, EntityMap, scene-id bridge
│       ├── registry.rs          # replicate(), #[replicate] bookkeeping
│       ├── snapshot/
│       │   ├── mod.rs           # Snapshot, SnapshotDiff
│       │   ├── encode.rs        # wire format (bitpack + varint)
│       │   ├── decode.rs
│       │   └── delta.rs         # baseline+delta encoding
│       ├── rpc/
│       │   ├── mod.rs           # Rpc trait, channel kinds
│       │   ├── reliable.rs
│       │   └── unreliable.rs
│       ├── prediction/
│       │   ├── mod.rs           # ClientPrediction resource
│       │   ├── rollback.rs      # replay-from-ack
│       │   └── interp.rs        # InterpBuffer for remote entities
│       ├── transport/
│       │   ├── mod.rs           # Transport trait
│       │   ├── quic.rs          # quinn-backed impl
│       │   └── virt.rs          # in-process loopback for PIE
│       ├── bandwidth.rs         # BudgetedSender, priority heap
│       └── diag.rs              # replication stats, plumbs to Phase 8 profiler
└── rustforge-editor/
    └── src/
        └── net_pie/             # NEW — dual-world PIE integration
            ├── mod.rs
            ├── topology.rs
            └── inspector.rs     # switch viewport between server/client world
```

The `rustforge-net` crate depends on `rustforge-core` with the `editor` feature **off**. The editor's `net_pie` module depends on both.

## 3. Entity identity on the wire

Phase 4 gave us `SceneId(u64)` — stable, assigned at save time, identical across every load. That solves half the problem: every entity *baked into a scene file* already has a cross-process name.

Runtime-spawned entities don't. A bullet spawned by a player script on frame 12 has no `SceneId`. For those, introduce:

```rust
#[derive(Reflect, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NetId(pub u64);
```

Allocation rules:

- `NetId` is **server-assigned only**. Clients that want to spawn a replicated entity ask the server; it returns an id. Never allocate a `NetId` client-side and hope — two clients spawning at the same tick would collide.
- For baked entities, `NetId` is derived deterministically from `SceneId` (`NetId(scene_id.0)`). This means the server never needs to tell a client "this entity with `SceneId(42)` is now `NetId(17)`" — the client already knows.
- For runtime spawns, `NetId` comes from a server-local `AtomicU64` seeded above the max `SceneId` space (reserve the top bit: `NetId(0x8000_0000_0000_0000 | counter)`). Clean separation, no range checks.

Every client maintains an `EntityMap`:

```rust
pub struct EntityMap {
    net_to_local:  FxHashMap<NetId, Entity>,
    local_to_net:  FxHashMap<Entity, NetId>,
}
```

Populated on spawn packets, drained on despawn packets. All `EntityRef` fields in replicated components serialize as `NetId` on the wire; the decoder rewrites them to local `Entity` before the component reaches the `World`. This is the same pattern Phase 4 used for scene save/load — the wire is just another serialization target.

## 4. The `#[replicate]` attribute

Replication hooks into the same reflection registry Phase 2 built for the inspector. No second registry.

```rust
#[derive(Reflect, Replicate)]
pub struct Health {
    #[replicate(channel = "reliable-ordered", priority = 10)]
    pub current: f32,

    #[replicate(channel = "reliable-ordered", priority = 10)]
    pub max: f32,

    #[replicate(skip)]
    pub last_damage_source: Option<EntityRef>,  // gameplay detail, server-only
}

#[derive(Reflect, Replicate)]
#[replicate(condition = "owner_only")]
pub struct PlayerInput {
    pub move_dir: Vec2,
    pub look:     Quat,
    pub fire:     bool,
}
```

What the derive emits:

1. A `ReplicateDescriptor` registered at startup alongside the existing `ReflectDescriptor`. It lists replicated fields, channels, priorities, and conditions.
2. `encode(&self, cursor, baseline: Option<&Self>) -> Bits` — bitpacks changed fields against the last acked value. Unchanged fields cost one bit.
3. `decode(cursor, into: &mut Self)` — reverse.
4. A predicate function for the `condition` attribute. Supported conditions:
   - `always` (default)
   - `owner_only` — only sent to the client that owns this entity
   - `visible_only` — filtered by the server's per-client AOI (area-of-interest, §7)
   - `custom = "path::to::fn"` — user-provided `fn(server: &World, client: ClientId, entity: Entity) -> bool`

Channels are named strings resolved at startup to `ChannelId(u8)`. Priority is an `i16` used by the bandwidth scheduler in §9.

Two rules that the derive must enforce:

- **No replication of non-reflected types.** If a field's type has no `Reflect` impl, compilation fails at the field, not at send time.
- **Changing a `#[replicate]` attribute is a protocol break.** A compile-time hash of the schema goes into the handshake; mismatched clients disconnect with a clear error. No silent wire drift.

## 5. RPCs

Three channels, mirroring what shipped engines have converged on:

| Channel              | Ordered | Reliable | Use case                              |
|----------------------|---------|----------|---------------------------------------|
| `reliable-ordered`   | yes     | yes      | "start match", chat, scripted events  |
| `reliable-unordered` | no      | yes      | score updates, pickup events          |
| `unreliable`         | no      | no       | inputs, voice frames, cosmetic events |

The `Rpc` trait:

```rust
pub trait Rpc: Reflect + Send + Sync + 'static {
    const CHANNEL: Channel;
    const DIRECTION: RpcDirection;   // ClientToServer | ServerToClient | Broadcast
}

#[derive(Reflect, Rpc)]
#[rpc(channel = "unreliable", direction = "ClientToServer")]
pub struct FireWeapon { pub target: Vec3, pub tick: u32 }
```

Call sites look the same in Rust and in WASM scripts (the binding layer from Phase 11 is extended with an `rpc::send` host function):

```rust
net.send_to_server(FireWeapon { target, tick: current_tick() });
```

Reliability is handled inside the transport (§8). Ordered delivery is enforced by sequence numbers per channel, buffered on receive.

Two non-negotiable rules for script RPCs:

- **No arbitrary server execution from client RPCs.** An RPC is a data message; the server-side handler is registered native code. A client cannot cause the server to run a script function just by sending an RPC.
- **Rate-limit every client→server RPC.** Per-RPC token bucket with defaults in `NetConfig`; scripts can tighten but not loosen. Unthrottled client RPCs are a denial-of-service vector and we will not pretend otherwise.

## 6. Snapshots, baselines, delta encoding

Replication is snapshot-based, not event-based. The server builds a per-tick snapshot of the visible world for each client and sends the delta from the last value that client acked.

```
Server tick N:
  for each client C:
     visible = AOI(C, world)
     snapshot_N = encode(visible, baseline = last_acked(C))
     transport.send_unreliable(C, snapshot_N, ack_tag = N)

Client receives snapshot_N with tag N:
     apply(snapshot_N) → local world
     send ack(N) next frame piggybacked on input packet
```

- **Baselines are per-client.** The server remembers the last tick each client acked and diffs against *that* tick. A client with 300ms ping costs more bandwidth than one with 30ms; this is fine and expected.
- **Acks are piggybacked on inputs.** Clients send input every tick anyway. No separate ack packets.
- **Spawn and despawn are special.** They live in a reliable side-channel; a snapshot cannot contain a component for an entity whose spawn hasn't arrived yet.

Wire format is boring on purpose:

```
[u16 proto_version][u32 tick][u16 entity_count]
  repeat entity_count:
    [VarInt NetId]
    [u8  change_mask]   # one bit per replicated component type present
    for each set bit:
      [component bits, bitpacked per #[replicate] descriptor]
```

No framing cleverness, no compression beyond bit-packing. If measured bandwidth turns out to be a problem, add snap-compressed delta streams in a later phase — do not invent it now.

## 7. Area-of-interest

Not every client needs to see every entity. An AOI filter runs server-side before snapshot encoding:

```rust
pub trait AreaOfInterest {
    fn visible(&self, world: &World, client: ClientId) -> SmallVec<[Entity; 64]>;
}
```

Ships with two implementations:

- `FullWorldAoi` — everyone sees everything. Correct default for small co-op.
- `RadiusAoi { radius: f32 }` — entities within N meters of the client's owned entity. Fine for arena shooters. Ignores occlusion deliberately — occlusion-aware AOI is a PVS problem and belongs in the game, not the engine.

Games that need cell-grids, portals, or streaming-based AOI write their own `impl AreaOfInterest`. The trait stays narrow.

## 8. Transport — QUIC via `quinn`

```
  ┌─────────────────────────────────────────┐
  │         rustforge-net::Transport        │   trait
  └───────────────┬─────────────────────────┘
                  │
        ┌─────────┴─────────┐
        │                   │
   ┌────▼─────┐        ┌────▼──────┐
   │ quic.rs  │        │ virt.rs   │   in-process for PIE
   │ (quinn)  │        │ (mpsc)    │
   └──────────┘        └───────────┘
```

Why QUIC:

- Multiplexed streams per channel — reliable-ordered, reliable-unordered, unreliable mapped onto QUIC streams and datagrams respectively. No head-of-line blocking across channels.
- TLS 1.3 built in — not as anti-cheat, just as "don't ship a multiplayer engine that puts inputs on the wire in plaintext in 2026."
- 0-RTT reconnect — reconnect on network drop is ~0ms instead of the TCP handshake tax.
- Runs over UDP — works through NATs that break raw UDP games and works on every OS we target.

`quinn` is the default. It is *not* the only option — the `Transport` trait is thin enough that a WebTransport impl, a WebSocket fallback for WASM-in-browser clients, or a UDP-direct implementation can be added without touching replication code.

The virtual transport (`virt.rs`) is a pair of `std::sync::mpsc` channels with an artificial latency+loss model. It is what makes PIE networking work (§10) and what the test suite uses.

## 9. Bandwidth budget and priority heap

A `BudgetedSender` per client holds a per-tick byte budget (default 64 KB/tick at 60Hz = ~30 Mbit/s cap; adjustable per connection). Every replicated component update and every RPC enters a max-heap keyed by `(priority, staleness)`. The scheduler pops items and encodes them into the outgoing snapshot until the budget is exhausted; leftovers carry over to the next tick with bumped staleness, so nothing starves forever.

```rust
pub struct BudgetedSender {
    budget_bytes_per_tick: u32,
    heap:   BinaryHeap<PendingUpdate>,   // (priority, staleness)
    stats:  BandwidthStats,              // wired to diag.rs and Phase 8 profiler
}
```

Rules:

- **Priority is the `#[replicate(priority = …)]` integer.** Higher wins. No runtime priority mutation APIs in 1.0 — games tune via the attribute.
- **Staleness is the number of ticks an update has been deferred.** Added to priority as a soft-floor to prevent infinite starvation of low-priority updates (cosmetic state).
- **Spawn/despawn bypass the budget.** They're reliable-ordered, and starving them for bandwidth deadlocks the client view.
- **Measured per-client, not global.** Two connected clients pay independent budgets; there is no shared server uplink budget in the engine. Dedicated-server hosting can enforce that externally.

## 10. PIE networking — two worlds in one process

The Phase 7 PIE snapshot invariant must survive: **Stop returns the scene byte-identical to pre-play**. Networking doesn't change that — it changes what happens *in between*.

Topology options exposed in the Play dropdown:

```
┌─ Play Mode ───────────────────────┐
│  ○ Single-player                  │
│  ● Listen server + 1 client       │
│  ○ Dedicated server + N clients   │  (N = 1..4 in PIE)
└───────────────────────────────────┘
```

"Listen server + 1 client" is the default for authors debugging multiplayer. The editor constructs:

```
                    ┌────────────────┐
                    │  Server World  │  ← snapshot source-of-truth
                    └───────┬────────┘
                            │  virt-transport (mpsc + latency sim)
             ┌──────────────┴──────────────┐
             │                             │
     ┌───────▼──────┐              ┌───────▼──────┐
     │ Client World │              │ Client World │
     │   (local)    │              │   (remote)   │
     └──────────────┘              └──────────────┘
```

All three `World`s are in the editor process. All three render (client worlds render into separate offscreen textures; the viewport has a dropdown to pick which one to show). All three are inspectable — the hierarchy and inspector gain a "World: Server | Client 0 | Client 1" selector.

On Stop: *every* world is discarded and the pre-play snapshot from Phase 7 restores the single editor `World`. Networking does not complicate the PIE invariant because the PIE invariant only talks about the *editor* world, and the editor world is the one Phase 7 already snapshots.

The latency simulator on the virt-transport defaults to 60ms RTT + 1% loss — bad enough that prediction bugs manifest in PIE, not in production.

## 11. Client-side prediction and reconciliation

Without prediction, a shooter at 60ms RTT has 60ms of input lag on the player's own character. Unplayable. With prediction, it's zero. This is table stakes.

The loop, for every entity the client owns:

```
Client tick T:
  1. Capture input.
  2. Send input to server with sequence number S_T.
  3. Apply input to local owned-entity using the SAME system the server runs.
  4. Store (S_T, post-input state) in a rolling history (default 2 seconds).
  5. Render.

Server responds with snapshot @ tick U, containing the state after input S_U was processed.

Client receives snapshot:
  - If snapshot.owned == history[S_U], server agrees → drop history entries ≤ S_U.
  - Else: server disagrees →
      a. Replace owned state with server state.
      b. Re-apply every input from S_U+1 to S_current.
      c. That becomes the new current state.
```

Rules:

- **Prediction code and server code must be the same function.** Not "similar." Same. The engine enforces this by routing input-driven systems through a `PredictedSystem` registration that both server and client use. Drift between client-predicted and server-applied code is the single most common source of rubber-banding in shipped games.
- **Only owned entities predict.** Everything else interpolates (§12).
- **History rolls by ticks, not by wall-clock.** Reconnects resync to server tick; clients never trust their local clock for authority.
- **Reconcile even on agreement.** Drop the history prefix; don't skip the path because it's harder to test when it only runs on disagreement.

## 12. Interpolation and extrapolation

Non-owned entities (enemies, other players, projectiles) arrive as snapshots at ~60Hz with jitter. Displaying them raw looks like bad stop-motion. Fix: interpolate between the last two received states, rendering ~100ms in the past.

```rust
pub struct InterpBuffer {
    samples: ArrayDeque<Snapshot, 4>,   // newest ~200ms
    render_delay: Duration,             // default 100ms
}
```

Rendering at tick T draws state at wall-time `now - render_delay`, linearly interpolating between the two bracketing snapshots.

When packets drop and no bracketing snapshot is available:

- **Extrapolate up to 2 ticks** using the last known velocity. This covers typical jitter.
- **Freeze beyond 2 ticks.** Extrapolating further produces worse artifacts than freezing (enemies teleporting through walls, projectiles curving into geometry).
- **Log the extrapolation event** to the netcode diagnostic panel. A game that extrapolates 5% of the time has a real problem the author needs to see.

The 100ms default is aggressive — it trades latency for smoothness. Games that need tighter (competitive shooters) lower it to 50ms; games that tolerate looser (co-op adventure) raise it to 150ms. Exposed in `NetConfig`.

## 13. Scope boundaries — what's NOT in Phase 14

- ❌ **Anti-cheat.** No signed inputs, no server-side hack detection, no integrity checks on the client binary. Games that ship PvP ship their own.
- ❌ **Matchmaking / lobbies / backfill.** Engine knows nothing about player pools, skill ratings, or session brokers. Connection is by address:port, that's it.
- ❌ **Voice chat.** Voice is a whole subsystem — codec, echo cancellation, spatialization. Separate phase if at all.
- ❌ **Dedicated-server orchestration.** No headless-mode auto-scaling, no process supervisor, no container image. Ship a `cargo run --bin my-game -- --dedicated` flag and call it done.
- ❌ **Cross-version compatibility.** Schema hash mismatch on handshake → disconnect. No graceful field-skipping across engine versions.
- ❌ **Peer-to-peer and lockstep.** See §1.
- ❌ **Replays.** Record-and-playback of a session is desirable and a common ask, but it's a separate feature that needs its own storage format. Later phase.
- ❌ **Bandwidth compression past bit-packed deltas.** No snap-compression, no domain-aware codecs, no ML compression. Measure first.
- ❌ **Federated servers / sharding.** Single-server authority only.
- ❌ **Editor-time collaborative editing.** That was Phase 12; it does not share code with this phase.

## 14. Build order within Phase 14

1. **Crate skeleton + `Transport` trait + `virt.rs`** — in-process loopback, nothing on the wire yet. PIE dual-world compiles against it.
2. **`NetId`, `EntityMap`, SceneId bridge.** Verify a baked scene loads with identical `NetId` on both endpoints.
3. **`#[replicate]` derive + schema hash.** Handshake rejects mismatched schemas. No encoder yet — just registration.
4. **Snapshot encode/decode, full-world, no AOI, no baselines.** End-to-end "server sees, client sees" over virt-transport.
5. **Baseline + delta encoding.** Ack piggyback. Measure bytes/tick in PIE, commit the number.
6. **Spawn / despawn reliable side-channel.** Runtime-spawned entities appear on clients.
7. **RPC channels** — reliable-ordered, reliable-unordered, unreliable. Native Rust first, then WASM bindings.
8. **QUIC transport via `quinn`.** Two editor instances on localhost successfully connect. Drop the virt-transport from the default PIE path behind a config flag (still used in tests).
9. **Client-side prediction** for owned entities. Shared `PredictedSystem` registration. History buffer.
10. **Server reconciliation with rollback-and-replay.** Verify with an intentional client-side divergence test.
11. **Interpolation buffer for non-owned entities.** 100ms render delay default.
12. **Extrapolation fallback** with the 2-tick clamp.
13. **Bandwidth budget + priority heap.** Hook to Phase 8 profiler.
14. **AOI trait, `FullWorldAoi` + `RadiusAoi`.**
15. **PIE networking UI** — topology dropdown, per-world viewport switcher, latency sim sliders.
16. **Netcode diagnostic panel** — bytes/tick per channel, reconcile rate, extrapolation rate.
17. **Soak tests** — 4 clients, 30 minutes, packet loss at 5%. Memory and correctness stable.

## 15. Risks & gotchas

- **Shared prediction/server code is hard to enforce.** Users will write a system that runs "only on the server" and then wonder why their client rubber-bands. The `PredictedSystem` registration trait should make the wrong thing impossible to express, not just discouraged. If it's discouraged-only, it will be violated.
- **Determinism inside predicted systems.** Even though this phase rejects global determinism, predicted systems must be locally deterministic — same input, same state → same output — or reconciliation oscillates. Any predicted system touching Rapier or threaded work needs a scoped single-threaded path.
- **Floating-point drift across client architectures.** An x86 client predicting differently from an ARM server because of `fma` differences produces constant reconciliation. Pin predicted math to non-fused operations, or accept the reconciliation bandwidth cost and document it.
- **Snapshot cost scales with world size × client count.** A 10k-entity world with 16 clients is 160k encode operations per tick. Measure before shipping. Consider snapshot sharing when clients have identical AOIs — but only if measured.
- **`EntityMap` leaks.** Forgetting to remove a `NetId` from the map on despawn is a slow memory leak that only shows up in 12-hour servers. Audit every despawn path. Write a test that runs 100k spawn/despawns and asserts the map is empty.
- **QUIC on hostile networks.** Corporate firewalls block UDP. A fallback to WebSocket-over-TCP is a thing other engines ship; we don't, and users will ask for it. Put it on the Phase 15 list the moment it's asked twice.
- **PIE latency sim makes bugs look like features.** A user tests at simulated 60ms+1% and ships. In production they hit 250ms+8%. The diagnostic panel should *always* show worst-case thresholds, not averages.
- **Time-warp / time-scale from Phase 7 §6.1 breaks prediction.** Prediction assumes monotonic server ticks. Time-scale != 1.0 inside PIE needs to be disabled when a client world is connected, or the reconciliation math produces nonsense. Easy to forget, impossible to miss once you do.
- **Schema hash churn during development.** Every `#[replicate]` change breaks the handshake. Fine in production, annoying during dev — add a `--accept-any-schema` dev flag with a giant console warning, scoped to debug builds only.
- **WASM script RPC flood.** A buggy script calling `rpc::send` every frame saturates the reliable-ordered channel in seconds. Per-script rate limits, not just per-RPC-type limits.
- **Replication-over-PIE vs replication-over-QUIC subtle differences.** The virt-transport is zero-loss by default; flip to "actual loss" early or the QUIC path will reveal bugs the virt-transport hid. PIE defaults to 1% loss for this reason.
- **Testing is harder than implementing.** A replication bug that occurs 1-in-10000 ticks at 60Hz manifests every 3 minutes and is nearly impossible to reproduce. Invest in deterministic packet-loss seeds in the virt-transport so failing tests can be replayed bit-exactly.

## 16. Exit criteria

Phase 14 is done when all of these are true:

- [ ] `rustforge-net` compiles standalone; `rustforge-core` builds without it; the `editor` feature adds `net_pie`.
- [ ] `#[replicate]` derive registers descriptors into the reflection registry and a schema-hash mismatch at handshake produces a clear disconnect reason.
- [ ] Baked `SceneId` entities have identical `NetId` on server and client without explicit messaging.
- [ ] Runtime-spawned server entities appear on clients via the reliable spawn channel; despawn removes them and drains the `EntityMap`.
- [ ] `reliable-ordered`, `reliable-unordered`, and `unreliable` RPCs work from native Rust and from WASM scripts, and respect per-RPC rate limits.
- [ ] Snapshots are baseline+delta encoded; an unchanged field costs exactly one bit on the wire; `cargo test` asserts this.
- [ ] QUIC transport connects two editor instances on localhost; TLS 1.3 is active; 0-RTT reconnect works.
- [ ] Virt-transport drives PIE dual-world with configurable latency and loss.
- [ ] PIE "Listen server + 1 client" is the default Play topology when the project has `[net] enabled = true`; the viewport has a world selector; Stop restores the editor world byte-identically (Phase 7 invariant preserved).
- [ ] Client-side prediction runs the same system function as the server; reconciliation rolls back and replays inputs on disagreement; a scripted divergence test shows one correction, not persistent drift.
- [ ] Non-owned entities interpolate at 100ms render delay; extrapolation activates for up to 2 ticks; extrapolation events log to the netcode panel.
- [ ] The `BudgetedSender` caps outgoing bytes/tick; low-priority updates defer without starving; spawn/despawn bypass the budget.
- [ ] The `AreaOfInterest` trait has `FullWorldAoi` and `RadiusAoi` implementations; a scene with 10k entities and a 50m radius AOI encodes only the visible subset.
- [ ] Netcode diagnostic panel shows bytes/tick per channel, reconcile rate, extrapolation rate, worst-case RTT per client.
- [ ] Soak test: 4 PIE clients on virt-transport @ 5% loss, 30 minutes, no memory growth, zero dangling `NetId`s on shutdown.
- [ ] No predicted system touches non-deterministic global state; CI lints against spawning rayon scopes inside a `PredictedSystem`.
- [ ] `rustforge-core` still passes every Phase 1–13 exit criterion unchanged.
