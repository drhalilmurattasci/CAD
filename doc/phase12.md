# Phase 12 — Collaboration & Version Control

Teams building on RustForge need more than a shared network drive. A game project is a mix of text (scenes, component data, `.meta` sidecars) and binaries (textures, meshes, audio, baked lightmaps), and every DVCS in common use — Git, above all — has sharp edges around both. Phase 12 puts Git awareness *inside* the editor: status badges on assets, a commit panel, a structural merge driver for `.ron` scenes, a conflict-resolution UI, and LFS onboarding. It is deliberately a **wrapper**, not a replacement.

We do not build a sync service, a hosted asset-lock server, or a pull-request UI. Git already exists, it already works, every studio already has opinions about hosting (GitHub, GitLab, self-hosted Gitea). The editor's job is to make the 90% flow — pull, edit, commit, push — painless and to smooth over the one place Git genuinely struggles: merging scene graphs.

## Goals

By end of Phase 12:

1. **Git-aware Content Browser** — every asset shows its VCS status without the user running `git status`.
2. **In-editor commit + push** for the common case, with `.meta` sidecars auto-staged beside their sources.
3. **Branch awareness** — current branch in the title bar, external branch switches trigger a Phase 4 project reload.
4. **Structural scene merge driver** — three-way merge of `.ron` scenes keyed by `SceneId`, installed via `.gitattributes`.
5. **Conflict-resolution UI** — per-field before/local/remote picker when structural merge can't auto-resolve.
6. **LFS bootstrapping** — detect large binaries on stage, prompt to track via LFS, ship a sensible `.gitattributes` template on project init.
7. **Graceful single-user mode** — a non-Git project hides all VCS UI silently.

## 1. Wrapper, not reimplementation

RustForge shells out to Git through a Rust binding. The two candidates:

- **`gitoxide` (`gix`)** — pure Rust, actively developed, fast `status` and `diff`, no libgit2 dependency.
- **`git2-rs`** — libgit2 bindings, more feature-complete today for exotic operations.

**Recommendation: `gix` for read paths (status, log, blame, diff), shell out to the `git` binary for write paths (`add`, `commit`, `push`, `merge`, `lfs`).** Rationale: `gix`'s read APIs are fast and have no C dependency; the write surface is where Git's own binary has decades of edge cases (credential helpers, LFS extension, hook invocation) that are a time sink to reimplement. Users already have `git` on PATH in any realistic development setup.

```
crates/rustforge-vcs/
├── lib.rs                  # VcsService facade, feature-gated
├── detect.rs               # is this directory a Git repo?
├── status.rs               # gix-backed status, debounced
├── commit.rs               # shells out to `git add`, `git commit`
├── branch.rs               # current branch, branch-change listener
├── merge_driver.rs         # standalone binary: see §5
├── conflict.rs             # conflict model shared with UI
├── lfs.rs                  # LFS detection + `git lfs track` shell-out
├── blame.rs                # gix blame + SceneId structural blame
└── hooks.rs                # installer for the pre-commit dry-load
```

A dedicated crate keeps the editor crate from depending on `gix` transitively and lets the merge driver binary link only what it needs.

### 1.1 Why no built-in sync service

A "RustForge Cloud" that pushes changes on save is tempting and wrong for this phase:

- It duplicates Git's job badly — no blame, no history, no branches.
- It requires a server the project does not have.
- It splits the team's mental model between "the editor's sync" and "Git".

If a studio wants continuous background sync they can run `git` in a script; the editor must never hide Git from them.

## 2. Content Browser status overlay

Phase 5's file watcher already polls the project tree. Phase 12 piggybacks: after a debounced watcher event (or on a 2-second timer, whichever fires first), call `gix::status` and merge the result into the Content Browser's file model.

Per-entry status:

| Status       | Badge           | Source                              |
| ------------ | --------------- | ----------------------------------- |
| `Clean`      | no badge        | tracked, no changes                 |
| `Untracked`  | `?` (gray)      | not tracked                         |
| `Modified`   | `M` (blue)      | tracked, working-tree differs       |
| `Staged`     | `+` (green)     | in index                            |
| `Conflicted` | `!` (red)       | unmerged entry                      |
| `Ignored`    | dimmed tile     | matches `.gitignore`                |

Badges render on the thumbnail corner. Hovering shows the ahead/behind counts for the current branch. Right-click adds **Stage**, **Unstage**, **Revert**, **View history**, **Blame**.

### 2.1 Debounce and throttling

`git status` on a 50k-file repo takes hundreds of milliseconds. Rules:

- Coalesce watcher events into a 500 ms window before calling `status`.
- If `status` is still running when a new event arrives, mark "dirty" and run once more when the current call returns. Never enqueue a chain.
- Status results older than 30 s are always re-fetched on focus.
- Repos with >20k tracked files fall back to `gix::status_no_renames` (cheaper; renames resolved lazily on blame/history).

## 3. Commit panel

The commit panel sits as a dockable tab. Not a full `git gui` — intentionally.

```
┌─ Commit ──────────────────────────────────────────────┐
│ Branch: feat/weapon-balance   ↑2 ↓0                    │
├────────────────────────────────────────────────────────┤
│ Staged (3)                                             │
│  + scenes/arena.ron                                    │
│  + scenes/arena.ron.meta                               │
│  + assets/weapons/sword.mat                            │
│ Unstaged (2)                                           │
│  M scripts/weapon.rs                                   │
│  ? assets/weapons/sword_new.png                        │
├────────────────────────────────────────────────────────┤
│ Message:                                               │
│ ┌────────────────────────────────────────────────┐    │
│ │ Rebalance sword damage curve                    │    │
│ └────────────────────────────────────────────────┘    │
│ [ ] Amend     [ Commit ]     [ Commit & Push ]         │
└────────────────────────────────────────────────────────┘
```

### 3.1 Auto-staging `.meta`

`.meta` sidecars are Phase 4's GUID anchors. Committing `hero.png` without `hero.png.meta` breaks every scene that references the GUID. Rule: when the user stages an asset, the commit panel auto-stages its `.meta`. When the user stages a `.meta`, the panel auto-stages the paired asset if modified. The user can override the auto-stage explicitly, but the default is paired.

### 3.2 What's not in the panel

- No rebase, cherry-pick, interactive stash, reflog. Shell out and use a real Git client.
- No diff view in the panel itself — "Show diff" opens the system `git difftool` or a paging view in the editor.
- No branch creation/delete. Branch management is external.

This is not a limitation the user has to accept with frustration — it is the correct scope. Studios standardize on GitKraken, Fork, Sourcetree, or plain `git`. Competing with them is wasted effort.

## 4. Branch awareness

Current branch renders in the title bar: `RustForge — MyProject [feat/weapon-balance]`. A background task re-reads `HEAD` every time the file watcher reports a change under `.git/`.

### 4.1 External branch switch

If the branch changes while the editor is running (user ran `git checkout` elsewhere), the editor:

1. Detects the `HEAD` change.
2. Checks whether any open scene or asset editor is dirty. If dirty, prompts save/discard/cancel.
3. Clears the Phase 6 command stack (scene reload invalidates it — this is already the rule).
4. Closes all asset-editor tabs (Phase 8 §1.2 bulk prompt already covers this path).
5. Reloads the project (Phase 4 project-open path).

Never attempt to "reconcile" in-memory edits against the new branch. That's the merge problem and the wrong place to solve it.

## 5. Scene merge strategy

Two users edit `arena.ron` on two branches. Git's line-based merge will fail on almost any overlapping change because `.ron` scene entries are order-sensitive arrays of structs with many fields per line.

### 5.1 Options considered

- **(A) Lock-based editing** — reserve a scene before editing, block others. Requires a central lock server. **Rejected**: we have no server, and locks kill parallelism.
- **(B) Structural three-way merge** — parse base, local, remote as trees keyed by `SceneId`; reconcile field-by-field. Conflicts only on same-field same-entity edits.
- **(C) Manual resolution dialog** — always pop the conflict UI; user resolves every scene conflict by hand.

### 5.2 Recommendation: B with fallback to C

Structural merge handles the common case (two users edit different entities, or the same entity's different components) automatically. When two users write the same field on the same entity, we can't guess — fall through to the conflict UI (C).

### 5.3 The merge driver

Git merge drivers are standalone executables invoked by `git merge`. Phase 12 ships `rustforge-mergetool` as a separate binary in the same workspace (Phase 1 §2 calls out workspace crates):

```rust
// crates/rustforge-vcs/src/bin/mergetool.rs
fn main() -> ExitCode {
    let args: MergeArgs = MergeArgs::parse(); // base, local, remote, merged
    let base   = SceneFile::load(&args.base)?;
    let local  = SceneFile::load(&args.local)?;
    let remote = SceneFile::load(&args.remote)?;
    match merge::three_way(&base, &local, &remote) {
        MergeResult::Clean(merged) => {
            merged.write(&args.merged)?;
            ExitCode::SUCCESS           // Git marks resolved
        }
        MergeResult::Conflicted(merged, conflicts) => {
            merged.write(&args.merged)?;            // best-effort merge
            conflicts.write_sidecar(&args.merged)?; // `.conflicts.ron`
            ExitCode::from(1)           // Git leaves in conflicted state
        }
    }
}
```

Installed via `.gitattributes` at project-init time:

```
*.ron merge=rustforge-scene
```

And in `.git/config` (per-clone, written by `rustforge-editor --init-git` helper):

```
[merge "rustforge-scene"]
    name = RustForge structural scene merge
    driver = rustforge-mergetool --base %O --local %A --remote %B --merged %A
```

### 5.4 Three-way merge algorithm

Pseudocode sketched:

```rust
pub fn three_way(base: &Scene, local: &Scene, remote: &Scene) -> MergeResult {
    let mut out = base.clone();
    let mut conflicts = Vec::new();
    let ids: HashSet<_> = base.ids().chain(local.ids()).chain(remote.ids()).collect();
    for id in ids {
        match (base.get(id), local.get(id), remote.get(id)) {
            (_,        None,        None)        => out.remove(id),           // both deleted
            (Some(b),  Some(l),     None)        if b == l => out.remove(id), // remote deleted, local unchanged
            (Some(_),  Some(l),     None)        => conflicts.push(DeleteVsEdit{ id, kept: l }),
            (None,     Some(l),     None)        => out.insert(id, l.clone()), // local-only add
            (None,     None,        Some(r))     => out.insert(id, r.clone()), // remote-only add
            (Some(b),  Some(l),     Some(r))     => merge_entity(&mut out, id, b, l, r, &mut conflicts),
            // ... symmetric cases
        }
    }
    if conflicts.is_empty() { MergeResult::Clean(out) } else { MergeResult::Conflicted(out, conflicts) }
}
```

Per-entity merge walks components; per-component walks fields via the Phase 2 reflection registry. A field counts as conflicting only if `local != base && remote != base && local != remote`.

## 6. Conflict resolution UI

When the merge driver emits a `.conflicts.ron` sidecar, the Content Browser flags the scene as `Conflicted`. Opening it routes to the conflict panel, not the scene viewport.

```
┌─ Conflicts: arena.ron ─────────────────────────────────────────┐
│ Entity: SceneId(17)  "EnemySpawner_A"                           │
│                                                                  │
│  Field: Transform.translation                                    │
│    Base    ( 0.0, 0.0, 10.0)                                     │
│    Local   ( 0.0, 2.0, 10.0)  [●]                                │
│    Remote  ( 5.0, 0.0, 10.0)  [ ]                                │
│    Custom  [__,__,__]         [ ]                                │
│                                                                  │
│  Field: SpawnConfig.interval_s                                   │
│    Base   3.0                                                    │
│    Local  2.5                 [ ]                                │
│    Remote 4.0                 [●]                                │
│                                                                  │
│ [ Accept all Local ] [ Accept all Remote ] [ Resolve & save ]    │
└──────────────────────────────────────────────────────────────────┘
```

"Resolve & save" writes the final merged `.ron`, stages it, removes the `.conflicts.ron` sidecar, and calls `git add`. The scene can then be committed from the commit panel like any other change.

## 7. Binary-asset conflicts

Textures and meshes cannot be three-way merged. When Git reports a binary conflict, the editor offers a three-button dialog:

- **Keep mine** — `git checkout --ours <path>`, stage.
- **Keep theirs** — `git checkout --theirs <path>`, stage.
- **Keep both** — keep `ours`, rename `theirs` to `<name>_conflict_<shorthash>.<ext>`, stage both, regenerate `.meta` for the renamed copy (new GUID).

GUID-based ownership (Phase 4) means "keep both" does not corrupt references: the old file keeps its GUID, the renamed copy gets a fresh one, and scenes continue to point at the original. Path-based engines do not have this luxury.

## 8. LFS integration

Large binaries in Git explode repo size and make `clone` painful. Two touchpoints:

### 8.1 Stage-time prompt

When the commit panel sees a newly staged file `>= 10 MB` (configurable) that isn't already LFS-tracked, it shows:

```
sword_hero.fbx is 42 MB. Track with Git LFS?
[ Track pattern *.fbx with LFS ]  [ Track this file only ]  [ Commit as-is ]
```

"Track pattern" runs `git lfs track "*.fbx"`, which edits `.gitattributes`. "This file only" uses a path-specific rule. "As-is" warns once and remembers the choice for the session.

### 8.2 Project-init `.gitattributes`

On `File → New Project`, write a template `.gitattributes`:

```
# RustForge baseline
*.ron          merge=rustforge-scene text eol=lf
*.meta         text eol=lf
rustforge-project.toml text eol=lf

# Binary assets — LFS
*.png  filter=lfs diff=lfs merge=lfs -text
*.jpg  filter=lfs diff=lfs merge=lfs -text
*.tga  filter=lfs diff=lfs merge=lfs -text
*.exr  filter=lfs diff=lfs merge=lfs -text
*.fbx  filter=lfs diff=lfs merge=lfs -text
*.glb  filter=lfs diff=lfs merge=lfs -text
*.wav  filter=lfs diff=lfs merge=lfs -text
*.ogg  filter=lfs diff=lfs merge=lfs -text

# Never commit
dist/            export-ignore
target/          export-ignore
```

LFS is opt-in per clone (`git lfs install`); the editor detects absence on first start and offers to run it.

## 9. `.meta` files and commit policy

`.meta` sidecars hold an asset's GUID plus import settings. Rules:

- **Always tracked.** The editor refuses to open a project whose `.meta` files are gitignored — that guarantees GUID churn across machines.
- **Auto-staged** alongside their source (§3.1).
- **Never merged textually** beyond Git's default line merge — fields are GUID + import config, and real conflicts here are almost always a rename race that wants human attention.

## 10. History and blame

Two entry points:

### 10.1 File history

Right-click any asset → **View history** opens a tab listing the commits that touched it. Each row shows hash, author, date, message. Clicking a row opens `git show` in the system pager or an external difftool. No in-editor graph viewer — that's a solved problem in every external Git client.

### 10.2 Structural scene blame

For `.ron` scenes, plain line-blame is useless — reorder the file and every line blames the reorderer. Structural blame runs over the scene tree:

```rust
pub fn blame_scene(path: &Path) -> BTreeMap<SceneId, BlameEntry>;

pub struct BlameEntry {
    pub last_author: String,
    pub last_commit: Oid,
    pub last_date:   DateTime<Utc>,
}
```

Implementation: walk the scene's Git history; for each commit, diff parsed scene-before vs. scene-after; for each `SceneId` whose serialized bytes changed, record that commit. Cache results keyed by `(path, HEAD oid)`. This is slow on first run and cheap thereafter.

The scene outliner can show "last edited by" next to each entity. Useful for "who broke the boss fight."

## 11. Pre-commit hook

Optional, opt-in the first time the user commits: "Install a pre-commit hook that loads the project headlessly and fails if it doesn't?" If accepted, write `.git/hooks/pre-commit`:

```sh
#!/usr/bin/env sh
exec rustforge-cli validate --quiet
```

`rustforge-cli validate` does a project open + scene parse + asset resolve with the `editor` feature off; exits non-zero on any failure. Catches broken scenes (missing GUIDs, invalid component data) before they hit `main`.

Hooks are per-clone (not committed); the prompt re-appears for new clones. This is acceptable — coercing every teammate into a hook is a studio-policy decision, not an editor one.

## 12. Editor-state vs. project-state

Three commit policies live side-by-side in a project:

| Path                              | Commit?        | Why                                      |
| --------------------------------- | -------------- | ---------------------------------------- |
| `rustforge-project.toml`          | always         | project identity                         |
| `assets/`, `scenes/`              | always         | authored content                         |
| `.rustforge/settings.toml`        | always         | shared project settings (render presets) |
| `.rustforge/user.toml`            | ignored        | per-user editor prefs, dock layout       |
| `.rustforge/cache/`               | ignored        | import caches, derived data              |
| `dist/`, `target/`                | ignored        | build artifacts                          |

`File → New Project` writes a matching `.gitignore`:

```
/target
/dist
/.rustforge/user.toml
/.rustforge/cache
```

## 13. Single-user mode

If `gix::discover` returns `NotARepository`, the VCS service stays disabled:

- Title bar shows no branch.
- Commit panel, History, Blame, Conflict tabs are hidden — not greyed out, absent.
- Content Browser shows no status badges.
- The commit-related menu items route to a one-shot "Initialize Git repository?" action that runs `git init`, writes the baseline `.gitattributes` and `.gitignore`, and re-enables the UI.

No modals, no errors, no nags. Projects without Git must feel unchanged from Phase 11.

## 14. Build order within Phase 12

Each sub-step is independently testable against a throwaway Git repo.

1. **VCS wrapper crate** (§1) — `rustforge-vcs` skeleton, `gix` detect + status, no UI. Test: status result matches `git status --porcelain` on fixtures.
2. **Content Browser status overlay** (§2) — hook `rustforge-vcs` into Phase 5's watcher, render badges. Test: modify a file, badge appears within the debounce window.
3. **Commit panel** (§3) — stage, unstage, commit, push, `.meta` auto-staging. Test: edit → stage → commit → `git log` shows it.
4. **Branch awareness + external-switch reload** (§4) — title bar, `HEAD` watcher, project-reload integration with Phase 4/6.
5. **Merge driver binary + `.gitattributes` install** (§5) — `rustforge-mergetool` as a standalone binary; clean-merge fixtures pass; conflict fixtures produce sidecars.
6. **Conflict resolution UI** (§6) — consume the sidecars, write resolved scenes. Test: simulated two-branch edit resolves end-to-end.
7. **Binary conflict dialog** (§7) — ours/theirs/both, GUID-preserving rename for "keep both."
8. **LFS bootstrapping** (§8) — stage-time prompt, project-init `.gitattributes`, detect missing `git lfs`.
9. **Pre-commit hook installer** (§11) — opt-in dialog, `rustforge-cli validate` entry point.
10. **Structural scene blame** (§10.2) — the slow path, built last so the UI is stable before we cache anything.

## 15. Scope boundaries — what's NOT in Phase 12

- ❌ **Built-in code review / pull request UI.** Use the host's web UI.
- ❌ **Server-hosted asset locking.** No server; see §5.1.
- ❌ **Perforce, Mercurial, SVN, or any non-Git VCS.** Git only. Perforce integration is a plausible separate future phase for studios that require it.
- ❌ **AI-assisted conflict resolution / semantic merge suggestions.** Structural merge is enough; guessing user intent is out of scope.
- ❌ **Real-time multi-user editing (operational transforms, CRDTs).** Different product entirely.
- ❌ **In-editor Git graph / rebase / cherry-pick UI.** External tools cover this; §3.2.
- ❌ **Built-in credential management / SSH key UI.** Git's credential helper is the right layer.
- ❌ **Automatic push on commit.** "Commit & Push" is a button; auto-push on save is a surprise-foot-gun.

## 16. Risks & gotchas

- **Merge driver not installed on every clone.** `.gitattributes` names the driver, but each clone must have `git config merge.rustforge-scene.driver` set and `rustforge-mergetool` on PATH. Solution: a `rustforge-editor --install-merge-driver` helper, auto-offered on first project open if missing. Fallback when absent: Git falls back to binary-style conflict (keep-ours/keep-theirs); the conflict UI still works but has no automatic resolution.
- **LFS pointer files leaking into non-LFS clones.** A teammate without `git lfs install` sees 130-byte pointer files and is confused why their textures are blank. Detect small files in binary extensions on open and surface a toast: "This looks like an un-fetched LFS pointer. Run `git lfs pull`."
- **`.meta` drift.** Someone deletes a `.meta` by hand or excludes it from a commit. Every scene referencing that GUID breaks silently. The pre-commit validator (§11) catches this; the editor's project-load also flags orphans but by then the commit may be landed. Document the failure mode.
- **Long `git status` on huge repos.** 100k-file monorepo hits seconds. `gix` status with its fs-monitor integration helps; the status cache (§2.1) absorbs the rest. If it's still bad, expose a pref to slow the poll to 10 s.
- **Binary-diff storage explosion.** Re-saving a 200 MB lightmap daily bloats the repo even with LFS (old versions linger on the LFS server). Document that baked outputs should live in a separate "artifacts" repo or be regenerated from source. The `.gitignore` template quietly suggests this by excluding `dist/`.
- **Cross-platform line endings on `.ron`.** A Windows clone with `core.autocrlf=true` can rewrite `\n` to `\r\n` and break the scene merge driver's byte-level comparisons. `.gitattributes` pins `*.ron text eol=lf` (§8.2) — but if a user overrides locally, every scene commit becomes noisy. Detect mixed line endings on load and warn.
- **Merge driver binary version skew.** Teammate A ships a new field; teammate B's older `rustforge-mergetool` doesn't parse it and either errors or silently drops it. The driver must refuse to run (exit non-zero, Git marks conflicted) on unknown schema rather than discarding data. Version the scene format (Phase 4) and gate the merge on version match.
- **Push credentials prompt.** `git push` from a spawned process can hang forever waiting for a password prompt on stdin. Always shell out with an inherited terminal and a timeout, or rely on a configured credential helper. Never reimplement credentials.
- **Hooks and WSL/Windows line endings.** A `pre-commit` written with CRLF fails with `env: 'sh\r'`. Write hooks with explicit LF and mark executable; test on Windows specifically.
- **Conflict UI drift from on-disk state.** The user opens the conflict panel, then runs `git reset` externally. The panel's state is now stale. Re-validate against the index on every focus and refuse to save if the conflict has already been resolved.

## 17. Exit criteria

Phase 12 is done when all of these are true:

- [ ] `rustforge-vcs` crate builds in isolation; its `status()` output matches `git status --porcelain` on a corpus of fixture repos.
- [ ] Content Browser renders correct status badges within 500 ms of a file change.
- [ ] Commit panel stages, unstages, commits, and pushes with a user-authored message; `.meta` sidecars auto-stage with their sources.
- [ ] Title bar shows the current branch; changing branch externally triggers a Phase 4 project reload with the Phase 6 command stack cleared.
- [ ] `rustforge-mergetool` is a buildable workspace binary; on clean-mergeable fixtures it exits 0 with a correct merged `.ron`; on conflicting fixtures it exits 1 with a `.conflicts.ron` sidecar.
- [ ] `.gitattributes` install helper wires the driver per-clone; editor detects and offers to run it.
- [ ] Conflict resolution UI displays per-field base/local/remote, writes a resolved `.ron`, and removes the sidecar on save.
- [ ] Binary-conflict dialog preserves GUIDs on "keep both" (verified by a scene that references the original GUID continuing to load).
- [ ] LFS stage-time prompt fires for files over the configured threshold and writes valid `.gitattributes` entries.
- [ ] New-project wizard writes baseline `.gitattributes` and `.gitignore` matching §8.2 and §12.
- [ ] Pre-commit hook installer is opt-in on first commit and a failing `rustforge-cli validate` blocks the commit.
- [ ] Structural scene blame returns correct `(SceneId → last author)` on a fixture repo with multiple commits.
- [ ] Single-user mode (no `.git/`) hides all VCS UI without errors; `Initialize Git repository?` is a one-click path to enable it.
- [ ] `rustforge-core` still builds and runs without the `editor` feature; `rustforge-vcs` is only pulled in by `rustforge-editor`.
