# A committed unit whose worker pane is gone settles as `stopped`

Status: ready, not dispatched. Dispatch after `fix/unit-refresh-batch` (wJ9:p2D) is on origin.
Line references are checked at `origin/main` 4e2be38. Re-check them on the base head at dispatch, because the base branch changes `src/unit_request.rs`, `src/launch.rs`, `src/cli.rs` and `src/main.rs`.

## Outcome

If a committed unit's worker pane is gone and its worker never sealed a final reply, the owner gets exactly one `stopped` settlement. pi-todo then releases the unit to Pending. Nothing changes for a live pane, for a sealed reply, or when the host gives no proof.

## Bug and evidence

- Mac admission `r-1792657889925-39225-0` (owner wJ9:p2D, host momokaya-2, pane w45:p1) has `unit.committed` set and `abandoned` null.
- On momokaya-2, the worker's `dogsled unit terminal-ack` failed with REQUEST_CONFLICT "acknowledged boundary has no final assistant reply". Then `dogsled close w45:p1` removed the pane, the worktree and the branch.
- `unit settlements` for wJ9:p2D returns `[]`. Because of that, pi-todo refuses `done` (ACCEPTANCE_REQUIRED) and refuses `drop` (UNIT_ALREADY_BOUND). The unit is held for good.
- Second live repro on the Mac's local host: admission `r-1792659547509-90003-0` (pane wPV:p1, gone). Its item has `closed_ms` set, its phase is `unresolved` and its settlement is null.
- Do not read, edit or copy either real ledger (`~/.local/state/dogsled` on the Mac or on momokaya-2). All tests use temp-dir fixtures.

## Protocol (already supported; no change outside this repo)

pi-todo already accepts the Bound settlement `{version:1,eventId,binding,result:{kind:"stopped",reason}}`, with optional evidence. It releases the binding and sets the unit to Pending (see oisinpi `crates/todo-core/src/binding.rs:72-82` and `provider.rs:370`). Do not touch oisinpi.

`docs/unit-requests.md:160` says that dogsled never emits `stopped` until a termination owner proves termination. This change adds that owner.

## Design

The execution host owns the proof of termination. The owner imports that proof like all other host evidence.

### 1. Host proof (`src/launch.rs` `unit_receipt`, :823)

`unit_receipt(ledger, execution)` has no shell today. Add `shell: &dyn Shell`, and pass it from every caller:
- `child()` in `src/main.rs` (:340-349);
- the local branch of `refresh` in `src/unit_request.rs` (:455-456), which uses `fleet.shell`;
- the base branch's batch read, wherever it calls `unit_receipt`.

Inside the existing `receipts.with_lock(request_id)`, after the refusal branch, when ALL of these are true:
- there is no `terminal_receipt` and no final refusal file;
- `progress.attempted && progress.confirmed`;
- one successful pane snapshot shows that `lost_pane(&progress, &panes, &terminals)` is true.

Use the same snapshot that cleanup uses: `pane_snapshot` in `src/cleanup.rs:582`. Move it, or expose it, into one shared function; do not copy it. Use the `lost_pane` rule in `src/launch/recovery.rs:112` as it is: a pane that is missing or replaced (another terminal in it) is proof, and a missing saved pane or terminal proves nothing.

Then persist on `Progress` (:26) a new field `stopped: Option<Stopped>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`). Its reason is `"worker pane closed without a final reply"`. Return `HostEvidence::Stopped`.

- Idempotent: when `stopped` is already saved, return it again and do not probe.
- A herdr failure, a timeout or output that does not parse is an error from `unit_receipt`. It is never a stop.
- `terminal_ack` (:747) refuses with a conflict when `stopped` is saved. The launch lock orders the two.
- Why this place and not only `src/close.rs`: panes also leave by crash or by a tab closed by hand. After `dogsled close`, the pane is gone, so the next refresh proves it here. This gives one proof point, under the lock that already orders it with terminal-ack. If you also call it from the close path, call this same function.

### 2. Evidence type (`src/unit_reply.rs`, :30)

`HostEvidence` is `#[serde(untagged)]`. Add `Stopped(Box<Stopped>)` with `Stopped { kind: StoppedKind::Stopped, launch: Launch, reason: String }` and `deny_unknown_fields`, in the same shape as `Refusal`. The `kind` tag keeps the three variants unambiguous. Add a serde round-trip unit test that proves each variant decodes only as itself.

### 3. Owner import (`src/unit_request.rs`)

- Add `import_stopped`, with the same checks as `import_refusal` (:514): the launch matches, the host id matches, the committed seed is present, the unit is not abandoned, and there is no `returned` and no `terminal_receipt`.
- It saves `Prepared.stopped: Option<Returned>` (serde default, skip if none). The result is `ReturnedResult::Stopped { reason }`, which you add in `src/unit_transport.rs:281` next to `Returned`. The eventId is `{request_id}-stopped` and the binding is `committed.binding`.
- `settlement()` (:353) returns `returned`, else `stopped`, else `abandoned`.
- `import_receipt` (:578) and `import_refusal` reject when `stopped` is saved. This is a contradiction, logged per request; the record stays unchanged.
- The base branch's skip rule (its version of "this unit needs no host read") treats a saved `stopped` as final: no host call, no import.
- `unit observe` reports phase `stopped`.
- Owner-side faults import nothing: a transport error, `Invalid`, a parse error, a batch failure, or an old host that does not know the evidence. This is the existing match.
- An owner on 0.22.0 cannot parse the new variant. That is a parse error with no import, which is the correct result for mixed versions.

### Decision to confirm at review

A pane may close after the agent wrote a final reply that was never sealed by terminal-ack. That unit also settles `stopped`, not `returned`, because the host has no sealed reply. This is intended: acceptance needs a sealed reply. The unit goes back to Pending, and the item's reply text stays visible to the owner.

## Tests (`tests/cli/unit_seeded.rs` fixtures, fake herdr and ssh scripts)

Positive:
- A committed unit, its pane removed from the fake `herdr pane list`, no reply: `unit settlements` returns exactly one `stopped` event for that request.
- Running it twice still gives one event. After an ack, the result is `[]`.
- A pane replaced by a different terminal also gives `stopped`.

Negative:
- `herdr pane list` fails: no event, and the unit stays open.
- The pane is live with the same terminal: no event.
- A terminal receipt is saved, then the pane goes: the result is `returned`, not `stopped`.
- Remote ssh exits 255: no event.
- A terminal-ack after `stopped` is refused.
- A returned receipt that arrives after `stopped` is rejected, and the record is unchanged.
- `attempted` without `confirmed`, or a missing saved terminal: no event.

Keep green with no change: `invalid_host_evidence_never_falls_back_to_cached_return_projection` and the request-scoped rejection tests.

## Owned paths

- `src/launch.rs`, `src/launch/recovery.rs` (only to share `lost_pane`), `src/cleanup.rs` (only to share `pane_snapshot`)
- `src/unit_reply.rs`, `src/unit_request.rs`, `src/unit_transport.rs`, `src/main.rs` (only the `unit_receipt` call), `src/close.rs` (only if needed)
- `tests/cli/unit_seeded.rs`, `docs/unit-requests.md` (:160 paragraph, and :65 if needed)
- `CHANGELOG.md`: one `## Unreleased` Fixed entry. If the base branch already added `## Unreleased`, add a line under it.

## Checks (only these; the lead runs the full gate)

`cargo fmt --check`; `cargo clippy --all-targets -- -D warnings`; `cargo test --bin dogsled unit_ launch`; `cargo test --test cli unit_`.

## Publish

- Branch `fix/unit-stopped-settlement`, made from `origin/fix/unit-refresh-batch` at the SHA the lead writes into the dispatch.
- One commit: `fix(unit): settle a committed unit whose worker pane is gone`. Push it. Hooks must pass; never bypass them.
- No PR, release, tag, version bump, install or host change. The lead opens a draft PR, stacked on the base branch until that branch lands.

## Reply

Commit SHA, a diff summary for each file, the test names, and the check results. Or `blocked: <reason>`. Never end a turn without one of these.
