# Phase 23 — Ecosystem, Marketplace & Documentation

RustForge 1.0 shipped an editor. Phases 14–22 carried it into the post-1.0 world: a renderer worth looking at, physics that don't lie, audio that mixes, networking, animation state that survives a retarget, a console target, mobile, a web export. Each of those phases added a capability. Phase 23 does something different. It ships almost no new engine code. What it builds is the infrastructure that lets people *other than us* build things on top of the engine: sample projects to learn from, templates to start from, documentation to look things up in, a plugin directory so the work of Phase 11 compounds, conventions for asset packs, a template for authoring plugins, and the lightweight governance that stops a community from either stalling or sprawling.

This is the phase that turns "an engine" into "an ecosystem." We are explicitly **not** building an Unreal-style paid marketplace. There is no billing, no DRM, no editorial review queue, no hosted cloud builds, no mandatory account. There is a curated samples library, an index file in a Git repository, an mdBook portal, and an F1 key that opens the right page. The leverage comes from making the unglamorous pieces — discoverability, onboarding, prose — as good as the engine itself.

## Goals

By end of Phase 23:

1. A **curated samples library** under CC0/MIT covering the common starter genres, reachable from the Phase 13 Welcome Window in one click.
2. A **project template system** — archive + manifest — with five first-party templates and a Git-URL install path for third-party templates.
3. An **mdBook documentation portal** split into eight top-level sections, built from the same repo and versioned with the engine.
4. **In-editor help** that fills Phase 13 §8's `F1` stub with real pages for every panel and every reflected type, available both offline and online.
5. A **community plugin directory** — a JSON index file maintained in a public Git repo — consumed by an editor plugin browser that installs via Phase 11's `rustforge plugins fetch`.
6. **Asset-pack conventions**: a `pack.toml` manifest and recommended `.gitattributes` rules that let pack authors ship to Git hosts without reinventing layout.
7. A **`cargo generate` plugin template** (`rustforge-plugin-template`) that produces a working, testable `rustforge-plugin-api` skeleton in one command.
8. **Versioning and release cadence** written down: editor semver, plugin-api semver-pre-1.0 discipline, three-month minor releases, LTS after 2.0.
9. **Community channels** wired into the editor: GitHub Discussions, Discord, issues, a "Feedback" menu link. No in-editor forum.
10. An **accessibility statement** and a documented keyboard-only workflow, on the record and enforced by the Phase 13 CI smoke test.
11. A **localization contribution flow** — PRs against the Phase 13 `en-US.ftl` string tables, with a style guide.
12. **Lightweight governance**: benevolent-maintainer model, contribution guide, code of conduct, an RFC process for changes that deserve design discussion.
13. **Telemetry transparency** restated — no usage telemetry, crash reports opt-in only — and linked from every surface where a user might wonder.
14. A **trademark and naming policy** that lets the community describe their work honestly without forcing us to litigate.

## 1. Build it in the right order

Phase 23 is load-bearing on its own outputs. Sample projects need templates to be instantiable; templates are easier to author once docs exist; the plugin directory is dead weight until the docs explain how to author a plugin; governance docs are meaningless without contribution paths. The build order matters.

1. **mdBook scaffold** — empty sections, CI that builds and deploys. Nothing to write yet, but the shelf exists.
2. **Getting Started + Core Concepts** — enough prose that a new user can open the editor and not bounce.
3. **Samples library v1** — three samples (3D platformer, top-down shooter, first-person walker). Each is also a proof-of-concept for the template system.
4. **Template system** — extract the samples' skeletons into first-party templates.
5. **Plugin author docs + `cargo generate` template** — the bait for the plugin directory.
6. **Plugin directory index format + editor browser** — Phase 11's fetcher gets a UI.
7. **Asset-pack convention + `pack.toml`** — small spec, referenced by the samples.
8. **F1 wiring** — every panel and reflected type resolves to a docs page.
9. **Governance docs** — CODE_OF_CONDUCT, CONTRIBUTING, GOVERNANCE, RFC process.
10. **Accessibility statement + keyboard-only workflow pages** — reuse the Phase 13 smoke test as the enforcement mechanism.
11. **Localization contribution flow** — one section under Community, tied to the Phase 13 string freeze rule.
12. **Trademark / naming policy** — written down, linked from the repo README.
13. **Release cadence and LTS policy** — policy documents plus a CI calendar reminder.

## 2. Samples library

### 2.1 Curated, not open

The samples live in a separate repository, `rustforge/samples`, not under `rustforge/rustforge`. Every sample is reviewed by the core team before merge. The bar is: *"a new user opening this should learn one specific thing cleanly."* Samples that demonstrate five things compete with themselves; three clean samples beat ten sprawling ones.

License: **CC0 for art and audio, MIT for code.** No exceptions. A sample that carries an ambiguous license is a trap for every learner who copies it.

### 2.2 The initial seven

```
rustforge/samples/
├── 3d-platformer/          # character controller, jump, coyote time, death/respawn
├── topdown-shooter/        # grid-aligned camera, simple enemy AI, bullet hell
├── first-person-walker/    # Phase 13 Welcome default; walking, looking, opening doors
├── networked-demo/         # Phase 20 client/server, 4-player lobby, deterministic replay
├── particles/              # GPU particles gallery, 12 effects
├── materials/              # material editor showcase, PBR + stylized
└── ui/                     # menus, HUD, settings page wired to Preferences
```

Each sample is a full RustForge project — it clones, it opens, it runs. No "setup instructions" beyond `rustforge open`.

### 2.3 Reachable from the Welcome Window

Phase 13 §6 shipped a Welcome Window with a stubbed "Samples" section. Phase 23 fills it. The editor on first boot fetches the samples index over HTTPS (cached for 24 h), displays it, and clones on demand.

```
┌─ Welcome to RustForge ─────────────────────────────┐
│   Samples                                           │
│    3D Platformer           [Clone and open]         │
│    Top-Down Shooter        [Clone and open]         │
│    First-Person Walker     [Clone and open]         │
│    Networked Demo          [Clone and open]         │
│    Particles Showcase      [Clone and open]         │
│    Materials Showcase      [Clone and open]         │
│    UI Showcase             [Clone and open]         │
│                                                     │
│   Offline? [Open sample from local folder]          │
└─────────────────────────────────────────────────────┘
```

"Clone and open" runs `git clone` into a user-chosen directory (default `~/RustForgeProjects/<sample>`), opens the project, and — this matters — marks the first scene so a banner reads *"This is a sample project. Your edits stay local; pull from upstream with `git pull` to get updates."* The sample is a normal Git project from that point on.

Offline path: samples can also ship alongside installer builds as an optional component for users behind corporate firewalls.

## 3. Template system

A template is *a sample with the project-specific content stripped out*. The distinction matters: a sample teaches; a template starts.

### 3.1 Archive + manifest

```
first-person-template.rftpl     # just a tar.gz with a known extension
├── template.toml
├── skeleton/
│   ├── rustforge-project.toml
│   ├── assets/...
│   ├── scenes/...
│   └── scripts/...
└── README.md
```

`template.toml`:

```toml
[template]
id          = "first-person"
name        = "First-Person Starter"
version     = "1.0.0"
api_version = "1.2"
authors     = ["RustForge core team"]
description = "Walking, looking, basic interaction. Good starting point for puzzle or exploration games."
tags        = ["first-person", "3d", "starter"]
license     = "MIT"

[requires]
editor = ">=1.2.0, <2.0.0"

[substitute]
# Placeholders replaced during instantiation
"{{project_name}}" = "project.name"
"{{project_id}}"   = "project.id"
"{{author}}"       = "project.author"

[ignore]
paths = [".git/", "target/", "dist/"]
```

Instantiation: `rustforge new --template first-person my-game` unpacks `skeleton/`, runs substitution across text files, writes a fresh `rustforge-project.toml`, and does *not* carry any VCS state across.

### 3.2 First-party templates

```
rustforge/templates/
├── empty/             # project.toml + one empty scene
├── 3d-platformer/
├── 2d-sidescroller/
├── fps/
└── ui-sandbox/        # no scene; just a UI playground
```

These ship with the editor installer. The Welcome Window's "New Project" flow offers them before any third-party template. No outbound network required to start a project.

### 3.3 Third-party templates

The same Git-URL path that Phase 11 gave plugins:

```
$ rustforge new --template git+https://github.com/someone/spaceshooter-template.git my-shmup
```

The CLI clones to a cache, validates `template.toml`, instantiates. Users can also pin templates in a user config file so frequently used community templates show up in the Welcome Window automatically. There is no central registry. That is deliberate.

## 4. Documentation portal (mdBook)

Everything lives in one Git repository, one mdBook, versioned per editor release.

```
docs/
├── book.toml
├── theme/             # matches Phase 13's editor theme
└── src/
    ├── getting-started/
    │   ├── install.md
    │   ├── first-project.md
    │   ├── the-editor-tour.md
    │   └── faq.md
    ├── core-concepts/
    │   ├── scenes.md
    │   ├── assets-and-guids.md
    │   ├── components-and-reflection.md
    │   ├── play-mode.md
    │   └── build-pipeline.md
    ├── editor-reference/
    │   ├── panels/...       # one page per panel, auto-indexed
    │   ├── menus.md
    │   └── keybindings.md   # generated from ActionRegistry
    ├── scripting/
    │   ├── rust-scripts.md
    │   ├── wasm-scripts.md
    │   ├── lifecycle.md
    │   └── api-reference.md # generated from rustdoc
    ├── asset-pipeline/
    │   ├── textures.md
    │   ├── meshes.md
    │   ├── audio.md
    │   ├── materials.md
    │   └── packs.md         # references §6
    ├── plugin-dev/
    │   ├── getting-started.md
    │   ├── extension-points.md
    │   ├── capabilities.md
    │   ├── publishing.md    # references §5
    │   └── template.md      # references §7
    ├── platform-notes/
    │   ├── windows.md
    │   ├── macos.md
    │   ├── linux.md
    │   ├── web.md
    │   ├── mobile.md
    │   └── console.md       # public version; NDA bits live elsewhere
    └── migration/
        ├── 1.0-to-1.1.md
        ├── 1.1-to-1.2.md
        └── ...
```

### 4.1 Build and deploy

CI: `mdbook build` on every main merge, deploy to `docs.rustforge.dev/<version>/`, redirect `latest` at the newest stable release. Every editor build embeds its exact docs version; the F1 handler resolves `docs.rustforge.dev/<embedded-version>/…` so an older editor never gets docs for a feature it doesn't have.

### 4.2 Offline bundle

Every installer includes the full mdBook output for that version at `<install>/docs/`. If the network is down or the user is behind a firewall, `F1` falls back to the local copy. The Help menu exposes a "Open local docs" item that points at `file://<install>/docs/index.html`.

## 5. In-editor help

Phase 13 §8 stubbed context-sensitive `F1`. Phase 23 fills it.

### 5.1 Panel help

Every first-party panel declares a `docs_path: &'static str`. `F1` with a panel focused opens `docs/editor-reference/panels/<docs_path>.html`. Plugins (Phase 11) declare theirs in `plugin.toml`:

```toml
[docs]
book = "https://plugindocs.acme.io/brush-extras/"
```

If a plugin omits it, `F1` opens a generic "This plugin has no docs page" stub that links to the plugin's repo.

### 5.2 Type and field help

Phase 13 extended the reflection registry to carry `///` docstrings into `FieldInfo::doc`. Phase 23 adds one more: every reflected type carries `docs_path`, auto-generated from its type path:

```
rustforge_core::transform::Transform
  -> docs/scripting/api-reference.html#rustforge_core.transform.Transform
```

Hovering a field in any inspector and pressing F1 opens the right anchor. The docstrings render as tooltips in the inspector anyway — same data, two surfaces.

### 5.3 Prose for everything reflected

The rule: *every reflected public type in `rustforge-core`, `rustforge-render`, `rustforge-audio`, `rustforge-physics`, `rustforge-net`, `rustforge-ui` has a doc comment at least one sentence long.* CI fails on a missing docstring on any `pub` item marked `#[derive(Reflect)]`. This is the one place Phase 23 puts real teeth on prose.

## 6. Plugin directory — a JSON index, not a marketplace

The word "marketplace" implies a storefront; the word "directory" implies a phone book. We are building the phone book.

### 6.1 The index file

A single JSON file lives in `rustforge/plugin-directory` on GitHub. Community PRs add, update, or remove entries. No review queue beyond "is it spam" and "does the Git URL resolve." The core team maintains it; merges are a matter of hours, not weeks.

```json
{
  "schema_version": 1,
  "updated_at": "2026-04-16T00:00:00Z",
  "plugins": [
    {
      "id": "com.acme.brush-extras",
      "name": "Brush Extras",
      "description": "Erosion, ridge, and plateau brushes for terrain.",
      "authors": ["Acme"],
      "homepage": "https://github.com/acme/brush-extras",
      "git": "https://github.com/acme/brush-extras",
      "license": "MIT",
      "tags": ["terrain", "brushes"],
      "screenshots": [
        "https://raw.githubusercontent.com/acme/brush-extras/main/docs/gallery.png"
      ],
      "runtime": "wasm",
      "target_api_version": "1.2",
      "latest_version": "0.2.1"
    }
  ]
}
```

### 6.2 Editor plugin browser

A new tab in the Plugin Manager panel (Phase 11 §10):

```
┌─ Plugins — Browse ─────────────────────────────────────────────┐
│ [ search ]  [tag: all ▼]  [runtime: all ▼]  [sort: updated ▼]  │
├─────────────────────────────────────────────────────────────────┤
│ Brush Extras               0.2.1  wasm   MIT    [Install]       │
│   Erosion, ridge, and plateau brushes for terrain.              │
│                                                                  │
│ Scene Linter               0.1.0  rust   MIT    [Install]       │
│   Flags common authoring mistakes before commit.                │
│                                                                  │
│ Asset Organizer            0.4.0  wasm   Apache [Install]       │
│   Bulk-rename, tag, and move assets with undoable ops.          │
├─────────────────────────────────────────────────────────────────┤
│ Last updated: 4 hours ago    [Refresh]                          │
└─────────────────────────────────────────────────────────────────┘
```

"Install" routes to Phase 11's `rustforge plugins fetch` with the entry's `git` URL. No payment. No ratings in v1 — ratings invite review-bombing and moderation load we're not staffed for.

### 6.3 What the editor does NOT do

- Not host binaries. Clones from the plugin's own Git repo. If the repo goes away, the plugin goes away; that's the web being the web.
- Not verify code. The sandbox is WASM's (Phase 11 §5) or the user's judgment (rust-static plugins).
- Not enforce quality. The index is a phone book; quality is the author's problem.
- Not take a cut. There's no money flowing through it to take a cut of.

## 7. Asset-pack conventions

Artists ship asset packs today on itch.io and Gumroad in whatever layout they feel like. Phase 23 proposes a convention; it does not enforce it. A pack that follows the convention drops into a project with zero setup.

### 7.1 `pack.toml`

```toml
[pack]
id          = "com.example.stylized-foliage"
name        = "Stylized Foliage Pack"
version     = "1.0.0"
authors     = ["Example Studios"]
license     = "CC-BY-4.0"
description = "42 stylized trees, bushes, and grasses."

[contents]
textures = "textures/"
meshes   = "meshes/"
materials = "materials/"
prefabs  = "prefabs/"

[requires]
editor = ">=1.0.0"
```

### 7.2 `.gitattributes`

Phase 12 §8.2 shipped a baseline. Asset-pack authors get the same baseline plus:

```
# Pack metadata is text
pack.toml           text eol=lf
LICENSE             text eol=lf
README.md           text eol=lf

# Everything else is LFS-tracked binary
*.png  filter=lfs diff=lfs merge=lfs -text
*.ktx2 filter=lfs diff=lfs merge=lfs -text
*.gltf filter=lfs diff=lfs merge=lfs -text
*.glb  filter=lfs diff=lfs merge=lfs -text
```

### 7.3 Install

```
$ cd my-game/assets
$ git clone https://github.com/example/stylized-foliage.git packs/foliage
```

The Content Browser recognizes a `packs/<name>/pack.toml` layout, labels the root with the pack name, and shows license/version in its inspector. No magic beyond that; packs are just subdirectories with conventions.

## 8. `rustforge-plugin-template`

Authoring a plugin from scratch today means reading Phase 11 and assembling a `Cargo.toml`, `plugin.toml`, a `Plugin` impl, and the WASM or static-link boilerplate. That is a 30-minute hurdle for a 5-minute idea. Ship a `cargo generate` template.

```
$ cargo generate rustforge/rustforge-plugin-template --name my-plugin
```

Produces:

```
my-plugin/
├── Cargo.toml
├── plugin.toml
├── README.md
├── .gitignore
├── .github/
│   └── workflows/
│       └── ci.yml          # fmt, clippy, build, test
├── src/
│   ├── lib.rs              # Plugin trait impl, one sample panel, one command
│   └── panel.rs
├── tests/
│   └── load.rs             # headless plugin load smoke test
└── examples/
    └── standalone.rs
```

The template is parametric on `runtime` (`rust-static` or `wasm`). Choosing WASM adds `wasm32-unknown-unknown` to `rust-toolchain.toml` and a `cargo xtask build-wasm` script. The first commit builds green on CI.

This template is the second-highest-leverage deliverable in Phase 23, after the docs portal. Its existence converts "I might write a plugin someday" into "I just did."

## 9. Versioning

### 9.1 Editor

`rustforge-editor` follows strict semver from 1.0 onward.

- `MAJOR` — breaking user-visible change (project file format, plugin API). Signalled a release in advance.
- `MINOR` — new features, backward-compatible. Ships on the cadence in §10.
- `PATCH` — bug fixes, no new APIs, no project-format changes.

### 9.2 Plugin API

`rustforge-plugin-api` stays on `0.x` until the whole shape settles. Phase 11 §2.1 set the rules; Phase 23 restates them:

- `0.x.y` — patch is additive-only; plugins compiled against `0.x.y` load on `0.x.z` for any `z`.
- `0.x → 0.(x+1)` — minor may require a rebuild. Document breaking changes in `CHANGELOG.md` with a migration recipe.
- Target the 1.0 pin of the plugin API for no sooner than editor 2.0, so we get a full major cycle to iterate.

### 9.3 Schema versions

Project files, scene files, and prefs have their own schema versions independent of editor version. A minor editor release may bump a schema version; migration is always forward-only, lossless within a major, and tested with `0 → N` as well as `N-1 → N` (Phase 13 §15 risk).

## 10. Release cadence

- **Three months** between minor releases. Predictable; short enough that the backlog never gets stale; long enough to stabilize.
- **Patch releases as needed** — critical fixes ship within a week of discovery; non-critical fixes batch into the next minor.
- **LTS tier after 2.0.** Each major release designates one minor as LTS, maintained with security and critical-bug patches for 18 months past its successor's release. 1.x gets no LTS — 1.0 is young.
- **End-of-life policy** documented per minor. Users know when their version stops getting fixes.
- **Release notes** posted to docs, Discussions, and Discord simultaneously. No "soft launches."

The cadence is a policy document in `docs/migration/release-policy.md`, plus a calendar reminder on the core team's shared calendar. That is the whole enforcement mechanism.

## 11. Community channels

Four touchpoints, each with a clear purpose:

- **GitHub Issues** — bug reports, feature requests. Templates enforce the minimum info.
- **GitHub Discussions** — Q&A, show-and-tell, proposals. The canonical async venue.
- **Discord** — live help, informal chat. Best-effort, not official support.
- **RFC repository** (see §14) — design discussion for non-trivial changes.

The editor's **Help → Feedback** menu item opens a chooser:

```
Feedback
 ( ) I found a bug            → GitHub Issues with a template
 ( ) I have an idea           → GitHub Discussions "Ideas"
 ( ) I want to chat           → Discord invite
 ( ) I want to propose a big change → RFC repository
```

No in-editor forum, no in-editor issue tracker, no telemetry button disguised as feedback. The editor is not a browser; link out cleanly.

## 12. Accessibility statement & keyboard-only workflow

### 12.1 The statement

A one-page commitment in the docs portal, written in plain language. Cribbed structure:

- What we test (Phase 13 §12 accessibility smoke).
- What we don't yet support (full RTL; full screen-reader parity on every platform).
- How to report gaps (specific label, directly to a maintainer).
- What "done" looks like per release.

### 12.2 Keyboard-only workflow docs

A page under `editor-reference/keyboard-only.md` that walks a new project from "open editor" to "export build" using only the keyboard. This is both documentation and a spec: if a user following this page gets stuck because a feature is mouse-only, that's a bug. The Phase 13 integration test already exercises the core of it; this page expands it into the long tail.

## 13. Localization contribution flow

Phase 13 shipped Fluent and `en-US.ftl`. Phase 23 opens the door to other languages.

### 13.1 The flow

1. Translator forks the repo, copies `locales/en-US.ftl` to `locales/<lang>.ftl`.
2. Translates. Plurals live in `<lang>.toml`.
3. Opens a PR. CI validates: every key present in English exists in the translation (or is explicitly marked untranslated).
4. One core-team review for conformance; no review of quality. That is the translator's craft.

### 13.2 Style guide

`docs/community/localization.md`: what to translate (UI labels, help strings), what not to (type names that appear in code, log messages meant for developers), how to phrase errors (imperative, plain, no engine jargon), how to handle compound placeholders.

### 13.3 String freeze interaction

Phase 13 §15's CI rule — *changing an existing `en-US.ftl` key fails the build* — protects translators. Phase 23 reaffirms it and adds: a per-release "stale translation" report lists keys newer than each locale's last full-coverage commit. Translators see their work list; maintainers see coverage at a glance.

## 14. Lightweight governance

### 14.1 The model

Benevolent maintainer model. A small core team has commit rights; the project lead has a tiebreaker vote. This is not democratic in principle, but it is responsive in practice: decisions are made in public on RFCs and Discussions, and rollback is cheap. The alternative — committee governance before there's a community large enough to populate a committee — is theatre.

### 14.2 The documents

Four markdown files at the repo root, each under 500 lines:

- **`CODE_OF_CONDUCT.md`** — Contributor Covenant 2.1, unmodified, with a project-specific enforcement contact.
- **`CONTRIBUTING.md`** — how to report a bug, propose a change, submit a PR, run the test suite, the DCO sign-off requirement.
- **`GOVERNANCE.md`** — who the maintainers are, how decisions are made, how someone becomes a maintainer, what "stepping down" looks like.
- **`SECURITY.md`** — how to report a vulnerability (GitHub Security Advisories), expected response time, disclosure policy.

### 14.3 The RFC process

A separate repo, `rustforge/rfcs`. Mirrors the Rust RFC process, simplified:

1. Fork, copy `0000-template.md`, write the proposal.
2. Open a PR.
3. Community comments for a minimum of seven days (14 for anything touching plugin API, project format, or governance itself).
4. Core team either merges (accepted), closes (rejected with rationale), or postpones.
5. Accepted RFCs get a tracking issue in the main repo; implementation follows.

Not every change needs an RFC. Bug fixes, small features, docs — just a PR. RFCs exist for things where disagreement is likely and the fix-after-merge cost is high.

## 15. Telemetry transparency

Phase 13 §9 was explicit; Phase 23 makes it unmissable. Add to the docs portal:

- A `docs/community/privacy.md` page listing every outbound network call the editor makes (update check, docs fetch, samples index fetch, plugin directory fetch, crash report if opted in), with frequency and payload contents.
- A link to that page from the About dialog, the Welcome Window footer, the Preferences → Advanced page, and the Feedback chooser.

The commitment, restated in those words:

- **No usage telemetry.** Not opt-in, not opt-out, not shipped.
- **Crash reports are opt-in.** Off by default. Payload documented.
- **No account required.** Ever, for any editor feature. The plugin directory, samples, templates, docs — all anonymous.
- **No phone-home for license validation.** There's no license to validate.

If any of these change, the change is a breaking one, announced a release in advance, with an RFC.

## 16. Trademark / naming policy

"RustForge" is a name, an install, and (eventually) a logo. We want the community to describe their work honestly without making the core team's lawyers wince.

### 16.1 What's permitted without asking

- "Built with RustForge."
- "A RustForge plugin."
- "A RustForge game."
- "A RustForge asset pack."
- The unmodified logo used as a "built with" badge, subject to clear-space and minimum-size rules in the brand page.

### 16.2 What's not

- Naming a product "RustForge X" or "X for RustForge" in a way that implies official endorsement.
- Using the logo as the primary identity for a non-core product.
- Creating "RustForgeHub," "RustForge Pro," or similar names that suggest affiliation.

### 16.3 How to ask

One email address, published in `TRADEMARK.md`. Response within two weeks. The default answer for good-faith community projects is yes with a short clarifying note.

The point is not to police the word. The point is to keep it meaning something.

## 17. Scope boundaries — what's NOT in Phase 23

- ❌ **Paid marketplace with billing.** No Stripe integration, no escrow, no seller onboarding, no tax handling.
- ❌ **DRM.** The editor is open; anything it builds can be copied. We do not ship an anti-piracy layer for other people's content.
- ❌ **Editorial review / curation of community plugins.** The directory index is a phone book. Spam and malware get removed; quality is not judged.
- ❌ **Hosted cloud builds.** Export targets are local. A future phase may add self-hosted CI recipes; Phase 23 does not host them.
- ❌ **Usage telemetry.** Restated for emphasis. Not opt-in. Not shipped.
- ❌ **Mandatory accounts.** Nothing in the editor requires a login. Git clone works anonymously; the plugin directory is a JSON file; crash reports are opt-in and anonymous.
- ❌ **First-party storefront for paid assets.** We are not itch.io. Creators sell on the platform of their choice; we link to it.
- ❌ **In-editor discussion forum.** GitHub Discussions exists; we don't need to rebuild it badly.
- ❌ **Binary-hosted plugin registry.** The index points at Git repositories. We don't host the bytes.
- ❌ **Auto-review bot for plugin submissions.** A human merges the index PR; the load is light at current volumes, and automating it would make bad-faith submissions easier, not harder.

## 18. Risks & gotchas

- **Samples bitrotting.** A sample written for editor 1.2 stops working on 1.5. Pin every sample to an editor version in its `rustforge-project.toml`; CI re-opens every sample on every editor release and flags breakage. Samples that fail are fixed before the release ships.
- **Plugin directory spam.** An easy PR-merge policy invites crypto-bro spam. Mitigate with a simple filter: no commercial upsell links, no affiliate redirects, no templates copy-pasted from another plugin's listing. Bad-faith submitters get banned from the index repo; the rest of the repo's activity is unaffected.
- **Docs drift from code.** A feature lands, docs lag. Enforce in CI: any PR adding a `pub` reflected type must add a docstring; any PR adding a panel must update its `docs_path` target. Not perfect, but raises the floor.
- **Template substitution misuse.** Templates that substitute arbitrary strings into code files can produce broken builds if the substitution touches an identifier. Restrict `substitute` to `.toml`, `.md`, and `.txt` by default; require an explicit opt-in for other extensions.
- **`cargo generate` bitrot.** The upstream tool changes its templating syntax; our template breaks. Pin it in the README; test the template on every release with a fresh cargo install.
- **Offline docs staleness.** Users on old installers hit docs that don't match their build. The F1 handler always appends the exact editor version to the URL; the embedded offline copy is tagged the same way. Never silently fall through to "latest."
- **Plugin browser fetches at startup.** Fetching the directory on every launch both wastes a request and fails on offline machines. Cache for 24 h, refresh on explicit user action, fail open (show the cached copy with a staleness banner).
- **Governance doc becoming a flypaper.** A well-lawyered CoC is not a shield; it's a process. Designate an enforcement contact and actually answer reports within a week. Anything less is worse than not writing it.
- **Trademark overreach.** Sending C&Ds to hobbyists burns goodwill and makes news. The default posture is permissive; TRADEMARK.md names the few cases where we push back, and the maintainers hold the line on that shortlist.
- **Localization review bottleneck.** A surge of translation PRs could overwhelm core reviewers. Delegate per-language review to trusted community members; record them in `GOVERNANCE.md` so the buck stops somewhere visible.
- **RFC process becoming the new "issues."** Users file RFCs for bug fixes. Document clearly what belongs in an RFC and what's a plain issue; close RFC PRs that don't fit with a redirect.
- **mdBook limits.** Large tables, complex anchors, and search across versions push mdBook's envelope. Accept the tradeoff: an imperfect book in one tool beats a perfect split across three.
- **Feedback menu misused.** The chooser must not become a venting surface. Each destination enforces its own template; the editor just routes.
- **Samples attracting contentious changes.** Someone PRs "add shader X" to a sample; the sample gets heavier. Reviewers hold the line: samples are minimal teachers, not kitchen sinks. The bar is *"does this help a new user learn one thing?"*

## 19. Exit criteria

Phase 23 is done when all of these are true:

- [ ] mdBook docs portal ships with all eight top-level sections populated, deployed at `docs.rustforge.dev/<version>/`.
- [ ] Offline docs bundle ships inside every installer; `F1` falls back to it when the network is down.
- [ ] Every `pub` reflected type in first-party crates has a doc comment; CI fails on missing docstrings.
- [ ] All seven initial samples build, open, and run on a clean install of the current editor.
- [ ] Welcome Window lists samples with working "Clone and open" buttons; offline fallback works.
- [ ] Five first-party templates (Empty, 3D Platformer, 2D Sidescroller, FPS, UI Sandbox) instantiate via `rustforge new --template <id>` and via the Welcome Window.
- [ ] Third-party templates install via `rustforge new --template git+https://...`.
- [ ] Plugin directory index repo exists; its JSON schema is documented; at least ten community-contributed entries are merged.
- [ ] Plugin browser in the editor lists, filters, and installs plugins from the directory; installation routes through Phase 11's fetcher.
- [ ] `rustforge-plugin-template` builds green from `cargo generate` for both `rust-static` and `wasm` runtimes; its CI workflow passes on first commit.
- [ ] `pack.toml` spec is documented; Content Browser renders pack metadata when a `packs/<name>/pack.toml` is present.
- [ ] Versioning and release cadence are written down and linked from the repo README.
- [ ] LTS policy is published; the date of 1.x EOL and the 2.0 LTS designation process are documented.
- [ ] Help → Feedback chooser routes to GitHub Issues, Discussions, Discord, and the RFC repo.
- [ ] Accessibility statement is published; keyboard-only workflow page is written; the Phase 13 accessibility smoke test still passes with those workflows exercised.
- [ ] Localization contribution flow is documented; CI validates key parity for every shipped translation; at least one non-English locale has landed via the flow.
- [ ] `CODE_OF_CONDUCT.md`, `CONTRIBUTING.md`, `GOVERNANCE.md`, `SECURITY.md` are present at the repo root; an enforcement contact is named.
- [ ] RFC repository exists with a template, and at least one RFC has gone through the full accept/reject cycle.
- [ ] Privacy page lists every outbound network call and is linked from About, Welcome, Preferences → Advanced, and Feedback.
- [ ] `TRADEMARK.md` is published; a brand page documents permitted uses and the contact address.
- [ ] `rustforge-core` still builds and runs without the `editor` feature; none of Phase 23's additions introduce a dependency on network access at runtime in shipped games.

---

## 20. Closing the 23-phase arc

Phases 1–13 built the editor: a workspace that compiled, a scene that rendered, a play mode that didn't lie, a command stack that undid, a profiler that reported honest numbers, a plugin API that didn't crash the host, a Git integration that didn't lose `.meta` files, a preferences system that migrated, a welcome window that loaded, a crash report that stayed opt-in. That was 1.0. It was also the point at which the design stopped being sequential; the rest is a portfolio.

Phases 14–22 were that portfolio, post-1.0, on separate tracks: a renderer that learned about clustered lighting and a reasonable shadow pipeline, physics that handled contact manifolds without jitter, audio with a mixing graph and spatialization that didn't embarrass itself, networking with rollback and lag compensation, an animation retargeting path that survived an IK pass, a console backend with the NDA bits in a separate crate, a mobile target that shipped on a real device, and a web export that loaded scenes worth loading. Each of those phases could have been ten. None of them needed to be — the core team held the line on scope and shipped capability instead of features.

Phase 23 is the other kind of work. It builds no new engine subsystem. What it adds is the scaffolding that lets people outside the core team participate: samples to copy, templates to start from, docs to look up, plugins to install, a contribution flow to follow, a governance model to push back against. After Phase 23, the *engine* is complete enough that the project's bottleneck is no longer "can the team ship the next subsystem" — it is "can the community build on top of what's shipped, and can we not get in their way."

What remains, genuinely, is a list — not a roadmap, because the ordering is the community's and not ours:

- **Phase 24 — Animation graph.** A node-graph authoring tool on top of the retargeting pipeline; blend trees, state machines, IK chains composed visually. The one deferred piece of Phase 17.
- **Destruction and cloth.** Voronoi fracture, skinned cloth with a reasonable constraint solver, debris with LOD. Physics-adjacent, not physics-core.
- **AI.** Navigation meshes, behavior trees or goal-oriented action planning, utility AI. An authoring surface for each. None of this is hard; all of it is work.
- **Sky and weather.** Atmospheric scattering, cloud volumetrics, time-of-day, weather-driven particle systems wired to gameplay events.
- **XR.** Head-mounted display rendering pipeline, hand tracking, controller input abstraction. The kind of thing a small team does once and then maintains forever.
- **Team-scale build infrastructure.** Self-hosted CI recipes, shared asset-import cache servers, incremental LFS snapshots, a distributed compile cache. For studios past ten people the current tooling is thin.

None of those belong in another phased document. The phase model was how we bootstrapped; what we have now is a project, and projects evolve by pull requests and RFCs.

The philosophy that carried the 23 phases is the philosophy that should carry the next 23 if they come: **Rust-first**, because memory safety and a real type system pay compounding dividends in a codebase this size. **Git-native**, because version control is not a feature to bolt on — it is the substrate of how teams work, and an editor that hides it is an editor that fights its users. **Capability-sandboxed**, because a plugin system without capabilities is a malware distribution system, and a sandbox without capabilities is a toy. **Open-ecosystem**, because the most valuable thing a game engine can be is legible — to new users, to plugin authors, to translators, to the person five years from now who has to fix a bug in a part of the codebase we never expected to touch again.

Those four are not slogans; they are the constraints that every phase had to satisfy and that every future phase should be held to. An engine that gives up any of them loses something that's very hard to recover. An engine that keeps all four gets the benefit every open project dreams of: it stops being a product and becomes a commons.

That is what Phase 23 is finally for. Not the docs portal, not the plugin browser, not the governance markdown files — those are artifacts. The phase is for the turn from *we build it* to *we tend it*. From here, the engine belongs to whoever shows up to use it.
