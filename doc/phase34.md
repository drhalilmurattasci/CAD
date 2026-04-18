# Phase 34 — Multiplayer Services: Lobbies, Matchmaking, Voice

Phase 14 shipped the wire: authoritative client-server, QUIC transport, `#[replicate]`, prediction, reconciliation, three RPC channels, PIE dual-world. A game running on Phase 14 can connect player A to player B at a known `address:port` and exchange state correctly. That is *netcode*. It is not *multiplayer*, not the way modern players use the word. Modern players expect to open a menu, see a friends list, invite a party, queue for a match, land in a lobby, chat with voice, ready-up, and be dropped into a server that was provisioned for them. None of that is transport. All of that is the layer Unreal calls `OnlineSubsystem` + `EOSSDK`, Unity calls `Relay` + `Lobby` + `Matchmaker`, and every shipped multiplayer game in our target niche implements some version of.

Phase 14 §13 explicitly punted six items to a later phase: matchmaking, lobbies, voice, dedicated-server orchestration, anti-cheat, replays. Phase 34 is the one that finishes the first four. Anti-cheat and replays stay out — anti-cheat is a defense-in-depth program, not a feature, and replays are a storage-format problem. Phase 34 delivers the service-layer crates and the self-hostable service binaries that sit between a game's main menu and the QUIC server Phase 14 can already stand up. The philosophical bet is the same as every other RustForge phase: the engine ships *tooling to self-host*, not *a hosted service*. Users run their own lobby server, their own matchmaker, their own voice relay, on their own infrastructure, with Docker/Kubernetes manifests we provide. We are not Epic Online Services. We are the thing a studio uses when they do not want to be locked to Epic Online Services.

## Goals

By end of Phase 34:

1. A **lobby service** — pre-match gathering, ready-up, text chat, member state — runnable as a self-hosted dedicated binary or as a peer-hosted listen-lobby for LAN/dev.
2. A **matchmaker service** with ELO-style skill-based matching, configurable queue constraints (region, mode, party size), and match-assignment output to the lobby service.
3. **Dedicated-server orchestration tooling** — Docker Compose files, Kubernetes manifests, and a bare-metal supervisor script — that provisions Phase 14 game-server processes on user infrastructure.
4. **Voice chat** — Opus-encoded, server-relayed, with positional/proximity audio plumbed through the Phase 17 `phonon` spatializer, plus team channels.
5. **Text chat** — lobby, team, and proximity channels with a user-configurable profanity filter and moderation-hook trait for UGC titles.
6. A **party system** — persistent pre-lobby groups that carry through matchmaking as a unit.
7. A **friends list** abstraction with adapter-trait integrations for Steam, Epic, PlayStation Network, and a RustForge-native email/username fallback.
8. A **`PlayerId` identity layer** — abstract handle over whichever identity provider the game picks, with an adapter trait for plugging in platform SDKs.
9. **Session discovery** — browse available games by filters (map, mode, player count) against a self-hosted directory; minimum viable scope, no global browser.
10. **Anti-spoofing** — lobby admission authenticates the incoming player via a platform token before allocating a slot.
11. **Spectator mode** — observers connect, receive AOI-filtered snapshots at a throttled rate, and send no input.
12. **Reconnect logic** — a dropped client holds its slot for a configurable grace window and rejoins with a session token rather than re-queuing.
13. **Presence** — "Friends playing X" notifications routed through the identity adapter.
14. An exit-criteria checklist runnable by one author against a locally-hosted stack (lobby + matchmaker + one game server + voice relay) on one machine.

## 1. Architecture — services, not a monolith

The temptation is one big `rustforge-online` crate that does everything. That temptation should be resisted. Games have wildly different needs: a 4-player co-op title wants a trivial listen-lobby and no matchmaker at all; a 64-player competitive shooter wants a matchmaker, voice relay, and region-aware server fleet. A monolith forces the first game to pull in the second game's dependencies. Split along service seams.

```
  ┌──────────────────────────── game client ────────────────────────────┐
  │                                                                     │
  │  rustforge-net (Phase 14)                                           │
  │  rustforge-identity ── PlayerId, adapter trait                      │
  │  rustforge-party    ── party state, invites                         │
  │  rustforge-lobby    ── lobby client, chat                           │
  │  rustforge-match    ── matchmaker client                            │
  │  rustforge-voice    ── Opus capture/playback, phonon bridge         │
  │  rustforge-presence ── friends list, presence notifications         │
  └─────────────────────────────────────────────────────────────────────┘
        │               │              │               │
        │ QUIC game     │ HTTPS/WS     │ HTTPS/WS      │ QUIC voice
        ▼               ▼              ▼               ▼
  ┌──────────┐    ┌──────────┐   ┌────────────┐  ┌─────────────┐
  │ game srv │    │ lobby srv│   │ matchmaker │  │ voice relay │
  │ (Phase14)│    │          │   │            │  │             │
  └──────────┘    └─────┬────┘   └──────┬─────┘  └─────────────┘
                        └────────┬──────┘
                                 ▼
                         ┌──────────────┐
                         │ postgres /   │  single persistence store
                         │ sqlite       │  shared between lobby+match
                         └──────────────┘
```

Each service is a separate crate, a separate binary, and a separate container image. The game client pulls only the client-side crates it needs. A project that does not want matchmaking depends on `rustforge-lobby` and skips `rustforge-match` entirely.

Transport between client and services is HTTPS + WebSocket for the control plane (low frequency, high reliability, debuggable) and QUIC for voice (same stack as Phase 14, low latency). Game-server traffic remains Phase 14 QUIC, untouched.

## 2. Module layout

```
crates/
├── rustforge-identity/       # PlayerId, IdentityProvider trait
│   └── adapters/
│       ├── steam.rs          # feature = "steam"
│       ├── epic.rs           # feature = "epic"
│       ├── psn.rs            # feature = "psn"
│       └── native.rs         # email/username fallback (default)
├── rustforge-party/          # client-side party state + invite protocol
├── rustforge-lobby/
│   ├── client/               # embedded in game client
│   └── server/               # the lobby-server binary's library
├── rustforge-match/
│   ├── client/
│   └── server/               # matchmaker binary's library
├── rustforge-voice/
│   ├── client/               # Opus encode/decode, mic/speaker
│   ├── server/               # voice-relay binary's library
│   └── spatial.rs            # phonon bridge (Phase 17)
├── rustforge-presence/
└── rustforge-orchestration/  # docker-compose, k8s manifests, supervisor
    ├── docker/
    ├── k8s/
    └── supervisor/           # bare-metal process supervisor (Rust)
binaries/
├── rustforge-lobby-server/
├── rustforge-matchmaker/
├── rustforge-voice-relay/
└── rustforge-supervisor/     # bare-metal orchestration
```

None of these depend on `rustforge-editor`. The editor gains a "Services" panel that launches local instances for PIE testing, but the services build and run without the editor.

## 3. Identity — one `PlayerId` to rule them all

Every other service in this phase takes a `PlayerId`. That type must be stable across platforms or the whole stack fragments. The design is an opaque 128-bit handle plus an adapter-resolved provider tag.

```rust
#[derive(Reflect, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlayerId {
    pub provider: ProviderTag,  // u16: Steam=1, Epic=2, PSN=3, Native=4, ...
    pub opaque:   [u8; 16],     // provider-specific bits
}

pub trait IdentityProvider: Send + Sync + 'static {
    fn provider_tag(&self) -> ProviderTag;
    fn current_player(&self) -> Option<PlayerId>;
    fn display_name(&self, id: PlayerId) -> Option<String>;
    fn avatar_url(&self, id: PlayerId) -> Option<Url>;
    fn mint_auth_token(&self) -> anyhow::Result<AuthToken>;   // for lobby admission
    fn verify_auth_token(&self, token: &AuthToken) -> anyhow::Result<PlayerId>;
}
```

Rules:

- **Provider tags are assigned centrally.** No user-defined tags in the 0..255 range; custom providers get 256+. Prevents two ship'd games allocating the same tag for incompatible providers.
- **`mint_auth_token` / `verify_auth_token` are the anti-spoofing primitives.** The client mints a token using the platform SDK; the lobby server verifies it with the same provider on the server side. Steam's session tickets, Epic's auth tokens, PSN's NP tokens, and our native JWT all fit this interface.
- **Native fallback is the default.** Games that do not want a platform dependency get email+password+JWT out of the box, backed by the same Postgres the lobby+matchmaker use. Not sexy, works.
- **No profile data in `PlayerId`.** Display name and avatar are queried through the adapter and cached. A user changing their Steam name does not change their `PlayerId`.

The adapter pattern matches Phase 14's `Transport` trait philosophy: one narrow interface, multiple interchangeable implementations behind feature flags.

## 4. Lobby service

A lobby is a bag of players waiting to start a match. It holds: a host, a list of members with ready-state, a map/mode selection, chat history, and a start-match trigger. It is *not* the game server — when the host starts the match, the lobby hands off to a game-server instance (provisioned by §6) and the lobby either dissolves or persists as a "back-to-lobby" rendezvous.

Two deployment shapes:

- **Dedicated lobby server** (`rustforge-lobby-server` binary). The lobby lives on infrastructure; clients connect over WebSocket. Host migration is trivial because the lobby is not the host. Recommended for production.
- **Peer-hosted listen-lobby.** One client *is* the lobby; peers connect to them directly. Zero infrastructure. Dies when the host disconnects. Fine for LAN parties, private games, and development. Not fine for matchmade sessions.

Both shapes implement the same protocol; `rustforge-lobby::client` does not know which it is talking to beyond a connection URL.

Wire protocol (WebSocket, JSON-with-optional-CBOR — human-debuggable wins over 5% bandwidth here):

```
client → server: Join { lobby_id, auth_token }
server → client: JoinAck { you_are: PlayerId, members: [...] }
server → *:      MemberJoined | MemberLeft | MemberReady | HostChanged
client → server: SetReady(bool) | Chat(msg) | StartMatch
server → *:      ChatMessage { from, channel, body }
server → *:      MatchReady { game_server_addr, session_token }
```

Invariants:

- **Lobby admission requires an adapter-verified auth token.** A client cannot join a lobby by guessing an ID.
- **The host is the only member allowed to send `StartMatch`.** Host migration on disconnect is prep-only in this phase: the lobby server selects a new host deterministically (oldest-joined non-spectator), but does not migrate an active game session. In-game host migration is out of scope — if the game has a host, it is a Phase 14 listen-server, and its failure is the game's problem.
- **Lobbies have a TTL.** Idle lobbies evict after 15 minutes default; configurable.

## 5. Matchmaker service

The matchmaker takes a queue of players with skill ratings and party bindings and produces match assignments. Opinionated design:

```rust
pub struct QueueEntry {
    pub player:      PlayerId,
    pub party:       Option<PartyId>,   // all party members enter together
    pub rating:      f32,               // ELO, mean
    pub uncertainty: f32,               // ELO, sigma (Glicko-ish)
    pub constraints: QueueConstraints,  // region, mode, map prefs
    pub queued_at:   Instant,
}

pub struct QueueConstraints {
    pub region:   Region,
    pub mode:     ModeId,
    pub max_ping: u32,        // ms; widens as queue time grows
}
```

Algorithm: ELO-with-widening. Every tick (default 1 Hz), the matchmaker scans the queue, attempts to form matches of the configured size, and widens acceptable skill-delta as entries age. Widening curve is configurable per mode; defaults ship for 1v1, small-team (≤5), and large-team (≤16).

```
  tick:
    for each mode in modes:
       entries = queue.filter(mode)
       sort by queued_at ascending
       for head in entries:
          window = ratings_within(head, delta(head.age))
          if window.fills_match(mode.party_rules):
             emit MatchAssignment { members: window, game_server: alloc(mode.region) }
             remove from queue
```

Rules:

- **Parties are atomic.** A party of 3 entering a 5v5 queue either lands in a match with 7 other compatible players or stays queued. No splitting.
- **`MatchAssignment` is a request, not an allocation.** The matchmaker asks the orchestration layer (§6) to provide a game server. If provisioning fails, the assignment is cancelled and members re-queue at the head.
- **No skill-rating *writeback* from the engine.** The matchmaker reads ratings; the game decides how to update them after a match and writes to the same store. Updating-formulas are a game-design decision, not an engine one.
- **ELO is a default, not a mandate.** The matchmaker is generic over a `SkillModel` trait; games that want TrueSkill or OpenSkill plug it in.

## 6. Dedicated-server orchestration

This is tooling, not a service. A game running Phase 14 already has a `cargo run --bin my-game -- --dedicated` mode. The orchestration crate adds three ways to multiply that:

```
  orchestration/
    docker/
      Dockerfile.game-server        # multi-stage, scratch-based
      docker-compose.dev.yml        # lobby + matchmaker + 1 game srv + voice relay
    k8s/
      game-server-deployment.yaml   # horizontal scale, readiness probe
      game-server-service.yaml      # headless service for allocator
      lobby-statefulset.yaml
      matchmaker-deployment.yaml
    supervisor/                     # Rust binary: rustforge-supervisor
      src/main.rs                   # spawns game-server processes on bare metal
```

The `rustforge-supervisor` bare-metal option is the minimum-viable allocator: a long-lived process that accepts "provision a server for mode X, map Y, return address:port" HTTP requests from the matchmaker and forks game-server children. Health-checks, port allocation, log forwarding. No Kubernetes required.

Rules:

- **No hosted RustForge orchestration.** We ship manifests and a supervisor; users run it on their AWS/GCP/Hetzner/their-basement.
- **Game-server allocation is pluggable.** `trait Allocator { fn provision(&self, req: AllocReq) -> Fut<GameServer>; }`. Implementations ship for: supervisor (bare-metal), Kubernetes (via the K8s API), Docker (local), and Nomad (community-contributed stub). Users with a custom allocator implement the trait.
- **The Phase 14 game server binary is unchanged.** Orchestration wraps it; it does not modify it. This is the contract: a single-player-capable binary with a `--dedicated` flag is all orchestration needs.

## 7. Voice chat

Voice is its own wire path, not RPCs on the game channel. Opus-encoded 20ms frames, ~24kbps per speaker, QUIC datagram transport, server-relayed (not P2P — P2P voice requires NAT traversal we are not shipping).

```
  ┌──────────┐   Opus frames (QUIC datagram)    ┌──────────────┐
  │ client A │ ─────────────────────────────────▶│ voice relay  │
  └──────────┘                                   │              │
                                                 │ mixes? no.   │
  ┌──────────┐                                   │ forwards yes │
  │ client B │ ◀───────────────────────────────── │              │
  └──────────┘  Opus frames from A (if in range) └──────────────┘
```

Mixing happens on the *receiving* client, not on the relay. The relay decides *who hears whom* based on channel membership and (for proximity) a position table the game server publishes to the relay. This keeps the relay CPU-cheap and lets each client apply `phonon` spatialization locally with its own HRTF.

Channels:

- **Team channel.** All members of a team hear each other regardless of distance.
- **Proximity channel.** Members within N meters of each other hear each other, attenuated by distance, spatialized via Phase 17's `phonon` bridge.
- **Lobby channel.** Pre-match chat. Drops when the match starts.
- **Spectator channel.** Spectators hear each other; players do not hear spectators.

Rules:

- **Positional audio uses the same `Transform` component the game uses.** The game server publishes a compact (PlayerId, position) table to the voice relay at ≤10 Hz. The relay uses this for the range-check and passes the relative-position to the receiver for spatialization.
- **Voice frames carry the sender's `PlayerId`.** Receivers look up mute-state and player volume locally. Server-side mute is a moderation tool and available, but per-listener mute is client-side.
- **Push-to-talk and open-mic both ship.** Open-mic defaults to VAD (voice-activity detection) with a high threshold. Shipping open-mic-always is a cruelty to teammates.
- **Opus codec only.** No legacy Speex, no AAC, no Skype-clone formats. Opus is the right answer for game voice and has been for a decade.

## 8. Text chat

Simpler than voice but gets most of the moderation headaches.

```rust
pub struct ChatMessage {
    pub from:    PlayerId,
    pub channel: ChannelId,          // Lobby | Team | Proximity | Match
    pub body:    String,             // already filtered server-side
    pub ts:      ServerTime,
}

pub trait ModerationHook: Send + Sync + 'static {
    fn filter(&self, msg: &mut ChatMessage) -> FilterResult;  // Pass | Redact | Drop
}
```

Filters run server-side (lobby server for lobby chat; game server for in-match chat). Default filter is a user-configurable wordlist shipped as a TOML file. UGC titles implement `ModerationHook` to call out to their preferred service (Microsoft Community Sift, ActiveFence, in-house). RustForge does not operate a moderation service.

Rules:

- **Filtering is mandatory in the codepath; the filter implementation is a choice.** A game that wants unfiltered chat ships a `NoopFilter`, knows what it is doing, and accepts the consequences. The codepath always goes through a `ModerationHook`.
- **Chat is rate-limited per-sender.** Per Phase 14 §5; same token-bucket machinery.
- **Chat history is ephemeral by default.** No server-side persistence unless the game explicitly enables an audit-log hook. Storing chat is a regulatory decision (GDPR, COPPA), not an engine decision.

## 9. Party system

Parties are persistent-across-sessions groups. Three to eight friends queue together, chat together, carry into matches together, persist after the match ends.

```rust
pub struct Party {
    pub id:      PartyId,
    pub leader:  PlayerId,
    pub members: SmallVec<[PlayerId; 8]>,
    pub max:     u8,
    pub mode_preference: Option<ModeId>,
}
```

Invites flow through the presence service (§11) — a party invite is a typed presence notification, not its own wire protocol. Party membership persists in the identity store; relogging does not dissolve a party.

Rules:

- **Leader is authoritative on queue actions.** Only the leader presses "queue for match." Members can set ready/unready but cannot pull the trigger.
- **Parties are lobby citizens.** When a party joins a lobby, the lobby tracks the party binding so members auto-assign to the same team when the match starts.
- **Cross-provider parties are allowed.** A Steam player and an Epic player in the same party is a supported configuration; the underlying `PlayerId` provider-tag is what matters, and the services treat all tags uniformly.

## 10. Session discovery

Minimum viable. A self-hosted directory endpoint that lobby servers register with; clients query with filters.

```
GET /sessions?mode=team-deathmatch&map=dust2&min_slots=2&region=eu-west
→ [ { lobby_id, name, host, map, mode, players: 6/10, region, ping: 42ms }, ... ]
```

Rules:

- **Directory is self-hosted.** RustForge does not run a global session browser. A game runs its own directory or uses matchmaking instead.
- **Lobbies opt in to listing.** A private lobby is invisible to the directory. Default is unlisted.
- **Ping is measured client-side.** The directory returns the lobby's address; the client probes and displays.

## 11. Friends list and presence

Two separate concerns bound together by the identity adapter.

Friends list:

```rust
pub trait FriendsProvider: Send + Sync + 'static {
    fn friends(&self) -> anyhow::Result<Vec<PlayerId>>;
    fn add_friend(&self, id: PlayerId) -> anyhow::Result<()>;
    fn remove_friend(&self, id: PlayerId) -> anyhow::Result<()>;
}
```

Native adapter uses the same Postgres the lobby+matchmaker share. Platform adapters defer to the platform SDK. A user playing on Steam sees their Steam friends; a user on the native adapter sees their RustForge-native friends list.

Presence:

```rust
pub struct Presence {
    pub player:   PlayerId,
    pub state:    PresenceState,     // Offline | Online | Lobby | InMatch | Spectating
    pub title:    Option<String>,    // "Playing: de_dust2"
    pub joinable: Option<LobbyId>,   // "Join game" deep-link
}
```

A background task on the client publishes presence updates through the identity adapter; friends subscribe. "Friends playing X" notifications are UI-level — the presence stream delivers the data, the game renders the toast. Engine does not mandate UI.

Rules:

- **Presence is opt-in with sane defaults.** Online-status is on by default; detailed-title is on by default for the native adapter, follows platform policy otherwise. Users can set presence to invisible.
- **Presence is eventually-consistent.** No real-time delivery guarantees. If a friend sees "in lobby" 5 seconds stale, that is fine.

## 12. Anti-spoofing on lobby admission

Every `Join` request to a lobby carries an auth-token minted by the client's identity provider. The lobby server calls `IdentityProvider::verify_auth_token` before allocating a slot. Failed verification → reject with a clear reason code.

```
client:                             lobby server:
  tok = ident.mint_auth_token()
  ws.send(Join { lobby_id, tok })
                                    pid = ident.verify_auth_token(tok)?  // fails if spoofed
                                    check_ban_list(pid)?
                                    lobby.members.insert(pid)
                                    ws.send(JoinAck)
```

Tokens are short-lived (≤5 minutes) and single-use on the lobby side — a replay of the same token past expiry is rejected. The game server that the lobby hands off to accepts a lobby-issued `session_token` (signed by the lobby server) rather than re-verifying with the platform; this keeps platform-SDK chattiness bounded.

## 13. Spectator mode

Spectators are clients with the `role = Spectator` flag. The game server admits them to a separate spectator slot pool, encodes snapshots with the `visible_only` AOI predicate relaxed (spectators see everything, or whatever the game's spectator-AOI says), and ignores their input packets. Outgoing bandwidth for spectators is throttled:

```rust
pub struct SpectatorPolicy {
    pub max_snapshot_hz:   u8,       // default 15 (vs 60 for players)
    pub voice_enabled:     bool,     // spectator-channel only
    pub max_per_match:     u16,      // default 50
}
```

Rules:

- **Spectators never send input.** Transport-level enforcement: the game server drops any input-typed RPC from a spectator connection.
- **Spectator snapshot rate is a server-side knob.** 15 Hz is invisible for anyone not playing; the bandwidth savings scale with spectator count.
- **Spectator voice is siloed.** See §7.

## 14. Reconnect logic

Client drops. Game server holds the slot for `reconnect_grace` (default 30 seconds). The disconnecting client received a `session_token` at admission; reconnecting within the grace window with the same token re-attaches to the same `NetId`-owning entity and the same Phase 14 prediction history is re-synced from the server.

```
t=0    client → server: (disconnect, not graceful)
t=0    server: player P slot → Dormant, grace_until = now + 30s
t=12s  client → server: Reconnect { session_token }
t=12s  server: verify token, P slot → Active, resume replication
t=31s  server: grace expired → P slot freed, entity despawned, party notified
```

Rules:

- **Session tokens are per-match, not per-session.** A token issued for match M is invalid for match M+1.
- **Reconnect does not re-run matchmaking.** The same game server the player left is the one they reconnect to. If the game server died, reconnect fails and the player re-queues.
- **Grace window is bounded.** 30s default, cap at 120s. Longer than that degrades the experience for the remaining players more than it helps the disconnected one.

## 15. Scope ❌ — what's NOT in Phase 34

- ❌ **Hosted RustForge online service.** We ship binaries, containers, and manifests. We do not run infrastructure on behalf of users.
- ❌ **Payment processing and monetization.** Storefronts handle payment. Engine does not touch money.
- ❌ **Account creation flows, KYC, age-gating, region-specific compliance.** GDPR, COPPA, China-specific requirements — user's responsibility. Engine provides the identity adapter; what it authenticates against is a business-and-legal decision.
- ❌ **Cheat detection at scale.** Server-side sanity checks on RPCs from Phase 14 §5 remain; global anti-cheat (BattlEye, EAC, kernel-level) is not engine surface area.
- ❌ **Community servers directory hosted by RustForge.** Users running their own directory is supported; a global RustForge-operated directory is not.
- ❌ **Cross-platform progression as a service.** Identity abstracts the provider; a cross-platform progression store is a game-design feature the game builds on top of the identity store.
- ❌ **In-game reporting/moderation review workflows.** Reporting-UI is a game feature; the moderation-hook trait exposes the plumbing, the workflow is not engine surface.
- ❌ **Voice transcription, translation, speech-to-text.** Opus frames in, Opus frames out. No ML in the voice path.
- ❌ **Host migration mid-match.** Prep for it in the lobby; do not execute it in the game. If the host dies, the match ends.
- ❌ **Replays, demos, spectator-from-replay.** Phase 14 punted replays; Phase 34 does not pick them up.
- ❌ **Peer-to-peer NAT traversal for voice.** Server-relay only.
- ❌ **Social-graph features beyond friends/parties.** No guilds, clans, chat rooms outside lobby/match/team/proximity channels.

## 16. Build order within Phase 34

1. **`rustforge-identity` crate + native adapter + `PlayerId`.** Postgres schema, JWT minting/verification, username/email registration. Smoke test: mint-verify-roundtrip passes.
2. **Steam adapter** behind `steam` feature flag. Smoke test: real Steam session ticket verifies.
3. **Epic + PSN adapters** behind their own feature flags. Stubs until SDK access.
4. **`rustforge-lobby-server` binary + client crate.** WebSocket protocol, admission via `IdentityProvider::verify_auth_token`, in-memory lobby state. Single-box stack.
5. **Text chat on lobby + moderation-hook trait + default wordlist filter.**
6. **`rustforge-party` crate + party persistence in Postgres + invite protocol.**
7. **`rustforge-matchmaker` binary + matchmaker client + ELO skill model + queue-widening.** Emits `MatchAssignment` to a stub allocator.
8. **`rustforge-orchestration` — supervisor binary first.** Allocator trait; supervisor impl forks Phase 14 game-server processes, reports address:port.
9. **Docker Compose for dev stack.** `docker compose up` brings up lobby + matchmaker + supervisor + Postgres + voice relay stub.
10. **Kubernetes manifests.** Same stack, k8s shape. Readiness probes, headless services, horizontal pod autoscaler on game-server deployment.
11. **`rustforge-voice-relay` binary + client crate.** Opus frames, QUIC datagrams, relay forwarding, team channel, lobby channel, spectator channel.
12. **Proximity channel + `phonon` spatial bridge** (Phase 17 dependency). Game server publishes position table at 10 Hz.
13. **`rustforge-presence` crate + friends-list adapter wiring.** Native store backs friends; Steam/Epic adapters defer.
14. **Session discovery directory service.** Simple HTTPS endpoint, lobby registration, filter query.
15. **Session-token handoff from lobby to game server.** Lobby signs a per-match token; game server verifies against the lobby's public key.
16. **Reconnect logic on the game server.** Dormant-slot grace window, session-token reattachment.
17. **Spectator mode** — role flag, snapshot throttle, input rejection, spectator-channel voice.
18. **Editor "Services" panel.** Launch local lobby + matchmaker + voice relay + supervisor from the editor for PIE testing.
19. **End-to-end soak test.** 16 clients, 2 parties of 4 + 8 solos, full stack, 30 minutes, reconnects injected at 5%, memory stable.

## 17. Risks & gotchas

- **Identity provider feature-flag combinatorics.** `steam + epic + psn` is nine build configurations and every one of them must compile and test. Budget CI time; skipping a config means shipping a broken one.
- **Platform SDK licensing.** Steamworks, EOS SDK, PSN SDK have distinct terms. The adapter code lives behind feature flags and the SDK itself is user-provided (downloaded under their developer account), never bundled. Document this explicitly or a user will open a PR that vendors Steamworks into the repo.
- **Matchmaker parameter tuning is a game-design problem masquerading as an engine problem.** Widening curves, skill-window shapes, party-balance rules — shipped defaults will be wrong for most games. Make every knob reachable from config, not from code, or users will fork.
- **Voice relay CPU at scale.** Forward-only design keeps per-stream cost low, but a 64-player match with all voices open is 64 × 63 forwarding decisions per frame. Profile. Consider interest-management for proximity voice (same cell-grid idea the game-server AOI avoids).
- **Phonon on voice is lovely and easy to misconfigure.** HRTF applied on a voice stream already reverb'd by the speaker's room sounds worse than no HRTF. Document source-signal assumptions; default aggressive noise-suppression on mic capture.
- **Lobby server split-brain on host migration.** If two lobby-server replicas race to appoint a new host, members see oscillation. Deterministic selection (oldest-joined) plus a single-writer invariant on the lobby record in Postgres closes this; the invariant is easy to forget under load.
- **Token replay windows.** Auth-tokens with a 5-minute validity, session-tokens per-match, reconnect-tokens per-drop — three separate token types means three chances to get expiry wrong. Centralize the token-signing/verifying logic, do not scatter it.
- **Profanity filters are wrong.** Every word-list filter has false positives (the "Scunthorpe problem") and false negatives (leetspeak). Ship the default with eyes open and a `ModerationHook` trait for anyone who needs better.
- **GDPR and friends.** A chat message stored for audit is personal data. A `PlayerId`-indexed row is personal data. We do not store these by default; the moment a user enables audit logging they enter regulated territory. Documentation must say this loudly; engine does not enforce compliance.
- **Orchestration on Windows.** The supervisor forks child processes; on Windows that is `CreateProcess`, not `fork`. The Rust abstraction is uniform; debugging when it goes wrong is not. Test the supervisor on Linux *and* Windows Server, not only Linux.
- **Voice in PIE.** Two client worlds in one process both want to capture the mic. The editor's Services panel must expose a "simulated voice" mode that feeds a canned audio sample through the pipeline, not the real microphone, or testing voice in PIE is impossible.
- **Reconnect creates a zombie-entity class of bugs.** A dormant slot holding a `NetId` that the rest of the world still references is a Phase 14 `EntityMap` leak waiting to happen. Every despawn path touched in Phase 14 needs a matching "grace-expired" despawn path here, with the same memory-leak test (§P14 exit criterion 15) extended to include reconnect cycles.
- **Matchmaker downtime kills gameplay.** A lobby that cannot reach the matchmaker cannot start a match. The lobby server should degrade: offer "play a private match" when the matchmaker is unhealthy, not just error.

## 18. Exit criteria

Phase 34 is done when all of these are true:

- [ ] `rustforge-identity` native adapter mints and verifies JWTs against a Postgres-backed user store; Steam/Epic/PSN adapters compile behind feature flags; provider-tag allocations are documented.
- [ ] `rustforge-lobby-server` accepts WebSocket clients, verifies `auth_token` via `IdentityProvider`, tracks members with ready-state, and routes text chat through a `ModerationHook`.
- [ ] Peer-hosted listen-lobby mode works on LAN without the dedicated lobby binary.
- [ ] `rustforge-matchmaker` runs an ELO queue with widening, respects party atomicity, honors region/mode constraints, and emits `MatchAssignment` to a pluggable `Allocator`.
- [ ] `rustforge-supervisor` provisions Phase 14 game-server processes on bare metal; Docker Compose stack brings up the full service suite; Kubernetes manifests deploy the same stack with readiness probes.
- [ ] `rustforge-voice-relay` forwards Opus-over-QUIC-datagram frames across team, lobby, proximity, and spectator channels; proximity uses position data published by the game server at ≥10 Hz; `phonon` spatializes received frames client-side.
- [ ] `rustforge-party` persists parties in the identity store; cross-provider parties (e.g. Steam + native) queue and match as a unit.
- [ ] Friends list and presence work through the native adapter; Steam/Epic adapters smoke-test against the platform SDKs under their feature flags.
- [ ] Session discovery directory lists opted-in lobbies, filters by mode/map/region, returns ping-testable addresses.
- [ ] Lobby admission rejects spoofed tokens with a clear reason code; session-token handoff from lobby to game server does not require the game server to hit the platform SDK.
- [ ] Spectator clients receive AOI-relaxed snapshots at the throttled rate; server drops spectator input at the transport layer; spectator voice channel does not leak into player channels.
- [ ] Disconnected clients reconnect within the 30-second grace window using their session token and resume replication on the same `NetId`-owning entity; grace expiry frees the slot and drains the map.
- [ ] Text chat passes through the default profanity filter; a `NoopFilter` is available for testing; per-sender rate limits apply.
- [ ] Editor "Services" panel launches a local stack for PIE; voice uses the simulated-capture mode, not the real microphone.
- [ ] 30-minute soak test with 16 clients, 2 parties of 4, 8 solos, full stack, 5% simulated reconnect rate: no memory growth, no token-replay acceptance, no orphaned lobbies, no dangling game-server allocations.
- [ ] Phase 14 exit criteria all still pass; none of the additions to the game server or client-side net stack regress prediction, reconciliation, or PIE invariants.
- [ ] Orchestration tooling runs successfully on Linux and Windows Server; CI exercises both.
