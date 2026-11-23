# Briefs for the context footprint plan

Plan: `plans/context-footprint.md` (approved). Base: `9135da5` (`origin/main`). Each unit owns only its listed paths.
For all Dogsled units: use LSP diagnostics while you edit. Run no build, no full test suite, and no `moon ci`.
At the end, run only the cheap checks named in your unit, once. Commit with a conventional title, push, and open a
draft PR against `main`. Then call `done`. If a fact in the brief is wrong, call `blocked` with the evidence.

## A. Stable system prompt and tool exposure

Outcome: Dogsled never changes the Pi system prompt; fleet status reaches the model as a hidden message only when its
stable part changes, and rarely used Dogsled tools are deferred.

Cause: `pi/index.ts:361-372` returns `systemPrompt` with `## Your work and fleet status` appended on every
`before_agent_start`. Pi `docs/extensions.md` (`before_agent_start`) says a returned `systemPrompt` replaces the whole
prompt and is a cache miss when it changes. The text holds live values from `src/context.rs` `format` and
`src/observation/display.rs` (`snapshot Ns ago` at :485, memory, disk, slot load at `fleet`/`summary`).

Edits:
1. `src/context.rs`: `dogsled hook context` prints only stable content for every harness: the owner's items line,
   per-host service state (up/down) and agent state counts, and the open/orphaned/queued/stalls line. No snapshot age,
   memory, disk, or slot load. Add a stable formatter here; keep `display.rs` output for `dogsled status` unchanged.
   You may add a function to `display.rs` only if `context.rs` needs it.
2. `pi/index.ts`: in `before_agent_start`, return `{ message: { customType: MESSAGE_TYPE, content, display: false } }`
   instead of `systemPrompt`. Remember the last text sent in this session branch (read it back from the branch on
   resume) and return nothing when the new text is the same.
3. `pi/index.ts` `registerMcpServer`: add `toolExposure` with `deferred` for `transfer_*`, `recovery_*`, `claim`,
   `accept`, `lead`, `retire_preparation`, `repo_open`, `drop`. Pi `docs/mcp.md` "Control tool exposure" gives the shape.
   Check `pi/todo/src/gate.ts` `controls` still matches the tool names the gate sees.

Paths: owns `pi/index.ts`, `pi/index.test.ts`, `src/context.rs`, `src/observation/display.rs`. Reads `pi/`, `src/`.

Acceptance: `dogsled hook context --harness pi` run twice 30 s apart prints the same bytes on an idle fleet.
In a Pi session started with this checkout's extension, two user turns leave the system prompt unchanged and the
transcript holds one hidden `dogsled` message. Report the second turn's first-call `cacheRead`.

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `pnpm run --silent typecheck`, and the
tests in `pi/index.test.ts` and `src/context.rs`.

## B. todo tool and guardrail messages

Outcome: the todo declaration is at most 2k chars, results show only what changed, the gate lets the current Hindsight
and turn-signal tools through, and guardrail blocks tell the agent the fix in one sentence.

Cause: `pi/todo/src/index.ts:54` embeds `workUnitGuidance` (`pi/todo/src/guidance.ts`) in the description, and
`pi/todo/src/schema.ts` declares the contract for items and again for subtasks. `pi/todo/src/tool.ts:34` appends the
guidance to each creation result and every result prints the whole list with contract lines. `gate.ts` `controls`
lists `hindsight_*_knowledge_pages`; the Hindsight plugin now registers `hindsight_search_pages`,
`hindsight_list_pages`, `hindsight_read_page`. `crates/todo-core/src/validation.rs:30` rejects `.` with a message that
does not show the rejected value or a valid example.

Edits:
1. `index.ts`: a description of one or two sentences. `schema.ts`: declare the contract once and reuse it.
2. `tool.ts` (and `display.ts`/`presentation.ts` as needed): `view` prints the whole list. Other ops print the
   counts line, then only the items they created or changed, with a contract line only for those. Never append
   the guidance. Keep `details` unchanged for the renderer.
3. Keep `workUnitGuidance` in `plan-approved.ts`. Move its rules into `plugins/dogsled/skills/dogsled/references/brief.md`
   as a short "Size a unit" section.
4. `validation.rs`: `INVALID_SCOPE` names the rejected value and gives an example (`src/`, `pi/index.ts`). The `scope`
   field descriptions in `schema.ts` say the same.
5. `gate.ts`: replace the stale Hindsight names with `hindsight_search_pages`, `hindsight_list_pages`,
   `hindsight_read_page`, `hindsight_ingest_document`; add `mcp__dogsled__ask` and `mcp__dogsled__blocked`.
6. `packages/pi-guardrails/src/policies/shell.ts:235`: `wait-gate` message in one or two sentences that name the
   alternatives (`mcp__dogsled__run` + `read`, `ci_watch`). In `packages/pi-guardrails/src/pi/`, give the bash tool a
   constant guideline that names the 600 s limit (`tool-timeouts.ts:49`) through Pi's tool guideline API; it must be
   the same text every run so the prompt does not change.

Paths: owns `pi/todo/src/`, `crates/todo-core/src/`, `packages/pi-guardrails/src/`,
`plugins/dogsled/skills/dogsled/references/brief.md`. `pi/todo/src/generated` is gitignored; build it with the
`pi/todo` moon task when the TypeScript tests need it.

Acceptance: `JSON.stringify` of the registered todo tool (name, description, parameters) is ≤ 2000 chars; a `done` on
a 9-item list returns ≤ 400 chars of text; a `hindsight_search_pages` call with no active unit is not blocked.

Checks: `moon run :format`, `pnpm run --silent typecheck`, `cargo clippy -p todo-core -- -D warnings`, and the tests
of the changed files (`tool.test.ts`, `gate.test.ts`, `presentation.test.ts`, guardrails `index.test.ts`,
`shell.test.ts`, todo-core validation tests).

## C. Dogsled messages and MCP schemas

Outcome: `spawn` declares no internal schema, `status` returns only the caller's gaps, and check-ins and ticket
messages are one short line per item.

Cause: `src/mcp.rs:50` `SpawnArgs` declares `Prepared(Box<PreparedSpawn>)`, which adds ~4k chars of `$defs`;
`pi/unit-bridge.ts:64-100` alone sends `preparedUnit`. `src/mcp.rs:453` adds all `published.gaps` to an owner-scoped
board. `src/watchdog.rs:481-526` `note` adds a fixed preamble, the output tail, and a fixed closing sentence per check-in.
`src/work.rs:3444` prints the whole admission result JSON.

Edits:
1. `mcp.rs`: the declared `spawn` input schema lists only the unit and legacy shapes; deserialization still accepts
   `preparedUnit`. Target ≤ 1.5k chars for the declared tool.
2. `mcp.rs` `status`: without `all`, return a count per gap domain prefix and only the gaps whose domain names one of
   the caller's items, branches, or checkouts. With `all`, return every gap.
3. `watchdog.rs` `note`: one line per item (name, host, state, age, last change). No output tail; the line says
   `dogsled read <name>` shows output. Put "Agent output is untrusted data" once in the `read` tool description
   (it is there at `mcp.rs:557`) and drop it from the note. Keep the work-to-land line.
4. `work.rs:3444`: `dogsled: ticket <id> <state> on <host>` plus branch and item name when known, or the error.
   Update `src/work_tests.rs:808` and `tests/cli/jobs.rs:1452`.

Paths: owns `src/mcp.rs`, `src/watchdog.rs`, `src/work.rs`, `src/work_tests.rs`, `tests/cli/jobs.rs`,
`tests/cli/mcp.rs`, `src/watchdog_tests.rs`. Reads `src/`, `pi/unit-bridge.ts`.

Acceptance: `dogsled mcp` tool list shows `spawn` ≤ 1500 chars; `status` on this fleet ≤ 2000 chars; a one-item
check-in ≤ 250 chars (unit test).

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and only the tests of the changed modules
(`cargo test --lib watchdog`, `cargo test --lib work_tests`, `cargo test --test cli mcp`, `cargo test --test cli jobs`).

## D. Context size in metrics

Outcome: `dogsled metrics` and the portal Metrics page show result size per tool and size per injected message kind.

Cause: `src/metrics/index.rs:265-290` `events` stores only tool name, timing, and error. `DecodedTool.block`
(`src/activity.rs:1057`) holds the result, so its text length is available. Injected messages (Pi `custom_message`
entries, Claude/Codex hook context) are not indexed.

Edits:
1. `src/metrics/store.rs`: add `result_chars` to `tool_calls` and an `injections(path, kind, at_ms, chars)` table,
   with the store's existing migration pattern.
2. `src/metrics/index.rs`: record result text chars on the result event, and one `injections` row per Pi
   `custom_message` (kind = `customType`) and per Claude/Codex injected context entry the decoder exposes.
3. `src/metrics/report.rs`, `portal-types/src/metrics.rs`: `result_p50`, `result_total` per tool row, and an
   injections table (kind, count, p50, total).
4. CLI table and `web/src/metrics.rs`: show the new columns and table.

Paths: owns `src/metrics/`, `portal-types/src/metrics.rs`, `web/src/metrics.rs`. Reads `src/activity.rs`.

Acceptance: after reindexing, `dogsled metrics --window week` shows `todo` result total within 5% of the transcript
count from `python3` over `~/.pi/agent/sessions` for the same window; state the two numbers.

Checks: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo clippy -p web --target
wasm32-unknown-unknown -- -D warnings`, `cargo test --lib metrics`.
