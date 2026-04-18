# Phase 50 — Stability, Release Ops & Maintenance Mode

Forty-nine phases ago, Phase 1 sketched a workspace and a feature-gate wall. Phase 13 turned the sketch into a 1.0. Phase 23 turned the 1.0 into a platform. Phases 24–45 grew the platform until it could plausibly ship a AAA-shaped game; Phases 46–49 closed the gaps a 2.0 series wanted closed and named the ones it didn't. Phase 46 looked back over the arc and reported what had compounded; Phases 47–49 finished the design surface for what a 3.0 candidate would carry. This document — Phase 50 — is the **last phase of the series**. It does not introduce a new subsystem. It introduces the *rules a maintained subsystem has to live under* once design is done.

Phase 46 was the backward-looking retrospective: forty-five phases of decisions audited, scored, and sealed. Phase 50 is its forward-looking counterpart. It says, plainly: design ends here; operations begin here. Every prior phase asked "what should exist?" Phase 50 asks "how does what exists persist?" The two questions belong to different disciplines, and conflating them is how good design series quietly become bad maintained projects.

The shape of this phase is operational. Versioning rules, deprecation timelines, release checklists, security disclosure clocks, governance triggers, and the small handful of refusals that prevent the project from drifting into a different kind of project than the one the previous forty-nine phases described. None of this is glamorous. All of it is the difference between a forty-nine-phase blueprint and a forty-nine-phase blueprint that anyone is still maintaining in five years.

## Why this is the last phase

A design series is finite by construction. There are only so many architectural questions a single arc can answer before the answers start to depend on a real codebase and a real community rather than on the arc's own internal consistency. Phases 1–45 answered the architectural questions. Phase 46 audited them. Phases 47–49 closed the design surface for the candidate 3.0 features that could be specified without a community in the loop. Beyond Phase 50, every further question — should hair simulation grow a tessellation pass? does the narrative stack need a scripting fast-path? — is a question whose right answer requires data the series cannot generate by sitting still.

Phase 50 is the last phase because it is the one that makes the previous forty-nine survivable in the wild. Without it, the design series is a snapshot: brilliant, complete, and immediately stale the moment the first PR lands. With it, the series has a contract for how it ages — what it promises to keep, what it allows itself to break, how it tells the difference, and who decides. That contract is the entire job of this document, and once it is written, the arc has nothing left to author. Subsequent design must come from the project's lived experience, not from the series' imagination.

## 1. Deprecation policy

Every API marked stable under Phase 23's semver discipline carries a deprecation contract. Removal is a three-stage process spread across at least one minor and one major release. The stages are *announce*, *warn*, *remove*, and the project never skips one.

**Stage 1 — announce.** At a minor release, the API is *soft-deprecated*. Documentation grows a "Deprecated in X.Y" banner pointing at the replacement. The compiler emits an `#[deprecated]` attribute with `since = "X.Y"` and a `note` that names the replacement. The lint level is `warn` but suppressible. No behavior changes. Minimum residency in this stage: one minor release.

**Stage 2 — warn.** At the next minor release (earliest), the lint becomes loud — the `note` is rephrased to "will be removed in X+1.0" and the docs page acquires a tombstone block at the top with the migration recipe. Behavior still does not change. Tooling — the editor's script panel, `rustforge check`, the plugin loader — surfaces the warning in its own UI, not just in `cargo build` output. Minimum residency: one major release.

**Stage 3 — remove.** At a major release, the API is deleted. A *tombstone stub* (see §10) replaces it for one additional minor release: the function still exists with the same signature but its body is `compile_error!` or, for non-callable items like trait methods, `unimplemented!()` with the deprecation note. The stub itself is removed at the next minor release after the major.

The blast radius of this policy is bounded by Phase 23's stability tiers. Items marked `#[unstable]` follow none of it — they may break in any minor. Items marked `#[doc(hidden)]` carry no deprecation contract at all. Items in *internal* crates (`rustforge-internal-*`) are not API. The contract applies to the public surface of `rustforge-core`, `rustforge-editor-api`, `rustforge-plugin-api`, and the asset format families (`.ron` scene schema, `.meta` sidecar, `.rui`).

A deprecation that violates the timeline — removed in the same major it was announced, for example — is a stability bug and is reverted. The bar for an emergency exception is "the API is a security vulnerability" and the path is the security disclosure process in §7, not a deprecation shortcut.

## 2. LTS branches

Long-term support is offered, conservatively, on a subset of major-version releases. Every `2.x` and later major version is a *candidate* LTS. The decision is made at the point of release, not at the point of design, and turns on community-uptake signals that can only be measured after the version ships. Signals tracked: download counts on the rustup-like channel; plugin-directory adoption; volume of issues filed against the version in its first 90 days; explicit endorsements from studios shipping on the version. None of these is a hard threshold. The benevolent maintainer (later, the maintainer committee — §14) makes the call within 90 days of release.

An LTS version receives:

- **Security patches** for the full LTS window.
- **Critical bug fixes** — crashes, data loss, build-breaking regressions — for the full LTS window.
- **No** new features, performance improvements that are not regression fixes, or API additions.

The LTS window is **24 months** from the LTS declaration date. At month 18 the project announces the end-of-life date publicly and points users at the migration path. At month 24, security patches stop. The branch remains in the repository as `release-X.Y` indefinitely; nothing is deleted, but nothing further is merged either.

Non-LTS versions get current-minor-only patches. If `2.4` ships and `2.3` is not LTS, `2.3` gets no further patches; users on `2.3` are expected to take `2.4`. The series accepts that this is a stronger upgrade pressure than some users will want, and trades it deliberately for a maintenance burden the project can sustain.

## 3. Version skew

Three versioned surfaces interact: the **editor binary** (`rustforge-editor`), the **game runtime** (`rustforge-core`), and the **plugin API** (`rustforge-plugin-api`). Each has its own semver number; they are released together but compatibility is governed independently.

| Source version | Target version | Direction | Behavior |
|---|---|---|---|
| Editor X.Y | Core X.Y' (Y' < Y, same major) | Open older project | Open with banner; warn on save (downgrade prompt) |
| Editor X.Y | Core X.Y' (Y' > Y, same major) | Open newer project | Refuse with actionable error: "this project was saved by a newer editor; upgrade to X.Y' or newer" |
| Editor X.Y | Core (X-1).Z | Open older-major project | Open via Phase 45 migration tool only; never silently |
| Editor X.Y | Plugin API X.Y' (Y' < Y) | Load older plugin | Load with shim layer; warn in plugin browser |
| Editor X.Y | Plugin API X.Y' (Y' > Y) | Load newer plugin | Refuse to load; show "needs editor X.Y' or newer" in plugin browser |
| Editor X.Y | Plugin API (X-1).Z | Load older-major plugin | Refuse to load; show "needs rebuild against X" |
| Game runtime X.Y | Plugin API X.Y' (Y' ≤ Y, same major) | Loaded mod | Load via shim; mod's manifest declares minimum API |
| Game runtime X.Y | Plugin API (X-1).Z | Loaded mod | Refuse; display in mod manager with rebuild link |

The asymmetries are deliberate. Editors are always at least as new as the projects they open in normal use, so the older-project case is the path the editor optimizes for. Plugins and mods are distributed independently and pinned to API versions in their manifests; the loader's job is to refuse incompatibility *at load time, with an actionable message*, not to attempt heroic compatibility shims.

The shim layer for older plugin APIs is one major version wide. A `2.x` editor loads `1.x` plugins via shim; a `3.x` editor does not load `1.x` plugins at all. This caps the shim maintenance burden at "the previous major" and removes any temptation to grow a perpetual compatibility museum.

## 4. Backport policy

Backports to LTS branches are governed by a decision tree, not by case-by-case argument. The tree:

```
Is the change a security fix (CVE-eligible or embargoed)?
  YES -> backport (always).
  NO  -> continue.

Is the change a fix for a crash, hang, or data-loss bug?
  YES -> backport (default; maintainer may decline only with written justification).
  NO  -> continue.

Is the change a fix for a build-breaking regression
  vs the LTS branch's previous patch release?
  YES -> backport.
  NO  -> continue.

Is the change a feature, an API addition, a performance
  improvement that is not a regression fix, or a refactor?
  YES -> do not backport. Tell the user to upgrade.
```

The tree is enforced. PRs against `release-X.Y` branches that do not fit the tree are closed with a pointer to the trunk PR, even if they are good changes. This is the discipline that keeps LTS branches actually long-term: the second a feature lands on an LTS, the LTS becomes a fork, and forks compete with trunk for maintainer attention until one of them dies.

A backport PR carries the original trunk PR number in its title (`[backport #4521]`) and links the original commit. The CI matrix for an LTS branch runs the same gates as trunk (see §5) plus an additional gate that asserts the backported diff is a strict subset of the trunk diff — no opportunistic edits along the way.

## 5. Regression test strategy

Forty-nine phases produced a lot of code; the test surface that defends it is correspondingly broad. The strategy is layered. Each layer has a clear owner and a clear failure mode.

**Unit tests, per-crate.** Owned by the contributor of the crate. Phase-level specs do not enumerate them. The release gate is "all crate test suites pass on the supported targets." Crates without tests do not ship in stable.

**Integration tests in CI.** A suite of representative project lifecycles — open, edit, save, play, stop, close — across the seven sample projects from Phase 23. Runs on every PR on Linux, Windows, and macOS. Web build runs the same suite headless via `wasm-bindgen-test`. Each sample also exercises a domain — networking sample exercises Phase 14 + 34, particles exercises Phase 18 + 21 — so a regression in a deep system surfaces as a sample-suite failure.

**Performance regression tests.** Phase 42's trace captures are the substrate. A baseline trace per representative scenario (editor idle, 10k-entity scene, cold start, 100-actor world-partition stream) is stored in the repo as a small reference file. CI re-captures on every PR and compares: frame time, memory high-water, build time, cold-start time. Regressions over **3%** are flagged on the PR; over **10%** block merge unless the PR carries an explicit performance-budget waiver in the description and a maintainer's review of the waiver. Dashboards live on the public status site (§16).

**Determinism regression.** The Phase 14 (networking) and Phase 33 (replay) deterministic scenarios are replayed on every release branch and asserted bit-for-bit identical against a stored golden replay. Drift is a release blocker. The drift can come from compiler updates, dependency updates, math-library changes, or platform-FP differences; all of those are root-causes the series committed to fixing rather than tolerating.

**Golden-image rendering tests.** The Phase 21 / 36 / 47 rendering pipelines have a per-pipeline gallery of fixed scenes captured at fixed resolutions. CI re-renders and diffs against stored PNGs at a small perceptual tolerance (FLIP or SSIM thresholded; not exact pixel match, because GPU drivers move). Drift over the threshold blocks merge; drift below is logged. A separate weekly job re-renders against the latest GPU drivers on the supported hardware matrix and tracks whether the threshold needs to widen — driver updates that change rendering are an upstream signal we want to record.

The five layers are independent. A change can pass unit tests and break integration; pass integration and break perf; pass perf and break determinism; pass determinism and shift goldens. The release gate requires all five to pass on the release branch.

## 6. Breaking-change RFC process

Phase 23 introduced an RFC process as a light-touch design-discussion mechanism. Phase 50 gives it teeth for one specific class of change: anything that breaks a stability contract from §1, §3, or the asset format families. Such changes require an accepted RFC before any code change lands; PRs that propose them without an RFC are closed with a pointer to the template.

The RFC template lives at `rfcs/0000-template.md`:

```markdown
# RFC: <title>

- Status: Draft | Active Comment | Accepted | Rejected | Withdrawn
- Champion: @<github-handle>
- Created: YYYY-MM-DD
- Comment-period-ends: YYYY-MM-DD (minimum 14 days after Active Comment)
- Stability surfaces affected: <list of stability-contract surfaces>

## Motivation
Why does the current contract need to change? Who is hurt by it today?
Be concrete. Cite issues, forum threads, plugin breakage reports.

## Detailed change
What exactly changes? API signatures, asset-format fields, version-skew
matrix entries — show the before and after.

## Alternatives considered
At least two. "Do nothing" is always one of them.

## Migration path
For users on the current contract: what they do, when they do it, what
breaks if they do not. Tie to the deprecation policy in §1.

## Deprecation schedule
Announce-version, warn-version, remove-version. Earliest each can be.

## Compatibility shim plan
For the duration of warn -> remove, what shim exists, what it costs to
maintain, and who removes it.

## Drawbacks
Honest list. If the only drawback is "some users will be annoyed,"
say so.

## Unresolved questions
What this RFC defers to follow-up RFCs.
```

The process: open the RFC as a PR against `rfcs/`, mark Status Draft, iterate with reviewers in the PR thread until the champion calls "ready for comment." Status flips to Active Comment, the comment-period clock starts, and the RFC is announced on the community channels (Phase 23). Minimum 14 days. After comment closes, the maintainer committee (§14) discusses in a public thread and merges as Accepted, Rejected, or returns to Draft. Acceptance authorizes implementation; implementation PRs cite the accepted RFC number.

The bar for an emergency RFC bypass is the same as for the deprecation bypass: only security. A breaking change that ships without an accepted RFC and is not a security fix is a stability bug; the change is reverted at the next patch.

## 7. Security vulnerability response

Vulnerability reports come in privately, are triaged privately, and are disclosed publicly on a clock. The path:

1. **Intake.** A `SECURITY.md` at the repo root names a private contact channel — encrypted email to a small distribution list of the maintainer committee, with a published PGP key. GitHub Security Advisories' private reporting feature is mirrored to the same list. *No security report is opened as a public issue.*
2. **Acknowledge.** Triage acknowledgement to the reporter within **72 hours**. The acknowledgement does not yet confirm exploitability; it confirms the report was received and a triage owner is assigned.
3. **Triage.** Within **7 days**, the report is classified: confirmed, declined-not-a-vulnerability, or needs-more-info. Confirmed reports get a CVSS score and a CVE request via GitHub's CVE Numbering Authority.
4. **Embargo.** A confirmed vulnerability is embargoed from public disclosure until a fix is ready or **90 days** elapse, whichever comes first. The 90-day clock is not negotiable. If the fix is not ready at day 90, the vulnerability is disclosed anyway — partial mitigations published, full advisory still issued. The series will not protect a vendor (itself) at the cost of leaving users uninformed.
5. **Coordinate.** Where the vulnerability touches an upstream dependency, coordinate with that project's disclosure timeline. Where it touches a downstream — a popular plugin built on the affected API — give plugin authors lead time via the dev-channel pre-release (see §11), but never beyond the 90-day cap.
6. **Patch and advisory.** Patch is prepared on a private branch, reviewed under embargo, and released simultaneously with the security advisory. Advisory is published on GitHub Security Advisories, mirrored to the project's RSS/Atom feed (§16), and announced on the community channels. CVE is assigned and listed.
7. **Post-mortem.** Within 30 days of advisory publication, a post-mortem is published describing root cause, detection, and any process changes. Post-mortems are blameless and public.

The explicit non-policy: **the project will never silently patch a security issue without an accompanying advisory.** A user who looks at the diff between two patch releases must be able to see, in the changelog or the linked advisories, why every security-affecting line moved. Silent patching is a betrayal of the user's ability to assess their own risk; the project does not do it.

## 8. Performance budgets as contracts

Earlier phases set performance budgets — Phase 13's 2 ms editor idle, Phase 1/9's 16 ms 10k-entity scene, Phase 13's 3 s cold start, Phase 22's web-build payload caps, Phase 42's trace-overhead bound. Phase 50 promotes these from aspirations to *enforced CI gates*. The Phase 5 §5 perf-test harness measures each on every PR; regressions over the budget block merge.

The list of budgets carried forward as contracts:

- **Editor idle frame**: < 2 ms CPU on a reference workstation (Phase 13).
- **10k-entity scene tick**: < 16 ms total frame on the same reference workstation (Phase 1, Phase 9).
- **Editor cold start to project-loaded**: < 3 s with a warm disk cache, < 8 s cold (Phase 13).
- **Headless `rustforge check`**: < 1 s for a single-scene project.
- **Web build payload**: < 8 MB compressed for the first-person-walker sample (Phase 22).
- **Plugin load**: < 250 ms per plugin from disk to capability-grant (Phase 11).
- **Snapshot/restore round-trip in PIE**: < 100 ms for the topdown-shooter sample (Phase 7).
- **Replay determinism**: bit-exact across the supported target matrix (Phase 14, Phase 33).

A regression past the budget requires either an RFC that revises the budget with motivation, or a revert of the offending change. There is no "we'll fix it next release" path for a budget regression; budgets exist precisely because the series learned that *next release* is where performance regressions go to die.

## 9. Release engineering operationalized

Every release follows the same checklist. The checklist is a file in the repo; cutting a release is the act of marking each item done.

```
[ ] Cut release-X.Y branch from main at the agreed commit.
[ ] CHANGELOG.md updated, sections grouped by Added / Changed / Deprecated /
    Removed / Fixed / Security, each entry linking the PR.
[ ] Migration guide drafted for any §1 deprecations advancing a stage.
[ ] Migration guide drafted for any breaking changes (must cite accepted RFC).
[ ] Plugin-API version computed; bumped if any §3 surface changed.
[ ] Asset-format version computed; bumped if any schema field changed.
[ ] Docs site (Phase 23 mdBook) rebuilt against the release branch; F1
    targets verified to resolve.
[ ] All seven sample projects (Phase 23) re-tested: open, play, stop, close.
[ ] Integration test suite green on Linux/Windows/macOS/web.
[ ] Performance regression dashboard green vs previous release.
[ ] Determinism replays green on the target matrix.
[ ] Golden-image rendering tests green.
[ ] Installers built and signed for all platforms (Phase 9, Phase 22).
[ ] LTS decision recorded (if a major release, or if 90 days post-major).
[ ] Security advisories for the cycle published, CVEs cross-linked.
[ ] Status page (§16) updated with the new version, LTS status, plugin-API
    version, EOL dates of any retiring versions.
[ ] Announcement posted to the community channels (Phase 23).
[ ] Release tag pushed; release artifact published to the rustup-like
    channel; checksums published.
```

Release automation runs every step except the human ones (LTS decision, announcement). The automation is GitHub Actions or an equivalent self-hosted runner setup; the project does not pay for a hosted CI service (see non-goals). A failing checklist item is a release blocker, not a footnote.

## 10. Deprecation tombstones

When a deprecated API is removed at a major release (§1 stage 3), it does not vanish. A *tombstone stub* persists for one additional minor release. The stub keeps the same path and signature, compiles, and produces a clear error at use:

```rust
#[deprecated(
    since = "3.0.0",
    note = "removed in 3.0; use `rustforge_core::scene::SceneId::new_v2` \
            instead. See migration guide M-2025-04."
)]
pub fn old_scene_id_constructor(/* ... */) -> ! {
    compile_error!(
        "rustforge_core::scene::SceneId::new was removed in 3.0. \
         Use SceneId::new_v2. See https://docs.rustforge.dev/migration/M-2025-04"
    )
}
```

For traits and types where `compile_error!` is not available at the use site, the body is `unimplemented!()` with the same note in the panic message. The intent is identical: the user attempting a blind upgrade does not get a confusing "function not found" error; they get a *named* error pointing at the migration guide.

Tombstones cost almost nothing to maintain — they are not exercised by tests, they are not in the runtime path, they are documentation expressed as a compile-time pointer. They are removed at the next minor release (`3.1`) once the upgrade has had one minor cycle to settle.

## 11. Plugin ecosystem maintenance

The plugin directory from Phase 23 is the project's most visible third-party surface, and the most fragile under version skew. Phase 50 adds operational rules.

**Pre-release notification.** Whenever the plugin API version is about to bump (any minor release that touches `rustforge-plugin-api`), a *dev-channel pre-release* of the editor and core is published at least **30 days** before the stable release. Plugin authors get the lead time to rebuild against the new API and publish updated versions. The dev channel is opt-in; no user is forced to it.

**Needs-rebuild badges.** When the plugin API bumps, the directory's index file (Phase 23) auto-generates a "needs-rebuild" badge against every plugin whose declared API version is now older than the current stable. The badge is computed by the index's CI from the manifest's `min-api-version` field; plugin authors do not have to do anything to *acquire* the badge. They have to publish a new version to *clear* it.

**Rotation.** A plugin that has not had a release for **3 major versions** of the engine is moved to an "Inactive" section of the directory. It remains installable — nothing is deleted — but it is hidden from default search and the install dialog displays an "unmaintained" notice. The 3-major window is roughly 4–6 years given the cadence; abandonment by then is fair to call abandonment.

**Reclaim path.** A plugin moved to Inactive can be reclaimed by its original author with a single release, or transferred to a new maintainer through a PR against the index file that includes evidence of permission (issue thread, fork commit, license note). The series does not mediate ownership disputes beyond requiring a paper trail.

**Audit on bump.** When the plugin API bumps in a way that affects capability grants — new permission, expanded scope, broader filesystem access — the audit policy in §12 applies to plugins re-published against the new API. The directory CI flags any new capability not present in the previous version; that flag triggers re-review by a maintainer before the new version is indexed.

## 12. Dependency policy

The workspace runs `cargo deny` on every PR. The policy file (`deny.toml`) at the repo root encodes:

- **Licenses.** Allowed: MIT, Apache-2.0, MIT-0, BSD-3-Clause, BSD-2-Clause, ISC, Zlib, MPL-2.0 (with carve-out — see §15), Unicode-DFS-2016, CC0-1.0. Denied: GPL-* (any flavor), AGPL-*, LGPL-* (in core crates; permitted in build-time-only dev-deps), proprietary or unspecified. A denied license in a transitive dep blocks merge until either the dep is replaced or the dep's license is upstream-corrected.
- **Advisories.** `cargo audit` runs alongside `cargo deny`; any unfixed advisory in the dependency tree blocks merge. Exceptions require a maintainer's written justification in the PR.
- **Source registries.** Only `crates.io` and explicitly listed Git sources. Vendored sources live in `third_party/` and carry a `LICENSE.upstream` and a `RELEASE.note` documenting the vendoring rationale.
- **Bans.** Dependencies known to have been abandoned, malicious, or relicensed away from compatibility are banned by name. The list is small and curated.
- **Sensitive crates.** `rustforge-plugin-host`, `rustforge-net`, `rustforge-cooker`, and any crate touching cryptography, network, or ML weights triggers an extra **manual review** of new direct deps. The reviewer signs off in the PR description; the CI lint asserts the sign-off line is present when the crate's `Cargo.toml` changed.

A new dependency in a sensitive crate without sign-off is a release blocker. Removing a dep is always cheaper than auditing one.

## 13. Documentation currency

Docs are not a post-release task. Every PR that changes a public API, an asset format, a workflow, or a stability contract carries a documentation delta in the same PR. The CI lint enforces a heuristic: PRs touching `crates/*/src/lib.rs` or `crates/*/src/api/*` must also touch `docs/` or include a `[no-doc-change]` line in the description with a justification. The justification is reviewed; "I'll do it later" is not accepted.

Stale docs are a release blocker. A release that ships with a docs delta missing — a renamed function, a removed panel, a changed default — is a documentation bug, and the bug is fixed in a patch within the cycle. The Phase 23 mdBook portal rebuilds against the release branch as part of the §9 release checklist; broken anchors, dead F1 links, and orphan pages fail the build.

The docs site is versioned. `docs.rustforge.dev/2.4/` and `docs.rustforge.dev/3.0/` coexist; LTS branches keep their docs alive for the LTS window. The default landing page lists the supported versions and their EOL dates.

## 14. Community governance maturity

Phase 23 named a benevolent-maintainer governance model and noted that it would mature. Phase 50 specifies the maturation trigger and the resulting structure.

**Trigger.** A maintainer committee is convened when **either** of these is true, whichever first:

- The project has reached **2.0 release**, *or*
- **30+ contributors** have had at least one PR merged in the **trailing 12 months**.

The trigger is objective on purpose. The benevolent maintainer does not get to defer indefinitely; the community does not get to demand the committee before the project is mature enough to populate one.

**Composition.** Five seats. The benevolent maintainer holds one ex-officio seat. The other four are elected by the contributor pool — anyone with at least one merged PR in the trailing 18 months may vote and stand. Terms are 18 months, staggered so that two seats turn over every 9 months, preserving institutional memory. No seat is a paid position.

**Authority.** The committee has authority over:

- RFC approval (§6) — by simple majority of those voting; quorum 3 of 5.
- Release cuts (§9) — the committee approves the LTS decision; the maintainer team executes the cut.
- Security disclosure coordination (§7) — the committee is the embargo distribution list.
- Committee additions and process changes (this document) — by supermajority (4 of 5).

**Veto.** The benevolent maintainer retains a veto on committee decisions and a tie-break on split votes. The veto is recorded publicly with rationale; an over-used veto is itself a governance signal that the committee should respond to. The veto exists so that bad collective decisions can be stopped, not so that the committee can be ignored.

**Transparency.** Committee discussions on RFCs and releases happen in public threads. Security discussions happen on the private channel; minutes are published to the public after embargo lifts.

## 15. Succession planning

The project does not depend on the continued participation of any single person. The succession plan is documented because if it is not documented, it does not exist.

**If the benevolent maintainer steps away**, voluntarily and with notice: the maintainer announces the departure with a minimum 90-day window. Within the window, the committee elects an *interim steward* from its seated members, by majority. The interim steward holds the BM's powers (veto, tie-break) for a transitional period of 6 months. During the transition, the committee organizes an open call for benevolent-maintainer candidates from the contributor pool; candidates submit a short statement and a track record. At the end of 6 months, the contributor pool elects a new BM by ranked-choice vote. The interim steward may stand.

**If the benevolent maintainer is unreachable** without notice (the bus-accident path): the committee may declare interim stewardship by supermajority (4 of 5) after **30 days** of unreachability with documented attempts to make contact. The 6-month transition then proceeds as above.

**If the committee itself dissolves** — fewer than 3 seated members for more than 60 days — the benevolent maintainer convenes a recovery election, in which seated members and the most recent 10 contributors form an interim electorate to fill the empty seats.

The intent of the plan is that the project is at most one bus accident from confusion, not one bus accident from death. Confusion can be recovered from; death cannot.

## 16. Operational tooling

Two tools are operational rather than design-time, and Phase 50 names them so they are not forgotten.

**Status dashboard.** A public page at `status.rustforge.dev` showing: current CI health on trunk and on the active LTS branches; the perf-regression dashboard (§5); the known-issues list (filed, triaged, by-version); current LTS versions and their EOL dates; current plugin-API version. The page is generated from CI artifacts and the issue tracker; it is not editorialized. A user who wants to know "is now a good time to upgrade?" answers it from this page in under a minute.

**Security advisory feed.** RSS and Atom feeds at `security.rustforge.dev/feed.{rss,atom}` mirror GitHub Security Advisories for the project. Each entry links the advisory, the patch version, and the migration note. The feed is the canonical *machine-readable* source of truth; downstream packagers and security scanners consume it directly. The feed is generated from the same advisory database that publishes to GitHub; there is no manual editing path.

Both tools are self-hosted (see non-goals). The infrastructure is documented in the operations repo; the dashboard and feed can be re-hosted by a fork without RustForge cooperation, which is the property the self-host bet (Phase 46 §philosophy) requires.

## 17. Fork-friendliness

Phase 23 set the trademark policy: the code is forkable under MIT/Apache, the *name* is not. Phase 50 reaffirms it and adds the operational case the policy was always reserving.

A fork that wants to call itself something other than RustForge is encouraged. The trademark policy provides naming guidance ("powered by RustForge" is fine; "RustForge: Studio Edition" is not without a license) and a contact path for studios that want a license to use the name in marketing. The series has never refused a reasonable request.

A fork that grows large enough to *replace* the project is also a path the series accepts. The handoff process: the benevolent maintainer publishes a notice declaring the fork the successor, the project's repository is archived with a pointer to the successor, the security advisory feed is forwarded, the docs site redirects, the plugin directory is migrated under terms agreed with the successor's maintainers. The successor inherits no obligation to keep the old name; the original repo's archive remains accessible indefinitely.

This is not a hypothetical. Long-lived open-source projects sometimes need a handoff, and projects that have not planned for it tend to handle it badly. The plan above is short, concrete, and rehearsed in writing rather than under stress.

## 18. Maintenance-mode endgame

The project ends in one of two ways. Phase 50 documents both and commits to neither, because the choice can only be made from inside the future the project has by then created.

**Always-evolving.** The project never enters maintenance mode. New features land at a slowing cadence; majors arrive less often; the design surface shifts toward maintenance, ecosystem, and long-tail polish. This is the path most large open-source engines have taken; it is the path the series implicitly assumes by writing 50 phases of design.

**Frozen with security patches only.** The project declares feature-completion at a specific version; subsequent releases are security-only and bug-only. The codebase becomes a long-lived utility rather than an evolving product. This is rarer but legitimate — `make`, `tar`, and several formats-and-protocols projects have lived this way for decades.

The signals that favor freeze: the design surface has stabilized for two consecutive majors with no accepted RFCs of significant scope; the contributor pool has shifted from feature work to maintenance work; user demand has flattened from "what's next?" to "please don't break what works." The signals that favor evolution: active RFCs proposing genuinely new categories; new contributors arriving with new motivations; ecosystem growth accelerating rather than plateauing.

The decision, when it comes, is made by the maintainer committee under the governance process in §14, with a public RFC and a comment period. Either path is a legitimate end for an open-source project; *no path* — drift, abandonment, undocumented cessation — is the failure mode the series will not accept.

## ❌ Out of scope

Operational scope creep is its own failure mode. The following are explicitly not part of Phase 50 and not part of the project as the series defines it:

- ❌ **A hosted CI service.** Use GitHub Actions or self-hosted runners. The project does not operate a CI cloud. Same self-host policy as Phase 29's DDC.
- ❌ **Paid support tiers.** Community-supported only. Studios that need a paid relationship hire consultants from the contributor pool privately; the project does not broker.
- ❌ **Commercial licensing changes.** MIT/Apache stays. No relicensing to a "source-available," BSL, SSPL, or any other gated license, ever. A relicensing PR is closed on sight.
- ❌ **Proprietary telemetry of how users use the engine.** Reaffirmed from Phase 13 and Phase 23. No usage telemetry, no analytics, no "anonymized" feature counters. Crash reports remain opt-in only and stay that way.
- ❌ **Enterprise SLAs.** The project is honest about being a community project. Studios that need a contractual SLA build their own internal support; the project does not promise response times beyond the security disclosure clock in §7.
- ❌ **A vendored package marketplace.** Phase 23 already refused this; Phase 50 reaffirms. The plugin directory is an index, not a store.

## 19. Risks and gotchas

Operational risks are not the same as design risks. Naming the operational ones honestly:

**Governance burnout.** The maintainer committee is unpaid; the work is real. Risk: committee members rotate out faster than the contributor pool produces replacements, and seats sit vacant. Mitigation: 18-month terms (long enough to feel meaningful, short enough to be survivable); staggered turnover; explicit permission to step down mid-term without stigma. Watch metric: average time-to-fill for a vacated seat. If it exceeds 90 days twice consecutively, the committee size or the term length is wrong.

**LTS maintenance cost.** Two LTS branches plus trunk is three concurrent code paths. Risk: backporting eats more cycles than feature work, and the project slows. Mitigation: §4's strict backport policy; the 24-month window (not "indefinite"); the willingness to *not* declare a version LTS if uptake signals are weak. Watch metric: backport PR count per release cycle. Sustained growth means the policy is too loose.

**Deprecation drift.** APIs marked deprecated for years that nobody removes because the removal is awkward. Risk: the deprecation policy in §1 becomes advisory rather than binding. Mitigation: a CI lint that flags `#[deprecated]` items past their announced removal version; the lint blocks merge on the major release that should have removed them. Watch metric: count of past-due deprecations at major-release time. Should be zero.

**Security advisory embargo leaks.** The 90-day clock is a clock; embargo leaks before the clock are an existential trust event. Risk: a committee member discusses an embargoed report on a public channel, or a patch lands on trunk before the advisory. Mitigation: training (every committee member reads the §7 process before the first embargo they handle); private-branch discipline (security patches land on a private branch, merged only at advisory publication); post-mortem of any leak, regardless of cause. Watch metric: number of leaks. Target: zero. A leak is a process failure first and a personal failure only if the process was followed correctly and still failed.

**Plugin ecosystem fracture.** A plugin API bump that breaks too much of the directory at once, with too few authors rebuilding, leaves users stranded between an old engine that runs their plugins and a new engine that doesn't. Risk: ecosystem trust erodes; users pin to old versions; LTS demand grows beyond what the project can sustain. Mitigation: §11's 30-day pre-release window; the needs-rebuild badge so users can see the situation honestly; a hard reluctance to bump the plugin API in minor releases unless absolutely necessary. Watch metric: percentage of directory plugins still on the previous-major API one cycle after a bump. Sustained high numbers mean the bump frequency is wrong.

**Performance budget creep.** A budget exception granted "just this once" turns into the new budget. Risk: the performance contracts in §8 become aspirations again. Mitigation: every exception is documented in the changelog under "Changed," not buried in the PR description; the dashboard tracks budget values over time, not just current versus previous; a quarterly review of the budgets restores them where they have crept. Watch metric: count of granted exceptions per cycle. Sustained growth is a signal that either the budgets are wrong (rewrite them via RFC) or the discipline is wrong (the more likely answer).

## The 50th phase — why we stop here

Fifty is a deliberate number. It is not the number of phases the engine *needs*; it is the number of phases the *design series* needs to be complete. Beyond Phase 50, the questions worth asking depend on facts the series does not have: which features the community has actually used, which APIs the plugin authors have actually reached for, which performance budgets have actually held, which security postures have actually been tested. A 51st phase written in advance of those answers would be guessing in prose.

Knowing when to stop is part of good design. The series began with a 13-phase plan to ship a 1.0 editor; it grew, by Phase 30 and Phase 46, into a 45-phase arc closing AAA parity; it added Phases 47–49 to close the design surface for a 3.0 candidate; and it ends, here, at 50, with the operational handbook that turns all of the above into something a project can keep. To extend the series further would be to confuse two different jobs: design and stewardship. They share an author for these fifty documents; they will not share an author beyond.

The split is honest in the other direction too. Stewardship is not less important than design — it is the work that determines whether design ever pays off — but it is differently disciplined. Stewardship's outputs are decisions made under uncertainty with real users in the loop. Design's outputs are decisions made with clarity and time. A document series can produce the second; only a project can produce the first. Phase 50 is the handoff: the series gives the project everything it can give, and steps back.

The series ends at 50 because to extend it further would be to pretend that thinking can substitute for shipping. It cannot. Fifty is enough. Fifty is the right number.

## Closing — when the map ends

Fifty phases. Forty-nine of design, one of operational commitment. The series begins at Phase 1 with a workspace diagram and ends at Phase 50 with a deprecation policy and a succession plan. Between those bookends, an editor was specified, a runtime was specified, a 1.0 was shipped in design, an ecosystem was shipped in design, a 2.0 closed AAA parity in design, and a 3.0 candidate closed the modern frontier in design. None of it is a built engine. All of it is the map a built engine could follow.

The series has two ends, and they are different ends. Phase 46 is the **reflection**: it looks back over what was decided and audits whether the decisions held together as an arc. Phase 50 is the **commitment**: it looks forward over what would have to be true for the decisions to *survive contact with reality*, and writes the rules. Reflection without commitment is nostalgia. Commitment without reflection is a vendor pitch. Together, the two phases bookend the series with the two postures a long-lived project actually needs from its founding documents: an honest accounting of what was done, and a binding contract for what comes next.

The gap between a design series and a built engine is real, and the series does not pretend otherwise. Fifty phases of careful prose do not produce an editor that opens, a runtime that ticks, or a plugin that loads. They produce a *plan that an engineering team can execute against*, and a set of *invariants the team can be held to*. Whether the engine ever exists depends on the team, the time, the money, the luck, and a hundred small contingencies the series cannot author. The series can give the team a map worth following. It has done that. The rest is not in its hands.

To the readers who followed all fifty phases: thank you. The arc was built for you in the sense that nobody else had a reason to read it cover to cover, and your willingness to do so is the only way the prose got better between phases. To the contributors who would, in some real future, turn parts of this map into territory: thank you in advance, and good luck. The map has bugs; you will find them. Fix them.

Phase 1 began with a single line: an editor that does not contaminate the runtime. Forty-nine phases later, the line still holds, and forty-eight other lines have been drawn alongside it. Phase 50 is the last of them. The series ends here. The work begins.
