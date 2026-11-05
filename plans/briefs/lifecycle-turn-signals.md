# L2: turn signals (`ask` and `blocked`)

Source: `plans/lifecycle-cleanup.md`, section 2. There are two units, and they run in this order:

1. **-16** records signals.
2. **-17** exposes and delivers them.

Both units start from current `origin/main`.

## Turn identity

This is the binding decision. Every unit uses it.

A signal is bound to a **turn key**: `{pane, session, turn}`.

- `session` is `Agent.agent_session`. It is required. A pane with no readable session refuses the signal with a typed `unproven` error.
- `turn` is `{state_change_seq, state_since_ms}`. Read it from the live agent at call time, while `Agent.state == "working"`. A call made while the agent is not working refuses, in the same way `done::mark` refuses.
- `completion_seq` is never part of the key. It resets on restore.

**Clearing.** A recorded signal stays visible until a later turn of the same session is positively observed. The `done::observe` pass clears it when it sees an agent in the same pane and session with `state == "working"` and either:

- `state_change_seq > turn.state_change_seq`; or
- equal or missing counters, with `state_since_ms > turn.state_since_ms`.

**Ambiguous identity.** In these cases, keep the signal and do not clear it:

- the counters went down while the session stayed the same, which means a restore;
- either the counters or the timestamp are missing.

A signal never changes terminal evidence (`sealed`, `reply`, `native_done`).

A different session in the same pane never sees or clears an old signal. Treat it as a different agent.

**More than one nonterminal call:**

- An identical repeat (same kind, same text) is deduplicated, and it returns the existing event id.
- A newer `ask` or `blocked` from the same session supersedes the active signal. Preserve the earlier event in persisted history.
- Equal or missing Herdr counters do not prove that the native turn stayed unchanged. Harness reminders can resume work without a visible state transition.

**Order against `done`:**

- `done` after `ask` or `blocked` is refused only when the available axes match and no new `message` or `task` input was claimed.
- Restored or decreased counters, entirely absent axes, and a positive later Herdr turn do not block `done`. Keep the existing unmatched-axis behavior.
- This rule is asymmetric: a false refusal strands a finished child. An accepted `done` retains the earlier signal in history.
- Deduplicate claimed input IDs. A lease replay cannot release a new signal's guard.
- Input delivery releases only the conflict guard. The earlier signal remains visible until the existing clearance rules apply.
- A harness-only reminder does not release the guard.
- `ask` or `blocked` after `done` remains refused.

## -16: record signals

Owned files: `src/done.rs`, `src/done/tests.rs`, `src/items.rs`, `src/lifecycle.rs`, `src/probe.rs` and `src/node.rs`.

1. Add a `Signal` record to `Mark` in `src/done.rs`. It is a new serde field with `#[serde(default, skip_serializing_if = ...)]`, so old `done.json` files load unchanged. Its fields:
   - `kind`: `ask` or `blocked`;
   - `text`;
   - `event_id`: a stable id, `sig-<pane>-<session-hash>-<state_change_seq>-<state_since_ms>`, or a ULID stored once;
   - `turn`: `{state_change_seq, state_since_ms}`;
   - `session`;
   - `recorded_ms`;
   - `cleared_ms: Option<u64>`.

   Write it under the existing `done` ledger lock.
2. Add `pub fn signal(ledger, shell, pane, kind, text, now) -> Result<Signal>`. It reads the live agent the same way `mark` does (`crate::inbox::local_agent`), and applies the identity, dedup and conflict rules above. A signal does **not** require clean source or finished children.
3. In `done::observe`:
   - clear signals as described under Clearing;
   - publish the active signal on the agent. Add `pub signal: Option<String>` to `probe::Agent`, with `#[serde(default)]`;
   - make `lifecycle::publish` / `of_agent` feed `Evidence.signal`. Remove `#[allow(dead_code)]` from `lifecycle::Signal`.

   The lifecycle derivation already maps an ended turn with a signal to `Asking` or `Blocked`.
4. Add an item projection for the owner side (`-17` delivers it). Add `items::Item.signal: Option<ItemSignal>` with `{kind, text, event_id, recorded_ms}`, as a serde default. Never use `set_reply` for a signal.
5. Unit tests in `src/done/tests.rs` and `src/lifecycle.rs`:
   - an ask during a working turn records, and the agent then derives `asking`;
   - blocked derives `blocked`;
   - an identical repeat is deduplicated;
   - a conflicting repeat is refused;
   - a signal from a different session in the same pane is not visible;
   - the next turn (a higher `state_change_seq`) clears it;
   - a restore (lower counters, same session) does not clear it;
   - done and a signal in the same turn are refused in both orders;
   - a mark file without signals loads unchanged.

   Each test must fail before the change.

Acceptance:

```
cargo test --bin dogsled done::
cargo test --bin dogsled lifecycle::
cargo test --test cli done::
```

## -17: expose and deliver

This unit runs after -16 merges.

Owned files: `src/cli.rs`, `src/main.rs`, `src/mcp.rs`, `src/runtime.rs`, `src/hosts.rs`, `src/inbox.rs`, `src/items.rs`, `src/node.rs`, `src/board.rs`, `src/done.rs` (the `CONTRACT` text only), `pi/index.ts`, `pi/index.test.ts`, `pi/unit-contract.test.ts`, `tests/cli/signals.rs`, `tests/cli/mcp.rs`, `tests/cli/inbox.rs` and `tests/cli.rs`.

1. Add `dogsled ask <question>` and `dogsled blocked <reason>` as CLI commands, as MCP tools `ask` and `blocked`, and as native Pi tools beside `done`. All of them call `done::signal`.
2. Owner import: the owner host imports child signals in the same way it imports replies, through host snapshots or the existing reply route.
   - Set `Item.signal` on the owner's item.
   - Deliver one inbox record of kind `ask` or `blocked` to the **current custodian**. Use `inbox::custodian_of` and `holds`, so the live session is checked; this is the same rule as replies. Deliver once per `event_id`.
   - A cleared signal clears `Item.signal`.
   - `ask` and `blocked` never close input and never seal.
3. Show the signal on the board row: the asking or blocked text appears as the row activity under NEEDS YOU.
4. Rewrite the guidance in all three places together: `done::CONTRACT`, the MCP tool descriptions, and the Pi tool descriptions in `pi/index.ts`. The guidance says:

   > End every turn with exactly one of: `done` (whole task complete), `ask` (you need an answer from your owner), `blocked` (you cannot continue), or waiting for your own open children.

   Present the native tool first, and do not present shell `dogsled done` as its equal. Do not change pi-todo.
5. Tests in `tests/cli/signals.rs`:
   - `ask` reaches the owner's inbox and item;
   - `blocked` shows its reason on the board;
   - the next turn clears both;
   - a replacement session never receives the old question;
   - `ask` works with dirty work and with open children;
   - an agent with open children and no signal shows `waiting`.

   Also update the MCP tool-list test, and the Pi contract test for the new tools and text.

Acceptance:

```
cargo test --test cli signals::
cargo test --test cli done::
cargo test --test cli mcp::
npm test
npm run typecheck
```

## Rules for both units

- Make the first edit within 3 tool calls.
- Push after each working step.
- Use the moon hook; never use `--no-verify`.
- Don't edit files outside the owned list. Stop and report if you need one.
- The final reply is one line with the SHA.
