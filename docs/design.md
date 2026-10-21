# Design: dogsled as host tooling for agents

Status: draft for review. Replaces `docs/plan.md` (the lead/orchestrator design).

## Goal

Any agent (Pi, Claude Code, Codex) and the user can:

1. See fresh load, agents, and jobs on every Herdr host.
2. Start an agent or a job on the host with the most headroom, with the same tool.
3. Queue work when every host is full.
4. Get a reliable wake-up when a child agent or job ends, stalls, or fails.
5. Find and remove cruft on a host (stale panes, orphaned worktrees, full disks).

Dogsled replaces Shepherdr for delegation. It starts Pi, Claude, and Codex children, carries every message between agents through an inbox, closes children on events, and does placement, jobs, the queue, the stall watchdog, status, and cleanup.
The board, inbox, and event-driven close are specified in `docs/plans/2026-10-17-dogsled-board.md`.

## Where the user and agents see it

| What | Where | Source |
|---|---|---|
| All agents on all hosts | Herdr sidebar (already built in) | Herdr integrations |
| Jobs | Herdr sidebar, one row per job | dogsled `report-agent` |
| Notes on a pane ("stalled 10m", "exit 1 · 3m") | Sidebar `$dogsled` token | dogsled `report-metadata` |
| Load per host | Tab-row status area | `dogsled status --line` (dotfiles #31 uses `--json` + jq until then) |
| Children, jobs, and queue tickets this session started | the board widget in Pi, and `/dogsled` | `dogsled board --json` |
| Messages to this agent | a `dogsled` message in Pi | `dogsled inbox` |
| Everything, for agents | `dogsled status [--json]` and a short block added to each prompt | dogsled |

## Parts

### 1. Host service: `dogsled serve`

One process per host, started by launchd (macOS) or a systemd user unit (Linux). It does local work only. It uses SSH only for owner-state reads and queue admission. The measurement step remains local.

Every N seconds (default 10) it:

- Probes the host: CPU busy fraction, cores, memory used (and macOS pressure level), disk free, Herdr agents and panes (`herdr agent list`, `pane list`); appends the sample to `samples.jsonl`.
- Writes the probe to `~/.local/state/dogsled/host.json`.
- Runs the stall watchdog on this host's panes (part 4).
- Starts queued work on an eligible host when it has headroom (part 3).

`dogsled status` reads the local `host.json` and fetches each peer's `host.json` over ssh, in parallel, with a short
cache (default 10 s). It never probes a peer itself. Pi, the prompt hooks, and the tab-row command all read through
this cache, so they cost one ssh read per peer per 10 s at most.

Service liveness comes from the kernel service lock, not snapshot age. Peer responses add a live `running` field without moving existing snapshot fields.
Old peers without this field retain age-based liveness until upgraded. Unreachable peers remain down.
Admission, watchdog marks and owner readiness, and idle-pane cleanup still require fresh measurements.
Finished-child and orphan cleanup use running-host snapshots, then close rechecks the live pane, `done` mark, and loss guard.
Each tick deduplicates checkout observations by directory. Independent checkout and shell-pane observations use at most `available_parallelism` workers per step.
Git tip caching remains in place, but dirty-state checks run each tick because unchanged commits do not prove an unchanged checkout.

### 2. Placement: `dogsled place`

`dogsled place [--host H] [--branch B] [--base B] [--repo R]` picks the host with the most headroom, writes a short
reservation (default 90 s) on it, and prints the host. With `--branch` it also creates the worktree on that host
with `git worktree add`, runs the repository's `dogsled.toml` `setup`, opens it as a Herdr workspace, and prints its
path and pane.

If the host has no checkout of the repository, dogsled clones it from the caller's origin into the same path relative to home first.
An existing checkout must have the same origin. Clone failures stop admission rather than silently moving work to another host.
Only a freshly cloned checkout can receive mise trust after a trust refusal.

The agent then spawns with Shepherdr: `agents spawn machine=<host> cwd=<path> …`. The reservation stops two callers
from picking the same host before the new agent shows in `herdr agent list`. It ends at its TTL (default 90 s), which covers the time until the agent is listed and its first build shows in the busy fraction.

Jobs use the same placement inside `dogsled run`.

### 3. Jobs and the queue: `dogsled run`

```
dogsled run [--host H] [--branch B] [--cwd D] [--timeout T] [--idle-timeout T] [--wait] -- <cmd> [args…]
dogsled ps | logs <id> | stop <id> | wait <id>
```

- Placement as in part 2. With `--branch`, dogsled creates the worktree on the selected host.
  Without a branch, remote work runs in `--cwd`. Non-repository directories must already exist there.
- Dogsled opens a pane where the work's repository is shown: its worktree's workspace, else the main checkout's, else a new workspace. Work outside a repository uses a tab in the caller's workspace locally, or a workspace with the same name on the remote.
- The pane runs `dogsled exec <id> -- <cmd>`, a small wrapper that:
  - reports `working` with `--agent dogsled-job` and shows the command as the agent name;
  - writes stdout and stderr to `~/.local/state/dogsled/jobs/<id>.log` and to the pane;
  - kills the command at `--timeout`, or when it prints nothing for `--idle-timeout`;
  - writes `{exit, signal, started, ended}` to `jobs/<id>.json`;
  - reports `idle` with `$dogsled = "exit 0 · 3m"`, or `blocked` with `"exit 1 · 3m"` or `"timeout"`;
  - closes its own pane with `herdr pane close <job.pane>`, which removes the job tab.
- `--wait` blocks until the job ends and exits with its code. Each harness runs it with its own background tool
  (Pi `bg_task`, Claude Code `run_in_background`, Codex background shell), so the caller wakes when the job ends.
- Without `--wait`, the call returns the job id at once. `dogsled wait <id>` can follow later.

The close can terminate `exec`, because it runs in that pane. The log, final record, and completion reports must finish first.
A failed close writes an error to stderr without changing the job record or exit code. Prune retains its ended-job-tab fallback.
`wait` and `logs` read the ledger, not the pane, so both work after the tab closes.

When every host is full, `run` and `place` put the work in the queue and print the ticket. With `--wait` they block
until the ticket is admitted (and for `run`, until the job ends). The owner's host service admits tickets in order when an eligible host has headroom.
The order holds per host: a ticket waits only behind earlier tickets that could take one of its hosts, so a full host never holds back work pinned to a host with room.
Admission uses the shared status cache and SSH transport, unlike the local-only measurement step.
`agent` also queues and supports `--wait`. All admission waits accept `--max` (default 30m).
A service interruption during admission marks that ticket failed on restart, preventing an automatic duplicate launch.

MCP `spawn` persists a ticket before startup, for both legacy requests and committed unit requests.
The admission worker starts the child under the existing setup limit, independent of the MCP call budget.
CLI agent launches remain synchronous. Startup errors retain their original classification and add recent pane output when available.
An interrupted startup can recover that output on confirmation. Diagnostic reads do not change launch receipts, holds, or closure checks.

### 4. No wait without a deadline: stalls and check-ins

Rule: an agent that owns an open child or job hears from dogsled at least every 30 min while it is idle, until that work ends. Nothing depends on the agent remembering.

**Owners.** Dogsled records the owner (host and pane) of each item:
- `dogsled run`: the caller's `$HERDR_PANE_ID`.
- `dogsled agent` (spawn for Claude Code, Codex, and scripts): the caller's pane.
- Children from elsewhere: `dogsled track <host> <pane>`.

A job item closes when its job ends. `done` validates the caller’s agent checkout while the caller can still fix it. Its mark uses launch/item, pane, and harness session identity. The first idle/done observation seals it, even without restored counters. The child host captures the reply, closes the pane and agent checkout, and retains the branch. The owner imports the durable result and shows unlanded branches under TO LAND without a live child. Only inbox work accepted during the marked working turn can cancel the mark. Later messages are refused. Idle without `done` is never closure consent.

**Stall.** An agent is stalled when Herdr shows it `working` or `unknown` and its transcript (`transcript` in host.json) has not changed for 10 min. The screen is not a signal: spinners and elapsed timers redraw it while a tool call hangs. An agent with no transcript is never marked stalled. Unit 3 confirms where Claude Code and Codex transcripts are, because Herdr may report only a session id for them.

**Check-in.** `dogsled serve` on the owner's host checks every minute. It reads the state of remote items from the peers' host.json through the status cache. For each owner on its host that is `idle` or `done`, has open items, and has an inbox reader, it puts a `check-in` record in the owner's inbox:
- at once when an item stalls or a job ends;
- otherwise 30 min after the last check-in or item event.

The note lists each item (name, host, state, age, time since its transcript changed, last 5 lines) and asks the owner to keep waiting, message the item, close it, or stop it. Serve skips owners that are `working` (the note waits until they are idle) and owners that are `blocked` (the user is in the loop). It sets `$dogsled = "stalled 10m"` on a stalled pane.

**Durability.** Events are captured before reconciliation, including between one-minute check-in deadlines. Transcript writes alone are not lifecycle events.
Normal restarts retain delivered events and last-send times. The inbox lease carries each check-in until the reader acknowledges it; nothing is typed into the owner's terminal.
Top-level notification attempts are at-most-once per stall; a crash during submission can lose that notification.
Watchdog, admission, child completion, inbox delivery, and requested cleanup have separate workers. Slow cleanup cannot block check-ins or publication.

**Hung parent.** The stall rule applies to every agent. A stalled agent that has an owner causes a check-in for that owner. A stalled top-level agent gets the sidebar mark and a Herdr notification.

**Bounded waits.** One deadline starts at command entry. `--max` (default 30m) includes discovery, locks, transport, preparation, admission, and job completion.
Routine calls and lock waits also have a 30-second limit. Setup calls (prepare, worktree removal, teardown, clone, mise trust, agent startup) have a 10-minute limit. Wait paths return `still running` and exit 124 at the deadline.
Non-waiting actions report the operation that timed out. Service admission runs separately from periodic snapshot publication. A job's `--idle-timeout` kills it after that much silence in its own output.

### 5. Status

```
dogsled status            text: hosts, agents and jobs per host, queue, stalls
dogsled status --json     the same, for scripts
dogsled status --line     one line: "local 50/12 ▲ · netcup 3/16 · momokaya-2 6/7"
dogsled status --brief    ≤ 6 lines, for the prompt hooks
```

### 6. Cleanup: `dogsled doctor` and `dogsled prune`

Doctor never removes candidates. It refreshes remote refs and builds a plan shared with prune and the sweep.

The loss rule has one owner, `src/loss.rs`. A checkout must contain no unique content, with ignored files excluded. Pure tracked deletions (` D` or `D `) are lossless only when the tip is held. Modified, added, renamed, type-changed, and untracked paths still count as dirt. Every commit must be held by the default branch or remote refs refreshed in this pass. A ref without a checkout skips the cleanliness condition. A failed fetch produces a warning and disables remote-ref evidence for that repository. Integration recognizes ancestry, squash, stack, and unchanged merge-tree results.

Squash and stack candidates must change every path in the net diff of their run. Before/after objects feed the exact three-way content check. Historical paths from merges of main or reverted edits do not enlarge the net diff. A doubtful path keeps the branch. The isolated merge-tree object store remains independent of repository attributes and configuration.

A tip also lands through a local or remote-tracking ref that contains it and passes the integration check. The result names that ref. `git for-each-ref --contains` bounds the search. Direct checks of those refs cover transitive containers without recursion or alias loops. Each `loss::Evidence` pass caches direct proofs by resolved tip and default-branch object IDs. Aliases share proofs, but changed IDs require a new proof. No cache survives the pass. Shorter runs derive net changes from loaded first-parent diffs instead of a Git subprocess for every endpoint.

Prune and the sweep use the same holds: active panes or processes, the caller, an unreadable process table, and pending preparations or launches. A preparation stops pending when its launch has a final receipt or its local preparation failed; a failed preparation also releases its own record. An open agent item holds its checkout in prune and the sweep, so only `close` ends it and keeps the reply. Automatic handoff retains the local branch, so unlanded commits do not hold the checkout. Shared checkouts release only the finished pane, in automatic and explicit close alike. A launch without a final receipt whose recorded terminal no longer runs in its pane (the pane is gone or replaced) has settled: start and recovery refuse that pane, so its preparation no longer holds. An idle or done agent that has not called `dogsled done` holds its checkout, except for an explicit `dogsled close`. Idle shells (past that age), done agents without open items, and workspaces alone are not holds. In prune and the sweep, a shell that the host snapshot reports idle for less than 15 minutes holds its checkout; a shell with no reported age does not, and an explicit `close` ignores this hold. The process check skips each holder pane's shell, and every descendant of an agent pane's shell; a shell's background job still holds. Foreign locks prevent removal.

An unheld dirty checkout whose work is in the default branch produces a `landed_dirty` finding. It includes the repository, path, branch or HEAD, dirty-path count, and up to ten paths. Unreadable repositories produce warnings without stopping other repositories. Absent remembered repositories produce removable registrations. Doctor changes no registrations.

Loss assessment and deletion recovery require the checkout's own `.git` and a Git top-level path that resolves to the checkout. Invalid identities return a path-specific refusal before status reads. They never count as clean or report a parent repository's changes. A worktree whose directory remains while Git no longer lists it, calls it prunable, or finds no `.git` file is an `unlinked` hold: never removed, never assessed for loss, and refused by `close` before anything closes.

The sweep applies only removable `worktrees`, `branches`, `workspaces`, `panes`, and absent remembered `repositories`. Orphan close uses the same loss rule. Prune also shows non-removable findings, expired jobs, and unused tagged build output. Disk and process findings are report-only. Prune never kills processes.

Both scopes use one pane rule and exclude the caller and focused panes. An empty shell closes after its job ends, its directory disappears, its launch becomes final, or its agent item closes. A shell without a Dogsled record needs 15 minutes continuously at an empty prompt. Agents retain the one-day rule and require `done`, with no open children.

Launch recovery owns final-shell checks for explicit close and cleanup. Refused, never-attempted, and unknown-outcome launches release empty panes under the request and pane locks. Recovery verifies terminal identity, agent absence, and foreground processes without changing receipts. An unfinished launch holds its checkout only before pane allocation completes or while the attempted start has its matching agent. Final launch panes close separately before worktree removal, not through a whole-workspace close.

Every removal rechecks identity and safety. Worktree removal runs teardown, restores only deletion-only paths from held HEAD, closes holders, unlocks, removes without force, and deletes a held branch. Restoration refuses other dirt and uses literal paths. Both creation and removal use the existing setup budget. A failed entry does not stop the others. A recheck refusal because state changed after the plan (now dirty, held, not landed, or busy, or its HEAD moved because of a new commit) keeps the entry and records it in `skipped`, not `failures`. A moved HEAD with the same repository, path, and branch reads `HEAD moved since the survey (<old> -> <new>)`, and the next survey re-plans the entry. The comparison of the planned tree with the fresh survey, the removal recheck, and the branch-tip check all use one rule, so the sweep and `prune` agree. A branch whose tip moved reads the same way. Identity changes (repository, path, branch, main checkout, or empty HEAD), missing entries, command errors, and unreadable evidence stay failures. Another actor can remove a worktree after the survey or the recheck lists it, before the checks that run Git inside it. A Git error there is not a failure when a fresh listing no longer has the worktree and its path is absent. The survey then plans a registration drop for a registered worktree, and the recheck drops the registration and reports the worktree as already removed. A Git error in a worktree whose directory still exists stays unreadable in the survey and a failure in the recheck. Completion-event behavior remains independent of doctor cleanup.

Stop writes a durable `sweep-request.json` and sends a local datagram to `sweep.sock`. The hook does not wait for cleanup.
The service checks the durable request at startup, on notification, and on each publication iteration. It surveys only when a request exists.
A socket bind failure produces a warning instead of stopping the service. Stop request failures do not prevent gate or inbox delivery.
Requests during a pass remain pending until the worker completes another pass. A service lock prevents concurrent service instances.
Each pass records its times, removals, failures, repository warnings, and landed-dirty findings in `sweeps.jsonl` and `sweep.json`.
History entries include `ts_ms`; each new pass removes entries older than seven days. Successful removals never trigger a gate reminder.
Status exposes the latest report, and `status --brief` summarizes it on one line. Prompt context counts only the caller's own findings.
The gate delivers findings once per recipient’s distinct set through `gate.json`.
Failures preserve their cleanup entry identity in `failure_entries`, beside the existing failure strings.
A notice reaches only the agent that created the work: the owner of an open item whose pane holds the worktree or pane, or the requester of a recorded launch into the worktree. An owner can itself be a child.
Without a creator, no agent receives the notice. Host-level failures and repository warnings stay in status.
The launch record keeps the creator after the child's pane closes, so landed-dirty findings still reach it.
Dirty-worktree guidance preserves wanted changes and requires user approval before discarding abandoned work.

Placement registers repositories in `repositories.json`, separately from worktree entries. Removal preserves repository registration.
Doctor also discovers repositories from the caller, live pane directories, Herdr workspaces, and `cleanup.repositories`. It never scans the whole home directory.
The remote default branch defines merged state, with local `main` as the fallback. Unknown merge status prevents removal.
Prune compares current Git path, branch, HEAD, and repository with the approved plan immediately before removal.
Removal runs the repository's `dogsled.toml` `teardown` in the worktree, then a non-forced `git worktree remove`. A teardown failure keeps the worktree. The branch is deleted only when the integration check finds it integrated.
Prune and the sweep remove absent remembered registrations after a locked recheck. Removal deletes no checkout data and preserves existing worktree records. Configured paths, discovered paths, unreadable paths, and dangling symlinks remain report-only.

### 7. Prompt context

| Harness | Hook | Content |
|---|---|---|
| Pi | `before_agent_start` (dogsled Pi extension) | `dogsled hook context --harness pi` |
| Claude Code | `SessionStart`, `UserPromptSubmit` (dogsled plugin) | `dogsled hook context --harness claude` |
| Codex | `SessionStart`, `UserPromptSubmit` (dogsled plugin) | `dogsled hook context --harness codex` |

The CLI reads status through the existing peer cache. It shows the owner summary before the fleet totals from `status --brief`.
Claude Code and Codex receive `hookSpecificOutput` JSON with the event name and `additionalContext`. Pi receives plain text.
Plugin registrations pass `--event` explicitly. The default is `UserPromptSubmit`. No hook reads stdin.
Without `HERDR_ENV` or snapshots, the hook prints nothing. Its total command budget is five seconds.
Failures contribute no context and exit successfully. Plugin commands also end with `|| true`.

Dogsled owns both marketplaces, both plugin manifests, and the hooks under `plugins/dogsled`.
Codex declares its hooks inline and uses `$PLUGIN_ROOT`. Claude reads `hooks/hooks.json` and uses `$CLAUDE_PLUGIN_ROOT`.
Pi, Claude Code, and Codex share the single skill under `plugins/dogsled/skills/dogsled`.
The Pi extension and Herdr manifest stay at the repository root paths. Binary archives retain the service templates.

## What is removed

- Leads, the lead claim, `⌂`/`↳` marks, and the `$sub`, `$lead`, `$queue` sidebar tokens.
- The Pi `dogsled` spawn tool. Agents use `dogsled place` and `agents spawn`. The Pi extension keeps
  prompt context only in unit 1. Later units add job and queue widgets.
- The Herdr release plugin.
- `docs/plan.md`.

## What stays

- Host policy in `~/.config/dogsled/config.toml` (thresholds, `remote_command`).
- The `prepare` hook (run in the checkout, from the pending 0.5.0 fix). The headroom formula changed in 0.8: free cores from the CPU busy fraction plus a memory reserve, replacing load1 (`docs/research/2026-10-15-admission-metric.md`).

## Units, in order

1. **dogsled 0.5: status and service.** `serve`, memory in the probe, `status --line/--brief`, peer cache, remove
   leads and the Pi spawn tool, add `place`. The service units ship in the release; dotfiles installs them.
   host.json lists each agent pane with its state, transcript path, and transcript modified time.
2. **dogsled 0.6: jobs and queue.** `run`, `exec`, `ps`, `logs`, `stop`, `wait`, queue admission by the service.
   Add `dogsled agent` (spawn any harness through `herdr agent start` and prompt it), owner records for `run` and `agent`,
   `dogsled track`, and the Pi extension tracking Shepherdr spawns. The widget lists open jobs and tickets every 10 seconds.
   Unit 2 remains in the 0.5.0 unreleased changelog. It does not bump or publish a version.
3. **dogsled 0.7: watchdog and cleanup.** Stalls and check-ins (part 4), `doctor`, `prune`.
   Proof: test A, where a child hangs in a tool call while its parent is idle.
   It passes when the parent gets a check-in naming the stall within 11 min.
   Also confirm that `herdr agent prompt` starts a turn in an idle Pi pane.
4. **dogsled standalone package.** Dogsled owns prompt hooks and one shared skill for Pi, Claude Code, and Codex.
   Add the two marketplaces, two plugin manifests, and `hook context`. Keep all package versions equal to Cargo without a version bump.
   Agent-repository delegation rules are outside this unit.
5. **dotfiles.** Tab-row command to `status --line`; sidebar rows for the new tokens; service install on each host.

Units 1–3 are sequential. Unit 4 builds on units 1 and 2. Unit 5 follows each release.

## Decisions

1. Stall: Herdr shows `working` or `unknown` and the transcript is unchanged for 10 min.
2. Wake mechanism: check-ins through `herdr agent prompt` (part 4); test A proves them in unit 3. Since 0.11, check-ins, replies, blocked reports, ticket notices, and peer messages all go through the inbox (board plan, U5b), because typing into a pane merged check-ins into the user's drafts.
3. Cleanup: the sweep closes a finished child after it captures the reply (board plan, decision 1); `dogsled close` ends one early. The `serve` sweep is the last resort: it removes only what loses nothing (part 6). Everything else waits for `dogsled prune`, which asks first.
4. Service install: dotfiles installs the launchd agent (macOS) and the systemd user unit (Linux) on every host.
5. The old 0.5.0 `prepare` PR was never pushed. Unit 1 carries its fix: the hook runs in the checkout, with no `{repo}`.
