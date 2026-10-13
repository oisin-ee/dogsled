# wrangle

Host status and placement for coding agents across [Herdr](https://herdr.dev) hosts.

Wrangle measures hosts, places jobs and agents, and queues work. Shepherdr keeps parent/child messages.
This branch contains the unreleased 0.5 redesign. Published 0.4 binaries do not support these commands.

## Install

Install the binary on each Herdr host. Replace `vX` with the same release tag in each command.
The plugin layout and context hook below are unreleased. The published `v0.4.0` tag does not contain them.

Binary:

```sh
mise use github:oisin-ee/wrangle@vX
```

Pi (extension and skill):

```sh
pi install git:github.com/oisin-ee/wrangle@vX
```

Claude Code (hooks and skill):

```sh
claude plugin marketplace add oisin-ee/wrangle
claude plugin install wrangle@wrangle
```

Codex (hooks and skill):

```sh
codex plugin marketplace add oisin-ee/wrangle --ref vX
codex plugin add wrangle@wrangle
```

Codex requires review and trust before it runs plugin hooks. See the [Codex hooks documentation](https://developers.openai.com/codex/hooks#plugin-hooks).
The [Codex CLI reference](https://developers.openai.com/codex/cli/reference#codex-plugin) describes plugin installation.

Herdr (status-board popup):

```sh
herdr plugin install oisin-ee/wrangle --ref vX
```

Harness plugins use the binary on `PATH`. They do not install it or start host services.

## Commands

```sh
wrangle serve                     # local measurements every 10 seconds
wrangle serve --interval 20        # change the interval, in seconds
wrangle serve --once               # publish one snapshot, then exit
wrangle status                    # hosts, agents, and reservations
wrangle status --json
wrangle status --line              # one line for the tab row
wrangle status --brief             # at most six lines for prompt hooks
wrangle status --watch             # refresh until interrupted
wrangle place --json               # reserve the host with the most headroom
wrangle place --host netcup --json # pin by host id or label
wrangle place --branch feat/unit-a --base main --repo /path/to/repo --json
```

Exit codes: 0 success, 1 queued without waiting, 2 error. Job waits return the job's exit code.
When every host is full, `place`, `agent`, and `run` print `{"queued":true,"ticket":"…"}`.
`--wait --max 30m` waits for admission. A wait that reaches its deadline prints `still running` and exits 124.

## Jobs, agents, and owners

```sh
wrangle run --wait -- sh -c 'echo hi; exit 3'
wrangle run --host netcup --cwd /tmp --timeout 1h --idle-timeout 10m -- make test
wrangle run --branch feat/job --base main --cwd /path/to/repo -- cargo test
wrangle ps --all
wrangle logs JOB --follow
wrangle stop JOB
wrangle wait JOB --max 30m
wrangle agent --kind codex --name reviewer --message 'Review the change' --branch feat/review --base main
wrangle track netcup w1:p2 --name child
wrangle items --owner "$HERDR_PANE_ID" --json
wrangle cancel TICKET
```

Jobs use a new tab in the caller's workspace. Agents can use the pane returned by the prepare hook.
Remote work uses a workspace with the same name as the caller's workspace, creating it when absent.
Without a branch, a remote non-repository `--cwd` must exist there.
Repository directories map to the same path relative to home. A linked-worktree caller needs `--branch` to preserve its branch remotely.

The pane runs `wrangle exec ID -- COMMAND`. Output goes to the pane and `jobs/ID.log` on that host.
If the pane output closes, capture continues to the log and preserves the child exit code.
The atomic `jobs/ID.json` contains the command, directory, host, owner, exit code, signal, reason, and millisecond timestamps.
`host.json` includes job records. `ps --all` includes completed jobs and reads remote records through the snapshot cache.
`logs`, `stop`, and `wait` use the configured SSH transport for remote jobs.
`stop` requests a process-group kill. Timeouts and wrapper termination also kill the process group.
Both timeout defaults are unlimited. Durations accept integer `ms`, `s`, `m`, and `h` suffixes.
The sidebar shows `working`, then `idle` for success or `blocked` for failure, with a `wrangle` exit or timeout token.

`--max` sets one deadline from command startup, including discovery, placement, admission, and job completion. It defaults to 30 minutes.
Operations use the remaining command budget, capped at 30 seconds for routine calls and locks, or 10 minutes for setup.
Setup includes prepare hooks, repository clones, mise trust checks, and agent startup, locally and through SSH.
A non-waiting action reports the operation that exceeded its deadline.
`logs --follow` can continue indefinitely, but each remote read has a 30-second limit.
Run long waits through the harness's background-task facility.
A wait deadline does not cancel its job or ticket. `cancel` only cancels pending tickets.

The owner's host stores locked `items.json` and `queue.json` files.
An item records its host, pane, owner pane, creation time, and closure time.
`items --json` returns `{items,queue}`. `--owner` filters both collections.
The service closes items after job completion, agent `idle` or `done`, or pane removal.
It ignores stale snapshots and snapshots older than the item's creation.

The service admits tickets in FIFO order using the same placement policy.
It records an admission error rather than silently choosing another host after a clone, prepare, or launch failure.
A service interruption during admission leaves an explicit failed ticket for inspection, not an automatic duplicate launch.
For non-waiting callers, admission prompts the owner only in `idle` or `done`. Other states defer the prompt.
`status` lists open items per owner and pending tickets. `--brief` includes their counts.

## Host service and status

Run `wrangle serve` on every host. Dotfiles owns launchd and systemd user-service installation.
The measurement step uses no SSH. It reads load, cores, memory, home disk space, Herdr agents, and Herdr panes.
When it has owned items or tickets, the service reads peers through the status cache and can launch admitted work through SSH.
A separate action worker keeps slow admission independent from the publication interval. Routine calls and lock waits have 30-second limits. Setup calls have a 10-minute limit.
An admission timeout records a failed ticket. The service continues publication and later actions.
It writes `~/.local/state/wrangle/host.json` through atomic replacement.
Each `probe.agent_panes` entry contains `pane`, `agent`, `state`, `name`, `transcript`, and `transcript_modified`.
Transcript paths come from Herdr path sessions. Their modification times use Unix milliseconds. Unavailable paths or times are null.
The service stops with an error if a measurement fails. The last complete snapshot remains available.

On Linux, the smallest finite `memory.max` from the process cgroup through its ancestors governs memory.
Used memory is that scope's `memory.current` minus `inactive_file` from `memory.stat`.
Without a finite limit, memory comes from `/proc/meminfo`: total minus `MemAvailable`.
On macOS, memory comes from `vm_stat` and `sysctl hw.memsize`.
Memory is informational. It does not change the headroom formula.

`status` reads the local snapshot and each enabled Herdr peer's snapshot.
Peer reads run in parallel through SSH and the configured `remote_command`.
A shared, locked cache permits at most one read per peer per ten seconds, including concurrent callers.
Status never probes peers itself.

A missing snapshot, failed read, or snapshot older than 30 seconds shows `service: down`.
The last peer measurement remains visible when available. Every discovered host stays in the output.
The JSON output contains `hosts`, with service state, reachability, sample time, probe, headroom, and errors.

Example tab-row output:

```text
local 50/12 ▲ · netcup 3/16 · momokaya-2 6/7
```

`▲` means not eligible. `?` means the snapshot is missing, stale, or unavailable.
A host without measurements prints `momokaya-2 ?`. A stale host keeps its last numbers, followed by ` ?`.
`--brief` includes load/cores, memory, disk, agent counts, and a hint to run `wrangle status`.
Fleets larger than five hosts share lines to preserve both the six-line limit and visibility of every host.
`--watch --interval N` changes the refresh interval. Its default is five seconds.

## Placement

Headroom remains:

```text
load_per_core_max × cores − load1 − reservation_load × reservations
```

A host needs a fresh service snapshot, positive headroom, and at least the configured minimum free disk percentage.
The host with most headroom wins. Ties use fewer live agents, then host id.
The selected host rechecks reservations under its own lock before it accepts the placement.

`place` writes a reservation with a default TTL of 90 seconds.
It returns the host id and label. JSON also includes the reservation id.
With `--branch`, it runs the prepare hook and returns the hook's `path` and `pane_id`, when present.
Then start the agent through Shepherdr, using that host and prepared pane or path.
`place` does not start an agent or claim a repository lead. `run` and `agent` perform placement and launch together.

A reservation ends only at its TTL, which covers the lag of the one-minute load average.
Agent discovery and prepare failures do not end it early.

## Configuration

The optional host policy lives in `~/.config/wrangle/config.toml`.
`--config PATH` or `$WRANGLE_CONFIG` selects another file.
Unknown keys are errors.

```toml
load_per_core_max = 1.5
disk_free_min_percent = 15
reservation_load = 1.0
reservation_ttl_ms = 90000
remote_command = ["mise", "x", "--", "wrangle"]

[hooks]
prepare = ["mise", "run", "-q", "agent:worktree", "--", "{branch}", "{base}"]
```

The prepare hook belongs only to host policy. Repository setup belongs to the repository's Worktrunk configuration.

The hook runs with the main checkout as its working directory, including calls from a linked worktree.
Before repository work, wrangle verifies the selected host's checkout against the caller's `git remote get-url origin`.
If the host has no checkout of the repository, wrangle clones it from the caller's origin into the same path relative to home first.
An existing path with a different origin, or a non-repository path, is an error. Wrangle does not overwrite it.
Clone errors identify the host and origin. Wrangle does not silently move the work elsewhere.
After a fresh clone, wrangle checks mise configuration. If mise reports missing trust, wrangle trusts only that new checkout's configuration.
For peers, a checkout under the local home maps to the same relative path under the peer's home.
The peer reads its own host policy.
Checkouts outside the home keep their absolute path.
Hooks substitute `{branch}` and `{base}` literally, without interpreting placeholders inside their values.
When `--base` is absent, a word that is only `{base}` is omitted. Other placeholders produce an error that names them.
Prefer an argument list over shell interpolation.

The hook must print JSON with `pane_id`, `path`, or both. `workspace_id` is optional.
Progress text can precede a final JSON line.

State defaults to `$XDG_STATE_HOME/wrangle`, or `~/.local/state/wrangle`.
`$WRANGLE_STATE` selects a complete state-directory path.
Host snapshots use `host.json`. New placement reservations use `placements.json`.
Jobs use `jobs/`, owner items use `items.json`, and FIFO tickets use `queue.json`.
The removed lead ledger and event log are not read or migrated.

## Prompt context and integrations

`wrangle hook context --harness claude|codex|pi` formats the same text as `status --brief`.
Claude Code and Codex register `SessionStart` and `UserPromptSubmit` hooks in `plugins/wrangle`.
Each registration supplies `--event SessionStart` or `--event UserPromptSubmit`. The default event is `UserPromptSubmit`.
The CLI never reads stdin. Claude Code and Codex receive `hookSpecificOutput` JSON with `hookEventName` and `additionalContext`.
Pi receives plain text through its `before_agent_start` extension. All three use the same snapshot reader and formatter.

Without `HERDR_ENV`, service snapshots, or a usable binary, the hooks add nothing.
A hook has a five-second command budget. Hook failures exit successfully without output.
Plugin commands end with `|| true`. Pi also leaves the prompt unchanged if the binary fails.

The Pi extension calls `wrangle hook context --harness pi` before each prompt.
It tracks successful Shepherdr `agents` spawn results through `wrangle track`.
An above-editor widget lists this session's open jobs and pending tickets. It refreshes every ten seconds and after tool results.
An empty list removes the widget. Shutdown stops the timer. Widget and tracking failures still appear as extension errors.
`$WRANGLE_BIN` selects the binary used by the extension.

The Herdr plugin keeps only the status-board popup:

```sh
herdr plugin pane open --plugin wrangle --entrypoint board
```

The popup runs `wrangle status --watch`. The pane-close release hooks and sidebar token configuration are removed.
All three harness packages share `plugins/wrangle/skills/wrangle/SKILL.md`. It describes placement, jobs, and queue admission.

## Build and development

Build this branch with the repository's pinned Rust toolchain:

```sh
cargo build --release
npm ci
```

The executable is `target/release/wrangle`.
Release archives include the binary, `wrangle.service`, and `dev.wrangle.plist` from `plugin/services/`.
Harness installers read their manifests and skill from Git, not the binary archive. The skill move does not change the archive contents.
Both run `wrangle serve` through `~/.local/bin/mise`, restart on failure, and log to the wrangle state directory.
Dotfiles installs the systemd user unit or renders the launchd template with an absolute, XML-escaped `@HOME@` path.
For launchd, dotfiles creates `~/.local/state/wrangle` before loading the agent. Systemd creates it through `StateDirectory`.
If mise has a different installation path, dotfiles adjusts the executable path.
Install the same release on every host before enabling its service. This PR does not install or start host services.

The pre-commit hook runs `mise run check`: Rust format, Clippy, Rust tests, TypeScript typecheck, and Pi extension tests.
CLI behavior tests use private state and fake Herdr/SSH executables. Time-sensitive unit tests inject timestamps.
See [the design](docs/design.md) for the later watchdog and cleanup unit.
