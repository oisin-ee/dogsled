# Design: wrangle as host tooling for agents

Status: draft for review. Replaces `docs/plan.md` (the lead/orchestrator design).

## Goal

Any agent (Pi, Claude Code, Codex) and the user can:

1. See fresh load, agents, and jobs on every Herdr host.
2. Start an agent or a job on the host with the most headroom, with the same tool.
3. Queue work when every host is full.
4. Get a reliable wake-up when a child agent or job ends, stalls, or fails.
5. Find and remove cruft on a host (stale panes, orphaned worktrees, full disks).

Wrangle does not replace Shepherdr. Shepherdr keeps agent spawn and parent/child messages.
Wrangle does placement, jobs, the queue, the stall watchdog, status, and cleanup.

## Where the user and agents see it

| What | Where | Source |
|---|---|---|
| All agents on all hosts | Herdr sidebar (already built in) | Herdr integrations |
| Jobs | Herdr sidebar, one row per job | wrangle `report-agent` |
| Notes on a pane ("stalled 10m", "exit 1 · 3m") | Sidebar `$wrangle` token | wrangle `report-metadata` |
| Load per host | Tab-row status area | `wrangle status --line` (dotfiles #31 uses `--json` + jq until then) |
| Agents this session started | Shepherdr widget in Pi | Shepherdr |
| Jobs and queue tickets this session started | wrangle widget in Pi | wrangle |
| Everything, for agents | `wrangle status [--json]` and a short block added to each prompt | wrangle |

## Parts

### 1. Host service: `wrangle serve`

One process per host, started by launchd (macOS) or a systemd user unit (Linux). It does local work only. It uses SSH only for owner-state reads and queue admission. The measurement step remains local.

Every N seconds (default 10) it:

- Probes the host: load1, cores, memory used, disk free, Herdr agents and panes (`herdr agent list`, `pane list`).
- Writes the probe to `~/.local/state/wrangle/host.json`.
- Runs the stall watchdog on this host's panes (part 4).
- Starts queued work on an eligible host when it has headroom (part 3).

`wrangle status` reads the local `host.json` and fetches each peer's `host.json` over ssh, in parallel, with a short
cache (default 10 s). It never probes a peer itself. Pi, the prompt hooks, and the tab-row command all read through
this cache, so they cost one ssh read per peer per 10 s at most.

### 2. Placement: `wrangle place`

`wrangle place [--host H] [--branch B] [--base B] [--repo R]` picks the host with the most headroom, writes a short
reservation (default 90 s) on it, and prints the host. With `--branch` it also creates the worktree on that host
(the prepare hook, now run in the checkout) and prints its path.

If the host has no checkout of the repository, wrangle clones it from the caller's origin into the same path relative to home first.
An existing checkout must have the same origin. Clone failures stop admission rather than silently moving work to another host.
Only a freshly cloned checkout can receive mise trust after a trust refusal.

The agent then spawns with Shepherdr: `agents spawn machine=<host> cwd=<path> …`. The reservation stops two callers
from picking the same host before the new agent shows in `herdr agent list`. It ends at its TTL (default 90 s), which covers the lag of the 1-minute load average.

Jobs use the same placement inside `wrangle run`.

### 3. Jobs and the queue: `wrangle run`

```
wrangle run [--host H] [--branch B] [--cwd D] [--timeout T] [--idle-timeout T] [--wait] -- <cmd> [args…]
wrangle ps | logs <id> | stop <id> | wait <id>
```

- Placement as in part 2. With `--branch`, the prepare hook creates the worktree on the selected host.
  Without a branch, remote work runs in `--cwd`. Non-repository directories must already exist there.
- Wrangle opens a pane: a tab in the caller's workspace locally, or a workspace with the same name on the remote.
- The pane runs `wrangle exec <id> -- <cmd>`, a small wrapper that:
  - reports `working` with `--agent wrangle-job` and shows the command as the agent name;
  - writes stdout and stderr to `~/.local/state/wrangle/jobs/<id>.log` and to the pane;
  - kills the command at `--timeout`, or when it prints nothing for `--idle-timeout`;
  - writes `{exit, signal, started, ended}` to `jobs/<id>.json`;
  - reports `idle` with `$wrangle = "exit 0 · 3m"`, or `blocked` with `"exit 1 · 3m"` or `"timeout"`.
- `--wait` blocks until the job ends and exits with its code. Each harness runs it with its own background tool
  (Pi `bg_task`, Claude Code `run_in_background`, Codex background shell), so the caller wakes when the job ends.
- Without `--wait`, the call returns the job id at once. `wrangle wait <id>` can follow later.

When every host is full, `run` and `place` put the work in the queue and print the ticket. With `--wait` they block
until the ticket is admitted (and for `run`, until the job ends). The owner's host service admits tickets in order when an eligible host has headroom.
Admission uses the shared status cache and SSH transport, unlike the local-only measurement step.
`agent` also queues and supports `--wait`. All admission waits accept `--max` (default 30m).
A service interruption during admission marks that ticket failed on restart, preventing an automatic duplicate launch.

### 4. No wait without a deadline: stalls and check-ins

Rule: an agent that owns an open child or job hears from wrangle at least every 30 min while it is idle, until that work ends. Nothing depends on the agent remembering.

**Owners.** Wrangle records the owner (host and pane) of each item:
- `wrangle run`: the caller's `$HERDR_PANE_ID`.
- `wrangle agent` (spawn for Claude Code, Codex, and scripts): the caller's pane.
- Pi children spawned with Shepherdr's `agents` tool: the wrangle Pi extension reads the spawn tool result and runs `wrangle track <machine> <pane>` with its own pane as owner.

An item closes when its job ends, when its agent is `idle` or `done` (Shepherdr delivers that result), or when its pane closes.

**Stall.** An agent is stalled when Herdr shows it `working` or `unknown` and its transcript (`transcript` in host.json) has not changed for 10 min. The screen is not a signal: spinners and elapsed timers redraw it while a tool call hangs. An agent with no transcript is never marked stalled. Unit 3 confirms where Claude Code and Codex transcripts are, because Herdr may report only a session id for them.

**Check-in.** `wrangle serve` on the owner's host checks every minute. It reads the state of remote items from the peers' host.json through the status cache. For each owner on its host that is `idle` or `done` and has open items, it runs `herdr agent prompt <owner> "<note>"`:
- at once when an item stalls or a job ends;
- otherwise 30 min after the last check-in or item event.

The note lists each item (name, host, state, age, time since its transcript changed, last 5 lines) and asks the owner to keep waiting, nudge the item, or stop it. Serve skips owners that are `working` (the note waits until they are idle) and owners that are `blocked` (the user is in the loop). It sets `$wrangle = "stalled 10m"` on a stalled pane.

**Durability.** Events are captured before reconciliation, including between one-minute check-in deadlines. Transcript writes alone are not lifecycle events.
Normal restarts retain delivered events and last-send times. Herdr has no prompt idempotency key, so crash-safe exactly-once delivery is unavailable.
Each prompt attempt is persisted before submission. Interrupted or failed attempts remain visible and retry after 30 minutes.
This can duplicate one delayed check-in after an ambiguous acceptance, but avoids restart spam and silent permanent loss.
Top-level notification attempts are at-most-once per stall; a crash during submission can lose that notification.
Watchdog and admission have separate action workers, so slow setup cannot block check-ins or publication.

**Hung parent.** The stall rule applies to every agent. A stalled agent that has an owner causes a check-in for that owner. A stalled top-level agent gets the sidebar mark and a Herdr notification.

**Bounded waits.** One deadline starts at command entry. `--max` (default 30m) includes discovery, locks, transport, preparation, admission, and job completion.
Routine calls and lock waits also have a 30-second limit. Setup calls (prepare, clone, mise trust, agent startup) have a 10-minute limit. Wait paths return `still running` and exit 124 at the deadline.
Non-waiting actions report the operation that timed out. Service admission runs separately from periodic snapshot publication. A job's `--idle-timeout` kills it after that much silence in its own output.

### 5. Status

```
wrangle status            text: hosts, agents and jobs per host, queue, stalls
wrangle status --json     the same, for scripts
wrangle status --line     one line: "local 50/12 ▲ · netcup 3/16 · momokaya-2 6/7"
wrangle status --brief    ≤ 6 lines, for the prompt hooks
```

### 6. Cleanup: `wrangle doctor` and `wrangle prune`

`wrangle doctor [--host H]` lists cruft and changes nothing:

- worktrees whose branch is merged or deleted and have no live pane;
- panes whose agent has been `done` or `idle` for more than 24 h;
- wrangle jobs that ended more than 24 h ago, with their logs;
- disk above 90 % used, with the largest wrangle-owned paths;
- processes using more than one core that are not under any Herdr pane (report only).

`wrangle serve` removes worktrees without a question only when their branch is merged or deleted, the worktree is clean,
and no live pane uses it. Every other class keeps the question in `wrangle prune [--host H] [<class>…]`.
`prune` shows what it will remove, asks, then removes it. It never kills processes.
Prepare registers repositories in `repositories.json`, separately from worktree entries. Removal preserves repository registration. Hourly automatic cleanup discovers their current Git worktrees, rather than trusting saved paths.
Doctor also discovers repositories from the caller and live pane directories. Neither scans the whole home directory.
This bounds automatic ownership: unrelated repositories and detached worktrees remain outside automatic cleanup.
The remote default branch defines merged state, with local `main` as the fallback. Unknown merge status prevents removal.
Prune compares current Git path, branch, HEAD, and repository with the approved plan immediately before removal.
A branch-taking repository task cannot guarantee an exact path. Until a task exposes a verifiable path-bound contract, removal uses non-forced `git worktree remove -- <path>`.
Missing or unreadable disk paths remain report-only findings without discarding other candidates.

### 7. Prompt context

| Harness | Hook | Content |
|---|---|---|
| Pi | `before_agent_start` (wrangle Pi extension) | `wrangle hook context --harness pi` |
| Claude Code | `SessionStart`, `UserPromptSubmit` (wrangle plugin) | `wrangle hook context --harness claude` |
| Codex | `SessionStart`, `UserPromptSubmit` (wrangle plugin) | `wrangle hook context --harness codex` |

The CLI reads status through the existing peer cache and formats `status --brief` for each harness.
Claude Code and Codex receive `hookSpecificOutput` JSON with the event name and `additionalContext`. Pi receives plain text.
Plugin registrations pass `--event` explicitly. The default is `UserPromptSubmit`. No hook reads stdin.
Without `HERDR_ENV` or snapshots, the hook prints nothing. Its total command budget is five seconds.
Failures contribute no context and exit successfully. Plugin commands also end with `|| true`.

Wrangle owns both marketplaces, both plugin manifests, and the hooks under `plugins/wrangle`.
Codex declares its hooks inline and uses `$PLUGIN_ROOT`. Claude reads `hooks/hooks.json` and uses `$CLAUDE_PLUGIN_ROOT`.
Pi, Claude Code, and Codex share the single skill under `plugins/wrangle/skills/wrangle`.
The Pi extension and Herdr manifest stay at the repository root paths. Binary archives retain the service templates.

## What is removed

- Leads, the lead claim, `⌂`/`↳` marks, and the `$sub`, `$lead`, `$queue` sidebar tokens.
- The Pi `wrangle` spawn tool. Agents use `wrangle place` and `agents spawn`. The Pi extension keeps
  prompt context only in unit 1. Later units add job and queue widgets.
- The Herdr release plugin.
- `docs/plan.md`.

## What stays

- Host policy in `~/.config/wrangle/config.toml` (thresholds, `remote_command`).
- The headroom formula and the `prepare` hook (run in the checkout, from the pending 0.5.0 fix).

## Units, in order

1. **wrangle 0.5: status and service.** `serve`, memory in the probe, `status --line/--brief`, peer cache, remove
   leads and the Pi spawn tool, add `place`. The service units ship in the release; dotfiles installs them.
   host.json lists each agent pane with its state, transcript path, and transcript modified time.
2. **wrangle 0.6: jobs and queue.** `run`, `exec`, `ps`, `logs`, `stop`, `wait`, queue admission by the service.
   Add `wrangle agent` (spawn any harness through `herdr agent start` and prompt it), owner records for `run` and `agent`,
   `wrangle track`, and the Pi extension tracking Shepherdr spawns. The widget lists open jobs and tickets every 10 seconds.
   Unit 2 remains in the 0.5.0 unreleased changelog. It does not bump or publish a version.
3. **wrangle 0.7: watchdog and cleanup.** Stalls and check-ins (part 4), `doctor`, `prune`.
   Proof: test A, where a child hangs in a tool call while its parent is idle.
   It passes when the parent gets a check-in naming the stall within 11 min.
   Also confirm that `herdr agent prompt` starts a turn in an idle Pi pane.
4. **wrangle standalone package.** Wrangle owns prompt hooks and one shared skill for Pi, Claude Code, and Codex.
   Add the two marketplaces, two plugin manifests, and `hook context`. Keep all package versions equal to Cargo without a version bump.
   Agent-repository delegation rules are outside this unit.
5. **dotfiles.** Tab-row command to `status --line`; sidebar rows for the new tokens; service install on each host.

Units 1–3 are sequential. Unit 4 builds on units 1 and 2. Unit 5 follows each release.

## Decisions

1. Stall: Herdr shows `working` or `unknown` and the transcript is unchanged for 10 min.
2. Wake mechanism: check-ins through `herdr agent prompt` (part 4); test A proves them in unit 3.
3. Prune: `wrangle serve` removes clean worktrees without a question when their branch is merged or deleted and no live pane uses them. Every other class keeps the question in `wrangle prune`.
4. Service install: dotfiles installs the launchd agent (macOS) and the systemd user unit (Linux) on every host.
5. The old 0.5.0 `prepare` PR was never pushed. Unit 1 carries its fix: the hook runs in the checkout, with no `{repo}`.
