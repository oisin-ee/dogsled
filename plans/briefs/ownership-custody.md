# Ownership custody for orphaned children

Status: user decisions recorded 2026-10-28 (interview b2d75ba5). Units O1–O4 under parent `-9`.

## Problem

An item's owner is its `owner_pane` (`src/items.rs:14`). When that pane ends, `sweep::children`
(`src/sweep.rs:209–268`) treats each open child as an orphan. `settle` (`src/sweep.rs:272–330`)
keeps every orphan that is not idle, done, clean and pushed, and writes it to `orphans.json`.
Every check-in on that host then repeats "Orphaned children the sweep keeps"
(`src/watchdog.rs:398–416`). Nobody can act on the notice: no command takes ownership, and unit
settlement accepts only the original `owner_id` (`src/unit_request.rs:140–143`, `875–883`).

## User decisions

1. **Takeover.** Nothing happens automatically. A lead takes over an orphan only with an explicit
   claim command. Until then, an orphan appears in `dogsled status` only. Check-ins and inbox
   notices do not repeat it.
2. **Authority.** A lead that claims a child inherits the original task. It can read the child's
   result, accept it, and settle the unit as done. Before a claim, other leads may only read.
3. **Orphan boundary.** A child is orphaned when its owner pane is gone, or when the pane survives
   but the owner's agent session has provably ended (the shell is open, the lead exited). Missing
   or unreadable evidence is not proof; such a child is `unknown`, not orphaned.
4. **Host.** Recovery stays on the host that records the item. A claim from another host is
   refused with a clear message. Cross-host transfer is later work.

## Design

- The task origin stays immutable: `Item.owner_pane`, the unit `ExecutionRef`, the final reply,
  and the unit result evidence never change.
- A separate custody record holds the current responsible owner: owner pane, owner session
  identity, revision, and the claim history (claimant, time, previous custodian, stable claim
  ID). It is durable, revisioned, and written under a lock on the recording host.
- A claim is atomic: it names the expected revision and fails if the record changed (two leads
  cannot both win). A claim of a child that is not orphaned is refused. A repeat claim by the
  current custodian is idempotent.
- Recovery events (reply delivery, settlement, attention) route to the current custodian by
  stable ID, never by pane alone, so a reused pane ID never inherits ownership.
- Existing policy stays: plan sections L3/P3 (lines 110–128) for stopped-orphan closure, and the
  `done` guard in `src/close.rs:146–153`. Resource closure never settles a task.
- Legacy items with no recorded owner session are ambiguous: the pane-gone rule still applies,
  but the session-ended rule needs recorded session identity and never guesses.

## Units

| Unit | Deliverable | Owns | After |
|---|---|---|---|
| O1 | Custody record: owner session identity on new items; durable revisioned custody store with atomic claim; orphan classification (pane gone, or session provably ended) | `src/custody.rs` (new), `src/main.rs` (`mod` line), `src/items.rs`, `src/work.rs` (record owner session at item creation only) | L1E landed (shares `src/main.rs`, `src/items.rs`, `src/work.rs`) |
| O2 | Route sweep, watchdog, inbox, gate and status through custody; orphans listed once in status, no repeated check-in notice | `src/sweep.rs`, `src/watchdog.rs`, `src/watchdog_tests.rs`, `src/inbox.rs`, `src/gate.rs`, `src/commands.rs`, `tests/cli/sweep.rs`, `tests/cli/watchdog.rs` | O1; ordered with `-13` and L3N on `src/watchdog.rs` |
| O3 | Settlement by custodian: a claimant can read, accept and settle the inherited unit | `src/unit_request.rs`, `src/unit_transport.rs`, `pi/unit-bridge.ts`, `tests/cli/unit_requests.rs` | O1, plus an architect binding of how the claimant's session records the inherited unit (provider todo files are not ours to edit) |
| O4 | `dogsled claim` in CLI, runtime and MCP; docs | `src/cli.rs`, `src/runtime.rs`, `src/mcp.rs`, `README.md`, `docs/design.md`, `tests/cli/claim.rs`, `tests/cli.rs` | O2, O3 |

## O1 acceptance

`cargo test --bin dogsled custody::` and `cargo test --test cli done::` pass. Tests prove:

- the custody record survives a service restart (reload from the ledger);
- a legacy item without owner session identity is never classified session-ended;
- a reused pane ID with a different session is not the owner;
- two concurrent claims at the same revision: exactly one wins, the other gets a conflict;
- a claim of a non-orphan, or from another host, is refused without a write;
- task origin, final reply and unit result evidence are byte-identical before and after a claim.
