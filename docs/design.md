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
- Dogsled opens a pane: a tab in the caller's workspace locally, or a workspace with the same name on the remote.
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
Admission uses the shared status cache and SSH transport, unlike the local-only measurement step.
`agent` also queues and supports `--wait`. All admission waits accept `--max` (default 30m).
A service interruption during admission marks that ticket failed on restart, preventing an automatic duplicate launch.

### 4. No wait without a deadline: stalls and check-ins

Rule: an agent that owns an open child or job hears from dogsled at least every 30 min while it is idle, until that work ends. Nothing depends on the agent remembering.

**Owners.** Dogsled records the owner (host and pane) of each item:
- `dogsled run`: the caller's `$HERDR_PANE_ID`.
- `dogsled agent` (spawn for Claude Code, Codex, and scripts): the caller's pane.
- Children from elsewhere: `dogsled track <host> <pane>`.

A job item closes when its job ends. An agent item closes when its pane closes, or when the sweep closes it: once its turn completes, the sweep captures the final reply, puts it in the owner's inbox, and closes a clean reader or a writer whose branch landed. A writer with commits to land stays open under TO LAND.

**Stall.** An agent is stalled when Herdr shows it `working` or `unknown` and its transcript (`transcript` in host.json) has not changed for 10 min. The screen is not a signal: spinners and elapsed timers redraw it while a tool call hangs. An agent with no transcript is never marked stalled. Unit 3 confirms where Claude Code and Codex transcripts are, because Herdr may report only a session id for them.

**Check-in.** `dogsled serve` on the owner's host checks every minute. It reads the state of remote items from the peers' host.json through the status cache. For each owner on its host that is `idle` or `done`, has open items, and has an inbox reader, it puts a `check-in` record in the owner's inbox:
- at once when an item stalls or a job ends;
- otherwise 30 min after the last check-in or item event.

The note lists each item (name, host, state, age, time since its transcript changed, last 5 lines) and asks the owner to keep waiting, message the item, close it, or stop it. Serve skips owners that are `working` (the note waits until they are idle) and owners that are `blocked` (the user is in the loop). It sets `$dogsled = "stalled 10m"` on a stalled pane.

**Durability.** Events are captured before reconciliation, including between one-minute check-in deadlines. Transcript writes alone are not lifecycle events.
Normal restarts retain delivered events and last-send times. The inbox lease carries each check-in until the reader acknowledges it; nothing is typed into the owner's terminal.
Top-level notification attempts are at-most-once per stall; a crash during submission can lose that notification.
Watchdog, admission, and the sweep have separate action workers, so slow setup or cleanup cannot block check-ins or publication.

**Hung parent.** The stall rule applies to every agent. A stalled agent that has an owner causes a check-in for that owner. A stalled top-level agent gets the sidebar mark and a Herdr notification.

**Bounded waits.** One deadline starts at command entry. `--max` (default 30m) includes discovery, locks, transport, preparation, admission, and job completion.
Routine calls and lock waits also have a 30-second limit. Setup calls (prepare, clone, mise trust, agent startup) have a 10-minute limit. Wait paths return `still running` and exit 124 at the deadline.
Non-waiting actions report the operation that timed out. Service admission runs separately from periodic snapshot publication. A job's `--idle-timeout` kills it after that much silence in its own output.

### 5. Status

```
dogsled status            text: hosts, agents and jobs per host, queue, stalls
dogsled status --json     the same, for scripts
dogsled status --line     one line: "local 50/12 ▲ · netcup 3/16 · momokaya-2 6/7"
dogsled status --brief    ≤ 6 lines, for the prompt hooks
```

### 6. Cleanup: `dogsled doctor` and `dogsled prune`

`dogsled doctor [--host H]` lists cruft and changes nothing:

- worktrees whose branch is merged, squashed, or deleted, or has had no commits for 24 h, and have no live pane;
- unused worktrees with unmerged commits (report only);
- detached worktrees, removable when clean, unheld, and their HEAD is on a remote branch;
- local branches without a worktree that are integrated or fully on a remote, and the others (report only);
- workspaces whose every pane is idle;
- shells at an empty prompt in an ended job's tab or in a deleted directory, at once;
- panes whose agent has been `done` or `idle`, or whose shell has run no command, for more than 24 h;
- dogsled jobs that ended more than 24 h ago, with their logs;
- build output (`targets`): a directory tagged with `CACHEDIR.TAG` directly under a checkout or worktree that no workspace, pane, or process uses, such as Rust's `target/`;
- disk above 90 % used, with the largest dogsled-owned paths;
- processes using more than one core that are not under any Herdr pane (report only).

Cleanup is a closed loop first. The lead that started a child closes it with `dogsled close` when it is finished, and a job closes its own tab.
`serve` runs a last-resort sweep on its own action worker, beside the watchdog and admission. Publication stays measurement only.
Each pass builds the doctor plan in sweep scope and applies only entries whose removal loses nothing:

- ended job tabs and shells in deleted directories (the immediate pane rules), and a workspace made only of them;
- clean worktrees that nothing holds (no pane, workspace, or process) whose branch is integrated, and clean, unheld detached worktrees whose HEAD is on a remote;
- local branches without a worktree whose every commit is on a remote ref;
- orphaned children: an open agent item whose owner pane is gone, whose child is idle or done, and whose worktree is clean and pushed. The sweep closes it as `dogsled close` would (teardown, workspace, worktree, item); a remote child closes through `host close --pushed`.

A dirty, unpushed, working, or blocked orphan stays. Status, the brief, and each check-in on the host report it from `orphans.json`.
The sweep never touches the service's own pane, workspace, or working directory. It takes the same per-worktree checks as `close` and `prune`, journals each removal on stderr, and continues past a failed entry.
Every other class waits for `dogsled prune [--host H] [<class>…]`, which shows what it will remove, asks, then removes it: idle shells and agents after a day, worktrees with no commits after a day, integrated but unpushed branches, jobs, stale registrations, and `targets`. Disk and processes are report-only. Prune never kills processes.
Placement registers repositories in `repositories.json`, separately from worktree entries. Removal preserves repository registration.
Doctor also discovers repositories from the caller, live pane directories, Herdr workspaces, and `cleanup.repositories`. It never scans the whole home directory.
The remote default branch defines merged state, with local `main` as the fallback. Unknown merge status prevents removal.
Prune compares current Git path, branch, HEAD, and repository with the approved plan immediately before removal.
Removal runs the repository's `dogsled.toml` `teardown` in the worktree, then a non-forced `git worktree remove`. A teardown failure keeps the worktree. The branch is deleted only when the integration check finds it integrated.
Missing or unreadable disk paths remain report-only findings without discarding other candidates.

### 7. Prompt context

| Harness | Hook | Content |
|---|---|---|
| Pi | `before_agent_start` (dogsled Pi extension) | `dogsled hook context --harness pi` |
| Claude Code | `SessionStart`, `UserPromptSubmit` (dogsled plugin) | `dogsled hook context --harness claude` |
| Codex | `SessionStart`, `UserPromptSubmit` (dogsled plugin) | `dogsled hook context --harness codex` |

The CLI reads status through the existing peer cache and formats `status --brief` for each harness.
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
