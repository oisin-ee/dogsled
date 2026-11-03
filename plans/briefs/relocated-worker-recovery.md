# Brief: recover a seeded worker relaunched into another pane, from a remote owner

## Outcome

An execution owner can make a seeded worker finish and return normally when two facts are true:

- an external restart moved the worker into a new Herdr pane, and
- the owner runs on another host.

The worker then seals its unit and writes a terminal receipt through the existing path. The owner's refresh imports the receipt, and the provider releases the binding as `returned`. Nothing is backfilled, stopped, or accepted.

## Why

Release 0.22.0 is blocked. A stale binding owns `README.md` and `CHANGELOG.md`.

| Fact | Value |
| --- | --- |
| Execution | `{executor:"dogsled", ownerId:"wJ9:p1", requestId:"r-1792578251667-82915-0"}` |
| Origin | session `01a15d71-36a3-7380-be1c-b1b53fbc1b5b`, unit `u-b1e0edad-04e4-48cc-8fe8-ad78a4cea6ff-9` r2, entry `58931b1d` |
| Owner host | Mac (local) |
| Admission host | netcup |
| Recorded launch | pane `w0H:p1`, terminal `term_65e572768d62c243`, `owner_host` absent, no terminal receipt |
| Live worker | pane `w0H:p2`, terminal `term_65e5e3d56d94a33c`, PID 511320, idle |
| Relaunch | `~/.local/state/fleet-restart` started it with the same `--todo-unit-file`, `--session-dir` and `--session-id` |
| Transcript | `launches/r-1792578251667-82915-0.sessions/2026-10-21T10-24-37-967Z_r-1792578251667-82915-0.jsonl`, header id = request id, cwd = `.worktrees/feat-quota` |
| Work | PR #86 merged as `1d6fc22`; worktree locked and clean at `5cabcb9` |
| Done mark | none; the unit was never sealed |

Three current refusals block the normal return, at `211117d`:

1. `launch::terminal_ack` requires `HERDR_PANE_ID` and the live terminal to equal the saved `progress.pane` and `progress.terminal`.
2. `continuation::legacy` is local-only. It requires the owner pane, the admission and the worker on one host, an open local item for the pane, and a done mark.
3. No path lets a remote owner deliver input to a worker whose launch record has no `owner_host`.

## Required design

Read `docs/unit-requests.md` ("Worker terminal acknowledgement", "Legacy continuation on the origin host") and `src/continuation.rs` first. Keep their lock order and replay rules.

### A. Relocation evidence

Add a host-side relocation record, `launches/<request>.relocation.json`. Never edit the original `.json` or `.progress.json` launch fields.

The host writes the relocation record only after it verifies all of these facts under the `launches/<request>` lock:

- The live pane runs Pi, and Herdr reports its transcript at the launch's private session directory.
- The transcript header id equals the request id. The header cwd equals the launch cwd. The header has no parent session.
- The live Pi process arguments contain the launch's exact `--todo-unit-file`, `--session-dir` and `--session-id` values. Read them from the process, not from an environment value or a log.
- The agent name equals the launch's agent name.
- The recorded pane is gone, or it no longer runs this transcript.
- Exactly one live pane matches. Zero or several matches refuse.
- The launch has no terminal receipt, no refusal, and no earlier relocation to a different pane or terminal.

The relocation record stores the old pane and terminal, the new pane and terminal, the PID and its start time, the transcript path, and the observation time. An identical repeat is a no-op. A different repeat is a conflict.

`terminal_ack` then accepts either the recorded launch identity or the verified relocation identity. It must check the live identity again at the time of acknowledgement. All existing transcript, seal and ancestry checks stay the same.

### B. Remote owner continuation

Extend legacy continuation with a peer command. The origin host sends it; the admission host executes it.

- The origin host verifies the caller locally, with the same rules as today: owner pane equals `execution.ownerId`, Herdr reports Pi now, and the transcript header names the origin `sessionId`. It verifies the admission locally: committed, unsettled, valid export and binding, and selected the named remote host.
- The origin host sends the execution, the origin proof digest, the kind and the message over the existing peer channel to the admission host. The origin host's fleet configuration names the admission host, so the peer channel is authenticated by the existing SSH trust.
- The admission host accepts only when the request has no `owner_host`, the saved seed equals `.unit.json`, the launch has no terminal receipt, and the worker is the recorded pane or the verified relocation pane. When no done mark exists, it delivers the input to the worker's inbox. When a done mark exists, it uses the current mark rules.
- Keep the continuation receipt file, its series identity, its phases and its replay rules. The receipt records the caller's host.

### C. CLI

`dogsled legacy-continue <pane> --host <label> --message <text>` uses the remote path. Without `--host`, the command behaves exactly as now.

## Negative cases (each one needs a test)

- A wrong caller pane, host, or origin session refuses, and nothing changes.
- An admission that selected another host than `--host` refuses.
- A worker with the wrong session id, session directory, unit file, cwd, or name refuses relocation.
- Two live panes with the same transcript refuse relocation.
- A recorded pane that still runs the transcript refuses relocation.
- A copied transcript outside the private session directory, or a symlink, refuses.
- A launch that has a terminal receipt, a refusal, or a settlement refuses both relocation and continuation.
- A changed repeat of the relocation record or the continuation receipt is a conflict.
- A crash after each atomic write leaves a state that a retry finishes or refuses, as the current continuation crash tests do.
- Every refusal leaves the launch, progress, relocation, receipt, inbox and outbox files byte-for-byte unchanged.

## Owned paths

- `src/continuation.rs`
- `src/launch.rs` (`terminal_ack` identity and the new relocation helper only)
- `src/launch/` (new relocation module, if it is cleaner)
- `src/cli.rs` (`LegacyContinue` `--host`, new host peer subcommand)
- `src/main.rs` (dispatch only)
- `src/hosts.rs` (only to call an existing peer command; no new transport)
- `tests/` and in-file test modules for the files above
- `docs/unit-requests.md`
- `CHANGELOG.md` (one Unreleased entry)

Read only: `src/unit_request.rs`, `src/done.rs`, `src/inbox.rs`, `src/unit_reply.rs`, `pi/`.

## Out of scope

- Writing `owner_host`, a seal, a terminal receipt, a return, or a settlement for the worker. The worker produces its own return.
- A `stopped` outcome or any termination evidence.
- Changes to pi-todo, oisinpi, or the fleet-restart scripts.
- Any live run against netcup, the worker, or the stale binding. The lead does the live recovery after release and install.

## Acceptance

- Scoped checks: `cargo test continuation`, `cargo test launch`, `cargo test cli`, and the commit hooks. Do not run the full suite, e2e, builds for release, installs, or CI polling.
- One draft PR against `main`, with the negative-case tests listed in its description.
- No merge, tag, deploy, or install.

## Return

PR URL, head SHA, changed paths, the exact test commands with their results, and any precondition you could not prove in tests.
