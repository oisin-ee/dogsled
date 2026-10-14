# Changelog

All notable changes to this project are documented in this file.

## 0.7.0 - 2026-10-14

- Skill: spawn a placed agent with `placement=pane pane=<pane_id>`. Without `placement`, Shepherdr ignored the prepared pane and opened a tab in the caller's workspace.
- Pi: give every `agents spawn` without a `placement` its own worktree workspace. Use the prepared or open worktree pane, or place a new `agent/<label>` branch with `wrangle place`. A queued or failed placement blocks the spawn with its reason.
- Find integrated worktrees from Git alone: a deleted branch, an ancestor of the remote default branch, or a branch that one commit there applies (merge, squash, or cherry-pick). A branch on the base tip is not a candidate.
- Do not count a shell with no foreground command as holding a worktree. Prune closes such panes and the worktree's workspace before removal. Doctor reports held integrated worktrees with the reason.
- Add `hooks.remove` (`{branch}`, `{path}`), default `mise run -q agent:worktree-remove -- {branch}`. `close` and `prune` both remove through it; wrangle no longer runs `wt` or `git worktree remove` itself.
- Let `close` finish a child tab in the caller's own workspace. Refuse only the caller's own pane, a worktree the caller works in or shows, and busy panes in the worktree.
- Print the full `wrangle close PANE --host LABEL` command for each finished child in `status`, prompt context, and check-ins.
- Let prune drop a stale registration whose worktree directory is gone. Run remote `doctor` and `prune` under the setup time limit.

## 0.6.2 - 2026-10-14

- Accept a host checkout whose origin names the same repository in another spelling, such as `git@github.com:owner/repo.git` for `https://github.com/owner/repo.git`. Compare host and repository path, not the raw URL.

## 0.6.1 - 2026-10-14

- Remove the hourly automatic worktree cleanup from `serve`. Wrangle never removes anything on a timer.
- Prompt the owner as soon as its child finishes, with the exact `wrangle close` command. Idle and done count as one event.
- List close commands in prompt context only for the caller's own finished children. `status --brief` shows only the count.
- Stop prompting about an agent item after it closes.

## 0.6.0 - 2026-10-14

- Add `close PANE [--host H]`: close a finished child's Herdr workspace, remove its worktree with `wt remove` so the repository's remove hooks run, and end its item. Refuse active agents, dirty or foreign-locked worktrees, and the caller's own workspace before any change.
- Keep an idle or done agent item open until its pane closes. Check-ins name the exact `wrangle close` command, and the brief, status, and Pi widget show unclosed children.

## 0.5.0 - 2026-10-13

- Detect transcript stalls, publish stall timestamps, and set or clear sidebar marks. Include stall counts in prompt context.
- Add durable idle-owner check-ins, job-completion events, and top-level stall notifications, independent from admission and publication.
- Resolve exact Claude and Codex transcript session IDs without choosing an ambiguous file.
- Add read-only `doctor`, confirmed `prune`, and hourly safe worktree cleanup in registered repositories.
- Revalidate complete Git identities and remove only the approved path. Keep repository registrations after the last worktree removal.
- Preserve cleanup reports when registered paths are missing or unreadable. Never kill reported processes or force-remove worktrees.
- Stop observing closed items. Show sanitized transcript activity rather than agent footer lines in check-ins.

- Package wrangle hooks and one shared skill for Pi, Claude Code, and Codex. Add Claude and Codex marketplaces.
- Add `hook context --harness claude|codex|pi`, with event-specific JSON or plain text from the shared brief formatter.
- Bound prompt context to five seconds. Missing Herdr, snapshots, or binaries do not block prompts.
- Keep package versions aligned with Cargo and verify plugin paths and release service templates in tests.

- Keep job capture and exit status independent from a closed pane output.
- Enforce one command deadline across discovery, preparation, transport, and lock waits. Bound routine operations to 30 seconds and setup operations to 10 minutes.
- Run service actions independently from snapshot publication and record ticket timeouts without stopping the probe loop.
- Keep the Pi polling timer active after an initial refresh failure. Race concurrent store tests through barriers.

- Add jobs (`run`, `exec`, `ps`, `logs`, `stop`, `wait`) with output logs, process-group cancellation, deadlines, and sidebar lifecycle reports.
- Add harness-independent `agent`, owner-host `track` and `items`, and locked FIFO admission with `cancel` and idle-owner wake-ups.
- Add jobs to host snapshots and open items and tickets to status. Service admission and owner reconciliation use the peer cache.
- Clone missing repository checkouts from the caller's origin before prepare or launch. Reject origin conflicts without overwriting paths.
- Check mise trust after fresh clones and trust only the new checkout when mise refuses its configuration.
- Track Shepherdr spawns in Pi and show open jobs and tickets above the editor, refreshed every ten seconds and after tool results.
- Add `serve` with local-only probes, atomic `host.json`, memory used/total, and a configurable ten-second interval.
- Record each agent pane, harness, state, transcript path, and transcript file modification time in the host snapshot.
- Make `status` read snapshots, with parallel peer reads and a shared ten-second cache. Show stale and unavailable hosts instead of hiding them.
- Add `status --line`, `status --brief`, and `place`. Placement reserves capacity until its TTL (90 seconds by default) and can prepare a worktree.
- Run host-policy prepare hooks in the repository checkout. Restore `--base` and literal `{branch}`/`{base}` substitution. Reject unknown placeholders. Remove repository hook overrides.
- Report Linux working-set memory from the cgroup with the smallest finite ancestor limit.
- Bundle launchd and systemd service definitions in release archives. Dotfiles installs them later.
- Replace the Pi spawn tool with CLI placement and keep prompt context from `wrangle status --brief`.
- Remove leads, marks, the old spawn/ticket API, lifecycle reports, and Herdr release hooks. The new stores do not migrate old ticket state.
- Remove the sidebar snippet and obsolete lead/placement plan. Keep the Herdr status-board popup.
- Document the approved transcript-based stalls, owner check-ins, and cleanup decisions for later units.

This entry does not change package versions or publish a release.

## 0.4.0 - 2026-10-09

- Repository config: an optional `wrangle.toml` at the root of a repository sets
  `[hooks] prepare` for spawns from that repository. It replaces the host policy's hook. The lead
  reads it from the main checkout. Admission thresholds stay in `~/.config/wrangle/config.toml`.
- The `prepare` hook's `{repo}` is now valid on the admitted host. A directory resolves to its
  main checkout, so a lead inside a linked worktree works. On a remote host the checkout is
  relative to home, where ssh starts the hook. Hooks no longer need to rebuild the path from a
  fixed folder such as `~/dev/<name>`.

## 0.3.0 - 2026-10-09

- Lifecycle log: the ledger appends `queued`, `admitted`, `spawned`, `released`, and `cancelled`
  events to `events.jsonl` under its lock. It stores no prompts or messages, keeps 30 days, and
  skips a torn line instead of failing.
- New command `wrangle report [--since <duration>] [--json]`: per-ticket queue wait and run time,
  plus host refusal counts with reasons.
- `wrangle status` shows the refusal reason for every ineligible host.
- The Pi tool passes `base` only to the `prepare` hook, never to Shepherdr's spawn.

## 0.2.1 - 2026-10-09

- The Pi tool never blocks: every spawn goes to `agents spawn` with `blocking: false`, so the call
  returns once the child has its task and the lead keeps working. The child's completion arrives
  as a Shepherdr message. The `blocking` parameter is removed.

## 0.2.0 - 2026-10-08

- One lead per repository. The first spawn from a Herdr pane claims the lead for the pane's
  repository (`⌂ <repo>`, tokens `role=lead`, `repo=`). A second live lead is refused with
  `lead_exists` before anything is admitted; `take_over=true` / `--take-over` moves it.
  New command `wrangle lead`.
- Children carry `lead=lead:<repo>` instead of `owner=`. `wrangle status` lists every lead with
  its children before the hosts; older `owner=` children are grouped too.
- Layout is no longer a choice: the Pi tool drops `placement`, `workspace`, and `pane`, and the
  CLI drops `--workspace`. A child with a branch opens in its worktree; any other child opens as
  a tab in the lead's workspace (a new workspace on a remote host). `spawn --lead` is optional
  inside Herdr.
- Pi widget shows the session's lead, its children by status, the queue, and host headroom;
  refreshes on every call and every 60 s while work runs. Toasts on spawn and on `lead_exists`.
- `wrangle status --sidebar` prints the Herdr sidebar rows (`herdr-sidebar.toml`).

## 0.1.1 - 2026-10-08

- Herdr plugin: `pane.closed` and `pane.exited` release the pane's reservation; a `board`
  popup runs `wrangle status --watch`.

## 0.1.0 - 2026-10-08

- Skill `wrangle`: the orchestration procedure for Pi and for shell harnesses.
- README: install, configuration, CLI, Pi tool, sidebar rows, skill.

- Pi tool `wrangle`: `spawn` (default) admits on the host with the most headroom, runs the
  `prepare` hook when `branch` is set, calls Shepherdr `agents spawn` on that host, and marks
  the child pane; `status`, `cancel`, `help`. Queued calls return a ticket at once.
- Queue: tickets persist in the session (`wrangle-queue` entry), a poll timer (30 s → 60 s)
  re-admits the oldest ticket, admission sends a `wrangle` wake-up message, `wrangle ticket=…`
  reuses the stored arguments; queued tickets show in a widget above the editor, as a
  `queue` token on the lead's Herdr pane, and as a toast.
- CLI `spawn` for harnesses that are not Pi (Claude Code, Codex, …): admit or resume a ticket,
  place a pane (the `prepare` hook with `--branch`, else a new tab or workspace), `herdr agent
  start`, mark, `herdr agent prompt`. An agent blocked on a startup dialog keeps its pane and
  returns `status: blocked` with the next step.
- Engine: `prepare` command runs the policy's `[hooks] prepare` template on a host.
- Engine: `probe`, `admit`, `release`, `status`, `cancel`, `mark`, `queue`, `notify`, and the
  per-host `host` primitives. Balanced admission by headroom, TTL reservations under a
  per-host lock, queue tickets, remote hosts over `ssh -T` (`remote_command`, default
  `mise x -- wrangle`).
- Scaffold: Cargo crate, Pi package manifest, CI, cargo-dist release workflow.
