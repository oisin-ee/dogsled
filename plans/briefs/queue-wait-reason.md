# Queued tickets wait for the real reason, and an SSH blip does not fail them

Base: `origin/main` 2375c23 of oisin-ee/dogsled (v0.23.0). Lead: wJ9:p2S.

## Outcome

1. A queued ticket shows why it waits, as the host's own refusal at reservation. It never shows "for a host with room" when a host is eligible.
2. A transient SSH failure before launch puts the ticket back in the queue with backoff. Only a refusal or an invalid request fails the ticket at once.

## Bug and evidence

- `begin()` in `src/work.rs` (~:604): `commands::place()` returns `Placement::Full { hosts }`. Those hosts carry each host's refusal from `fleet.reserve`, but `begin()` returns `Ok(None)` and discards them. No wait reason is saved.
- `waiting()` in `src/board.rs` (~:658) rebuilds a reason from the status snapshot. It takes the first host that has a headroom reason. When the snapshot shows the hosts eligible but `reserve` refused them, it shows "for a host with room".
- The `admit` loop (`src/work.rs` ~:840-865) retries only `Error::Launch`. `retryable()` (~:875) wraps `Ssh`, `Command` and `Io`, but only for the launch call. An SSH error from `place()`/`reserve`, `prepare_launch` or worktree setup fails the ticket on the first try. Live (`~/.local/state/dogsled/queue.json`, read-only):
  - `r-1792669895624-1708-48`: failed, `ssh netcup: other`, retry attempts 0.
  - `r-1792672566806-1708-125`: failed, `ssh momokaya-2: remote command failed`, retry attempts 0.
  In both cases the host was up, and a respawn a minute later worked.

## Design

- Add `wait_reason: Option<String>` to `Ticket` (`#[serde(default, skip_serializing_if = "Option::is_none")]`). On `Placement::Full`, save one line per refusing host (`<label>: <reason>`), taken from the reports that `place()` returns. Clear the field on admission.
- `waiting()` shows the saved reason. It falls back to the snapshot only when no reason is saved. A pinned ticket shows only its host.
- Find which call raised each of the two errors above. Make an SSH failure before a confirmed launch retryable through the same `Backoff`/`LAUNCH_TRIES` path. Every such step must be safe to repeat: a reservation or worktree left by a failed try is released or reused, never leaked. Say in the commit body which steps you made retryable, and why each one is safe.
- Refusals (`Invalid`, `Deadline`, a conflict) still fail at once.

## Tests

- A ticket refused by a host that the snapshot shows as eligible shows that host's reservation refusal.
- No saved reason: the old fallback is unchanged. A pinned ticket shows only its host.
- An SSH failure at reservation or worktree setup leaves the ticket `pending` with retry attempts at 1. A second success admits it. After `LAUNCH_TRIES` attempts it fails with "(after N attempts)".
- An `Invalid` error still fails at once.
Use the existing fixtures and the fake ssh/herdr scripts in `tests/cli`. Never touch a real ledger.

## Owned paths

`src/work.rs`, `src/commands.rs`, `src/board.rs` (`waiting()` only), and their tests. Add one Fixed line to `CHANGELOG.md` under `## Unreleased`. Add that heading above `## 0.23.0` if it is missing.

## Checks

`cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo test --locked`. Run them on this Linux host; do not build on the Mac. Hooks must pass; never bypass them.

## Publish

Branch `fix/queue-wait-reason` from 2375c23. One commit: `fix(queue): keep the refusal reason and retry ssh blips`. Push, and open one DRAFT PR against `main`. No merge, release or tag.

## Reply

The commit SHA, the PR URL, the steps made retryable and why each is safe, the test names, and the check results. Or `blocked: <reason>`.
