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

One process per host, started by launchd (macOS) or a systemd user unit (Linux). It does local work only. It does
not use ssh.

Every N seconds (default 10) it:

- Probes the host: load1, cores, memory used, disk free, Herdr agents and panes (`herdr agent list`, `pane list`).
- Writes the probe to `~/.local/state/wrangle/host.json`.
- Runs the stall watchdog on this host's panes (part 4).
- Starts queued work for this host when it has headroom (part 3).

`wrangle status` reads the local `host.json` and fetches each peer's `host.json` over ssh, in parallel, with a short
cache (default 10 s). It never probes a peer itself. Pi, the prompt hooks, and the tab-row command all read through
this cache, so they cost one ssh read per peer per 10 s at most.

### 2. Placement: `wrangle place`

`wrangle place [--host H] [--branch B] [--repo R]` picks the host with the most headroom, writes a short
reservation (default 60 s) on it, and prints the host. With `--branch` it also creates the worktree on that host
(the prepare hook, now run in the checkout) and prints its path.

The agent then spawns with Shepherdr: `agents spawn machine=<host> cwd=<path> …`. The reservation stops two callers
from picking the same host before the new agent shows in `herdr agent list`. It ends when the agent appears, or at
the TTL.

Jobs use the same placement inside `wrangle run`.

### 3. Jobs and the queue: `wrangle run`

```
wrangle run [--host H] [--branch B] [--cwd D] [--timeout T] [--idle-timeout T] [--wait] -- <cmd> [args…]
wrangle ps | logs <id> | stop <id> | wait <id>
```

- Placement as in part 2. A remote job needs a pushed branch: wrangle creates the worktree on that host from it.
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
until the ticket is admitted (and for `run`, until the job ends). The host service admits tickets in order when its
host has headroom.

### 4. Stall watchdog

The host service watches every pane on its host that is `working` or `unknown`. When the pane output does not change
for the stall time (default 10 min), it:

1. Sets `$wrangle = "stalled 10m"` on the pane, so the sidebar shows it.
2. Wakes whoever waits for that pane. **Open: the mechanism.** Two choices, decided by test A:
   - (a) `pane report-agent <pane> --source custom:wrangle --state blocked`, if Shepherdr then wakes the parent.
   - (b) `herdr agent prompt <parent pane> "<child> stalled 10m: <last lines>"`, if (a) does not wake it.

For a wrangle job, `--idle-timeout` kills it, and its wrapper reports the result. The watchdog is for agents.

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

`wrangle prune [--host H] [<class>…]` shows what it will remove, asks, then removes it. It never kills processes.

### 7. Prompt context

| Harness | Hook | Content |
|---|---|---|
| Pi | `before_agent_start` (wrangle Pi extension) | `wrangle status --brief` |
| Claude Code | `UserPromptSubmit` (agent repo lifecycle runner) | same |
| Codex | `UserPromptSubmit` (agent repo lifecycle runner) | same |

## What is removed

- Leads, the lead claim, `⌂`/`↳` marks, and the `$sub`, `$lead`, `$queue` sidebar tokens.
- The Pi `wrangle` spawn tool. Agents use `wrangle place` and `agents spawn`. The Pi extension keeps the widget and
  the prompt context only.
- The Herdr release plugin.
- `docs/plan.md`.

## What stays

- Host policy in `~/.config/wrangle/config.toml` (thresholds, `remote_command`).
- The headroom formula and the `prepare` hook (run in the checkout, from the pending 0.5.0 fix).

## Units, in order

1. **wrangle 0.5: status and service.** `serve`, memory in the probe, `status --line/--brief`, peer cache, remove
   leads and the Pi spawn tool, add `place`. The service units ship in the release; dotfiles installs them.
2. **wrangle 0.6: jobs and queue.** `run`, `exec`, `ps`, `logs`, `stop`, `wait`, queue admission by the service.
3. **wrangle 0.7: watchdog and cleanup.** Stall watchdog (after test A), `doctor`, `prune`.
4. **agent repo.** Prompt hooks for 3 harnesses, the `wrangle` skill, `20-delegation.md` lines 21–37 and 70–72.
5. **dotfiles.** Tab-row command to `status --line`; sidebar rows for the new tokens; service install on each host.

Units 1–3 are sequential. Unit 4 can start after 1. Unit 5 follows each release.

## Open decisions

1. Stall time default: 10 min?
2. Wake mechanism for stalls: test A decides between (a) and (b).
3. Should `prune` ever run without a question (for example, merged worktrees with no pane)?
4. Service install: dotfiles (macOS launchd and Linux systemd user unit), or the infra repo for the Linux hosts?
5. Drop the pending 0.5.0 `prepare` PR, and fold its fix into unit 1?
