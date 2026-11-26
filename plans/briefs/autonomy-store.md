# Autonomy store and the `decide` commands

Plan: `plans/autonomous-toggle.md` (approved). Wire types: `portal-types/src/autonomy.rs`. Base: `origin/main` c55c4ff.

## Outcome

Each host stores the two switches and its agents' decisions in `state.db`, and `dogsled decide`, `dogsled decisions`, and the MCP tool `decide` write and read the decisions.

## Design

`state.db` owns this state (`src/store.rs`). Nothing resolves or publishes autonomy in this unit; the next unit does that.

## Edits

1. `src/autonomy.rs` (new, declared in `src/main.rs`): the schema constant and the typed functions below. Follow `src/lead_store.rs` for the store style.
   - Tables, all `STRICT`:
     - `autonomy_fleet(id TEXT PRIMARY KEY CHECK(id = 'local'), on_ms INTEGER NOT NULL)`.
     - `autonomy_agents(pane, session, on_ms, PRIMARY KEY(pane, session))`.
     - `autonomy_decisions(seq INTEGER PRIMARY KEY, id TEXT UNIQUE, pane, session, name, owner_host, owner_pane, created_ms, source CHECK(source IN ('question','plan','agent')), title, decision, why, question, alternatives, review)`.
   - `fleet(ledger) -> Result<bool>` and `set_fleet(ledger, on, now)`.
   - `agents(ledger) -> Result<Vec<(pane, session)>>` and `set_agent(ledger, pane, session, on, now)`. Switching on replaces older sessions of the same pane.
   - `record(ledger, shell, pane, body: &DecideBody, source, review: Option<&str>, now) -> Result<DecisionView>`.
     Refuse with `Error::Invalid(body.refusal())`. Read the session with `inbox::local_agent` (`src/inbox.rs:789`), as `done::signal` does (`src/done.rs:505`).
     Read `name`, `owner_host`, and `owner_pane` from `launch::observed_agent_launch` (`src/launch.rs:1511`), as `owned_work::agent_executions` does (`src/observation/owned_work.rs:288`). A pane without such a launch has none.
     `host` is `inbox::hostname()`. In the same transaction, delete all but the newest 100 rows.
   - `decisions(ledger, pane: Option<&str>) -> Result<Vec<DecisionView>>`, newest first.
2. `src/store.rs`: add the schema to `schema()` (:13). `VERSION` 8 becomes 9. Versions 6, 7, and 8 upgrade in place (:78): create the three tables, and rebuild `web_questions` so that `state` also accepts `'released'`.
3. `src/web_store.rs:91`: `web_questions.state IN ('pending','answered','rejected','released')`. A released row has `answered_ms` set and no `rejection_reason`. No code writes `released` in this unit.
4. `src/cli.rs`: `Decide { --title, --decision, --why, --question, --alternatives }` and `Decisions { --pane }`, after `Blocked` (:337). `src/main.rs`: dispatch beside `turn_command` (:217). `decide` uses `inbox::own_pane(None)?` and source `agent`. It prints the decision id, or the `DecisionView` with `--json`. `decisions` prints one block per decision, or the list with `--json`.
5. `src/mcp.rs`: tool `decide` with the five fields, after `end_turn` (:693), in the same style. Description: "Record a decision you made without your owner: what you chose and why. Use it for each open question you settle yourself."
6. `README.md` (near :292 and the tool list at :883) and `plugins/dogsled/skills/dogsled/references/commands.md` (near :77): document both commands and the tool.

## Paths

Own: `src/`, `tests/`, `README.md`, `plugins/dogsled/skills/dogsled/references/commands.md`. Read: `portal-types/`, `plans/`.
Do not change `portal-types/`, `web/`, or `pi/`. If a wire type is wrong, reply `blocked` with the reason.

## Acceptance

`cargo test autonomy` passes, with tests in `src/autonomy.rs` on a real temporary `state.db` (see the tests of `src/store.rs` near :424 for the fixture style):

- A version 8 store with one pending and one answered question opens at version 9 with both rows intact and the three new tables.
- `set_fleet` and `set_agent` round-trip; a second session of a pane replaces the first.
- 101 recorded decisions leave the newest 100, newest first. A body with a two-line title is refused and stores nothing.

## Checks

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test autonomy`, and `cargo test store::`. Run no other test and no other build.

The root crate's tests embed the portal bundle (`src/web/assets.rs:53`, `moon.yml` task `test`). Run `moon run :web-build` once in the worktree before the first `cargo test`. `web/public/` is ignored by Git: do not commit it and do not copy it from another checkout.
