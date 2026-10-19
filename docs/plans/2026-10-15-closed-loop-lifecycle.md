# Plan: dogsled owns the lifecycle of everything it starts

Status: draft for review, 2026-10-15.

## Goal

Every worktree, branch, Herdr workspace, tab, and job record that dogsled creates ends when the event that finishes it happens.
Each owner closes its own work when it finishes. That is the closed loop.
A periodic sweep in `serve` catches only what a broken loop leaves behind: a crashed owner, a killed process, or a worktree that something else made.
`dogsled prune` stays as the manual form of that sweep.

One tool owns this. The host `mise` tasks `agent:worktree`, `agent:worktree-remove`, and `agent:prune` (dotfiles `conf.d/host`) are deleted.

## Why

Evidence from netcup, 2026-10-15:

- Seven finished job tabs stayed open for hours (`wJ1:p2`–`pA`). The job ended, but its shell kept the tab. Release 0.8.2 made prune find them; nothing closed them on its own.
- Five worktrees held 5.6 GiB after their work reached `main` through #1541 and #1563. Doctor called them unmerged, because their branches carry files that `main` later deleted, and three held leftover `.brief.md` files and stale edits.
- Three clean worktrees (`feat/e2e-request-trace`, `fix/native-rsvp-reread-editor-plan`, `test/native-lineup-lifecycle`) merge into `main` with no change (`git merge-tree` gives `main`'s tree), yet doctor reports them as unmerged.
- Two detached-HEAD worktrees (`e2e-lead-probe`, `fix-web-edit-event-return-test`) never appear in doctor, because it lists only branch worktrees.
- Cleanup policy lives in three places with different rules: dogsled's doctor (one day for everything), `agent:prune` (`--idle 2h` remote, 24h local, `wt step prune` for integrated), and `agent:worktree-remove` (closes the anchor workspace).

## Decisions (maintainer, 2026-10-15)

1. Cleanup is a closed loop first. Automatic sweeping is the last resort.
2. A finished job's tab closes as soon as the job ends. The job log keeps the output.
3. When an owner's pane is gone, `serve` closes its children when that is safe: the child is idle or done, its worktree is clean, and every commit is on a remote. Otherwise `serve` reports it.
4. Repository setup and teardown commands live in a `dogsled.toml` in each repository. Dogsled creates and removes worktrees with Git itself. It does not use `wt`.

## Lifecycle per resource

| Resource | Created by | Closed by (the loop) | Sweep catches |
| Job tab | `dogsled run` | `dogsled exec` closes its own pane after it writes the final record | a tab whose job record has ended |
| Job record and log | `dogsled run` | expires after a day, as today | same |
| Child worktree, workspace, branch | `dogsled place`, `dogsled agent --branch` | `dogsled close` by the owner | owner gone (decision 3); clean worktree whose branch is integrated |
| Main-checkout anchor workspace | `dogsled place` when the repository had no workspace | the last child's close | an anchor with no children and one idle pane |
| Queue ticket | `place`/`agent`/`run` when full | admission, `cancel`, or `--max` expiry | tickets of a dead owner |
| Item record | every spawn | pane closed, `close`, job end | items whose pane is gone |
| Idle shell pane | anything | none: dogsled did not start it | one day idle, as today |

## `dogsled.toml` (repository)

```toml
[worktree]
# Where linked worktrees go, relative to the main checkout.
dir = ".worktrees"
# Run in the new worktree after creation, in order. A failure stops placement and removes the worktree.
setup = []
# Run in the worktree before removal, in order. A failure stops removal and keeps the worktree.
teardown = [
  "if [ -d node_modules ]; then node {main}/packages/dev-environment/src/main.ts reset --yes; fi",
  "if [ -d node_modules ]; then node {main}/packages/dev-environment/src/main.ts reset --yes --stack e2e; fi",
]
```

Placeholders: `{main}` (main checkout), `{path}` (the worktree), and `{branch}`. Other placeholders are errors.
A repository without `dogsled.toml` gets the defaults: `.worktrees`, no setup, no teardown.
The host config's `hooks.prepare` and `hooks.remove` are removed. Host policy keeps `[cleanup]`.

## Integration rule change

A branch is integrated also when `git merge-tree --write-tree <default> <branch>` produces the default branch's tree: merging it changes nothing.
This finds the three clean worktrees above. It does not find branches that would re-add files `main` deleted; those stay report-only, so a person decides.

## Units

Each unit is one PR in its repository, with its tests. Units 1–3 are independent and can run in parallel. Unit 4 needs unit 1. Unit 5 needs units 1–4 released. Unit 6 needs unit 1 released.

1. **dogsled: native worktrees and `dogsled.toml`.** Parse `dogsled.toml`. `place` creates the worktree with `git worktree add`, locks it, runs `setup`, opens the Herdr workspace, and records the anchor. `close` and prune run `teardown`, unlock, `git worktree remove`, delete the branch only when it is integrated, and close an idle anchor. Remove `hooks.prepare` and `hooks.remove`. Doctor lists detached worktrees.
   Acceptance: `cargo test` covers create, setup failure, teardown, refusal of a dirty tree, branch kept when unmerged, and anchor close; a live `dogsled place` and `dogsled close` on netcup in rondo stop its stacks.
2. **dogsled: merge-tree integration.** Add the no-change merge rule to `integration::check`.
   Acceptance: a test repository where a branch's content is in `main` by another route is integrated; one that would re-add a deleted file is not.
3. **dogsled: job tabs close themselves.** `exec` closes its pane after it saves the final record. A failed close leaves the record correct and the sweep finds the tab.
   Acceptance: `cargo test` shows `pane close` after the record is final; a live `dogsled run --wait -- true` leaves no tab.
4. **dogsled: the sweep in `serve`.** Each tick, `serve` runs the doctor plan and applies only entries that lose nothing: ended job tabs, shells in deleted directories, clean integrated worktrees that nothing holds, local branches whose every commit is on a remote, and orphaned children under decision 3. Everything else stays report-only for `prune`. Doctor's `processes` and `disk` stay report-only. Rust `target/` directories move in as a report-only `targets` class with a prune-only removal.
   Acceptance: `cargo test` drives `serve --once` over a rig with each case; nothing dirty, unpushed, held, or working is touched.
5. **dotfiles: remove the host tasks.** Delete `agent:worktree`, `agent:worktree-remove`, `agent:prune`, and `runtime/agent-prune.mjs` with its tests. Update `rules/topics/20-delegation.md` (no `agent:prune` steps; `dogsled prune` is the manual sweep), the shell hook policy that points at `agent:worktree`, `docs/tools.md`, and pin the release.
   Acceptance: `mise run host:test` and the harness checks pass; `rg 'agent:(worktree|prune)'` finds nothing.
6. **rondo: `dogsled.toml`.** Move the two `[pre-remove]` reset commands from `.config/wt.toml` to `dogsled.toml` and delete `wt.toml`.
   Acceptance: the commit hook passes; a `dogsled close` of a rondo child stops its dev and e2e stacks.

## Open questions

- Should units 1–4 ship as one dogsled release, or each as its own release? A single release keeps the host tasks working until unit 5 lands, so the order is safe either way.
- Herdr also has `herdr worktree create` and `herdr worktree remove`. Unit 1 uses Git for both, because dogsled must run `setup` and `teardown` around them and must decide branch deletion with its own integration rule. Dogsled keeps `herdr worktree open` to open the workspace. Is that split right, or should creation go through Herdr?
