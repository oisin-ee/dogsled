# wrangle

Host status and placement for coding agents across [Herdr](https://herdr.dev) hosts.

Wrangle measures hosts and reserves capacity. Shepherdr starts agents and handles parent/child messages.
This branch contains the unreleased 0.5 redesign. Published 0.4 binaries do not support these commands.

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
wrangle place --branch feat/unit-a --repo /path/to/repo --json
```

Exit codes: 0 success, 1 no eligible host, 2 error.
When every host is full, `place` prints `{"full": true, "hosts": [...]}` even without `--json`.
There is no queue in 0.5. Jobs and a new queue belong to 0.6.

## Host service and status

Run `wrangle serve` on every host. Dotfiles owns launchd and systemd user-service installation.
The service uses no SSH. It reads load, cores, memory, home disk space, Herdr agents, and Herdr panes.
It writes `~/.local/state/wrangle/host.json` through atomic replacement.
Each `probe.agent_panes` entry contains `pane`, `agent`, `state`, `name`, `transcript`, and `transcript_modified`.
Transcript paths come from Herdr path sessions. Their modification times use Unix milliseconds. Unavailable paths or times are null.
The service stops with an error if a measurement fails. The last complete snapshot remains available.

On Linux, memory comes from a finite cgroup v2 user-slice limit and its usage.
Without that limit, it comes from `/proc/meminfo`.
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

`▲` means not eligible. `?` means the host read failed.
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

`place` writes a reservation with a default TTL of 60 seconds.
It returns the host id and label. JSON also includes the reservation id.
With `--branch`, it runs the prepare hook and returns the hook's `path` and `pane_id`, when present.
Then start the agent through Shepherdr, using that host and prepared pane or path.
Wrangle does not start an agent, choose a pane layout, or claim a repository lead.

A reservation with a prepared pane ends when that pane appears in `herdr agent list`, or at its TTL.
A placement without a pane remains reserved until its TTL.
If preparation fails, wrangle releases its reservation and reports the error.

## Configuration

The optional host policy lives in `~/.config/wrangle/config.toml`.
`--config PATH` or `$WRANGLE_CONFIG` selects another file.
Unknown keys are errors.

```toml
load_per_core_max = 1.5
disk_free_min_percent = 15
reservation_load = 1.0
reservation_ttl_ms = 60000
remote_command = ["mise", "x", "--", "wrangle"]

[hooks]
prepare = ["mise", "run", "-q", "agent:worktree", "--", "{branch}"]
```

Each repository can replace the prepare hook in `wrangle.toml` at its main checkout:

```toml
[hooks]
prepare = ["./scripts/prepare-child", "{branch}"]
```

The hook runs with the main checkout as its working directory, including calls from a linked worktree.
For peers, a checkout under the local home maps to the same relative path under the peer's home.
The peer reads its own checkout's `wrangle.toml`, falling back to its host policy.
Checkouts outside the home keep their absolute path.
Hooks substitute `{branch}` in each argument. Prefer an argument list over shell interpolation.
`{repo}` is removed and produces an explicit error.

The hook must print JSON with `pane_id`, `path`, or both. `workspace_id` is optional.
Progress text can precede a final JSON line.

State defaults to `$XDG_STATE_HOME/wrangle`, or `~/.local/state/wrangle`.
`$WRANGLE_STATE` selects a complete state-directory path.
Host snapshots use `host.json`. New placement reservations use `placements.json`.
The removed ticket ledger, queue, and event log are not read or migrated.

## Pi and Herdr

The Pi extension adds `wrangle status --brief` to each prompt through `before_agent_start`.
It registers no tool, timer, or widget. Status failures appear as extension errors.
`$WRANGLE_BIN` selects the binary used by the extension.

The Herdr plugin keeps only the status-board popup:

```sh
herdr plugin pane open --plugin wrangle --entrypoint board
```

The popup runs `wrangle status --watch`. The pane-close release hooks and sidebar token configuration are removed.
The skill in `skills/wrangle/SKILL.md` describes placement followed by Shepherdr spawning.

## Build and development

Build this branch with the repository's pinned Rust toolchain:

```sh
cargo build --release
npm ci
```

The executable is `target/release/wrangle`.
Install the same release on every host before enabling its service. This PR does not install or start host services.

The pre-commit hook runs `mise run check`: Rust format, Clippy, Rust tests, TypeScript typecheck, and Pi extension tests.
CLI behavior tests use private state and fake Herdr/SSH executables. Time-sensitive unit tests inject timestamps.
See [the design](docs/design.md) for later jobs, watchdog, and cleanup units.
