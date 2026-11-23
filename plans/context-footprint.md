# Cut the context that agent tooling spends

Status: approved on 2026-11-23 with the recommended decisions. Implementation in progress.

## Goal

1. A user turn reuses the prompt cache. Dogsled does not change the system prompt between turns.
2. Dogsled tool declarations, todo results, and Dogsled messages carry only what the agent needs to act.
3. Hindsight, global rules, skills, and project `AGENTS.md` files inject each fact once.
4. `dogsled metrics` reports context size, so this stays measured.

## Evidence

Measured on 2026-11-23 from 79 local Pi sessions of the last 7 days (`~/.pi/agent/sessions`).
`dogsled metrics` reports calls, errors, and latency, but no sizes. The numbers below come from the transcripts.

| # | Cost | Size | Owner |
| --- | --- | --- | --- |
| 1 | Fleet status in the system prompt breaks the prompt cache | ~200k uncached tokens per fast Claude user turn | `pi/index.ts:368` |
| 2 | Dogsled tool declarations in every session | 27.8k chars (~7k tokens) | `pi/todo/src/index.ts`, `src/mcp.rs` |
| 3 | todo results repeat the whole list and the guidance | 1.68M chars, 635 results | `pi/todo/src/tool.ts:34` |
| 4 | todo gate refuses Hindsight's current tool names | 53 refusals | `pi/todo/src/gate.ts` |
| 5 | Dogsled check-ins and tickets | 739k chars, 781 messages | `src/watchdog.rs`, `src/work.rs:3444` |
| 6 | `status` returns every fleet gap | 8.6k chars per call | `src/mcp.rs:453` |
| 7 | Guardrail block messages and retries | ~100 blocks, 200–650 chars | `packages/pi-guardrails` |
| 8 | Hindsight page refresh every turn, recall on the first prompt | 3.8k × 372 and 9.8k × 168 (14 days) | `~/.hindsight/coding-agent.json` (no owner) |
| 9 | rondo `AGENTS.md` loads in full | 75.5k chars per rondo session | `oisin-ee/rondo` |
| 10 | Stale `APPEND_SYSTEM.md` describes `bg_task`, which no package loads | 3.6k chars per session | `~/.pi/agent/APPEND_SYSTEM.md` (no owner) |
| 11 | Paperclip skills in the skill list | ~2.8k chars per session | paperclip install symlinks |

### 1. The cache break

`before_agent_start` returns `systemPrompt` with `## Your work and fleet status` appended. The text holds live values
(`snapshot 9s ago`, memory, host load). Pi documents the effect: a returned `systemPrompt` "replaces the whole prompt
for the run … a cache miss when it changes" (Pi `docs/extensions.md`, `before_agent_start`).

On Claude models, the first call after a user message that arrives within 2 minutes has a median cache hit of 25%.
Calls after a Dogsled custom message, which do not change the prompt, have a median hit of 100%.
The Claude Code plugin does not have this defect: it adds context in `UserPromptSubmit`, which goes in the user message.

### 2. Declarations

`todo` is 6.9k chars. Its description embeds the 1.2k-char `workUnitGuidance`, and the contract schema is declared
twice, for items and for subtasks. `mcp__dogsled__spawn` is 5.8k chars, mostly `$defs` for `preparedUnit`.
`pi/unit-bridge.ts:66` reserves `preparedUnit` for the extension's own handoff; the model must never send it.
The other 26 Dogsled tools are 0.2–1.3k chars each. In the last month, 12 of them had 11 calls or fewer:
`transfer_*` (0), `recovery_*` (3), `claim`, `accept`, `lead`, `retire_preparation` (1 each), `repo_open` (7), `drop` (11).

### 3. todo results

`append` averages 4.2k chars and `done` 2.7k. Of all todo result text, 45% is item descriptions,
27% is contract lines (`u-… r1 · self · scope`), 17% is the guidance paragraph (231 repeats), and 11% is status lines.
todo fails 13.9% of calls. The top causes are `INVALID_SCOPE` (50; agents send `.`) and the gate refusal (53).

### 4. The gate

`controls` in `pi/todo/src/gate.ts` lists `hindsight_search_knowledge_pages`, `hindsight_list_knowledge_pages`, and
`hindsight_read_knowledge_page`. The Hindsight plugin now names them `hindsight_search_pages`, `hindsight_list_pages`,
and `hindsight_read_page`. The Hindsight guide tells the agent to search first, so the first call is refused.

### 5–7. Messages

A check-in is ~1.1k chars. Two sentences are the same in every check-in (`src/watchdog.rs:482`, `:525`).
A ticket message prints the whole admission JSON (`src/work.rs:3444`). `status` adds `published.gaps` for the whole
fleet to an owner-scoped view; in the sample it held 12+ stale `job-validity` and `holds/checkout` records.
Guardrails only return a reason when they block a call. `timeout-gate` (40) and `wait-gate` (39) cost a retry each.

### 8–11. Not Dogsled

Hindsight is third party (`@vectorize-io/hindsight-coding-agents`). Only its configuration is in scope.
`pageRefreshEveryTurns` defaults to 1, so the 3.8k-char roster and guide come back every user turn.
`autoInject: "recall"` with `recallOptions.types: null` adds ~9.8k chars on the first prompt, and the guide still tells
the agent to search. `hindsight_list_pages` returns ~38.8k chars because each page description repeats the
scoping clause; that text belongs to the plugin and is out of scope.

## Does a project need `AGENTS.md` if it has Hindsight?

Yes, but a much smaller one. Measured on rondo:

1. Hindsight is not a copy of `AGENTS.md`. Across 13 rondo pages, 4-word overlap with `AGENTS.md` is 0.2%.
   Per section, 0–59% of the code identifiers in `AGENTS.md` appear anywhere in the pages.
   Development data, fixture authentication, communication, and vendored repositories have 0%.
2. Hindsight is a past record. Its own guide says to check each claim against the code. A rule ("run checks through
   moon", "never X") must be current and always present, so it cannot live only in Hindsight.
3. Moving text into Hindsight does not save context. rondo's "Conventions and patterns" page is 116k chars,
   larger than `AGENTS.md`.

The duplication is elsewhere. rondo `AGENTS.md` repeats the global rules in other words: "Delegated work" restates the
lead/worker split of the global `AGENTS.md` and the Dogsled skill, "Communication" restates `40-communication.md`,
and "CI minutes" restates the check tiers of `10-evidence-and-continuity.md`. It also keeps history
("The hosted Actions minutes ran out on 2 October 2026…") that Hindsight and Git already hold, and reference data
(Development data, 6.8k chars of fixture names) that only fixture work needs.
The global `AGENTS.md` and the Dogsled skill share 14% of their text: the delegation conditions are in both.

The fix is progressive disclosure, as the global `AGENTS.md` already does with its rule files: a short index of
project rules that always apply, plus linked files that the agent reads when their condition applies.

## Decisions (recommended values first)

1. Fleet status delivery: send it as a hidden custom message, only when its stable part changes.
   Alternatives: only at session start; or drop it and rely on `status`.
2. Rare Dogsled tools: `deferred` exposure through Pi tool search. Alternative: CLI only, documented in `commands.md`.
3. Hindsight: `pageRefreshEveryTurns: 20`, `autoInject: "none"`, `pageSearchLimit: 3`.
   Alternative: keep `recall` with `recallOptions.max_tokens: 800`.
4. rondo `AGENTS.md`: an index of at most 8k chars plus `docs/agents/*.md` files read by condition.
5. Paperclip skills: remove them from the Pi skill list if Paperclip is not in use.

## Work

Units A–D own different Dogsled files and run in parallel. E and F are in other repositories.

### A. Stable system prompt and tool exposure (`pi/index.ts`, `src/hook*`)

1. Return the fleet status as a `message` with `display: false`, not as `systemPrompt`.
2. Split `dogsled hook context` output into a stable part (counts, states, items that need the owner) and volatile
   values (`snapshot Ns ago`, memory, load). Send the message only when the stable part changes.
3. Pass `toolExposure` in `registerMcpServer`: `deferred` for the 12 rare tools, `direct` for the rest.

Acceptance: in a Pi session with two fast user turns, the second turn's first call has `cacheRead` ≥ 90% of the
previous context. The system prompt does not contain `## Your work and fleet status`.

### B. todo (`pi/todo/src/`)

1. Description: one or two sentences. Move `workUnitGuidance` to the Dogsled skill (`references/brief.md`), which
   leads read before they split work. Declare the contract schema once.
2. Results: print the counts, then only the items the operation changed. `view` prints the whole list.
   Print a contract line only for a changed item. Never append the guidance.
3. `scope`: give the format and an example in the field description. Name the rejected value in `INVALID_SCOPE`.
4. Gate: add `hindsight_search_pages`, `hindsight_list_pages`, `hindsight_read_page`, `hindsight_ingest_document`,
   `mcp__dogsled__ask`, and `mcp__dogsled__blocked` to `controls`. Remove the stale names.

Acceptance: the `todo` declaration is ≤ 2k chars. A `done` on a 9-item list returns ≤ 400 chars.
A first call to `hindsight_search_pages` with no active unit runs.

### C. Dogsled messages and MCP schemas (`src/mcp.rs`, `src/watchdog.rs`, `src/work.rs`)

1. `spawn`: leave `preparedUnit` out of the declared schema; keep its handling. Target ≤ 1.5k chars.
2. `status`: return a count per gap kind and only the gaps of the caller's items. `--all` returns every gap.
3. Check-in: one line per item; state the "untrusted data" and next-action text once in the `spawn` and `read` tool
   descriptions, not in each message.
4. Ticket message: state, id, host, and branch. No admission JSON.

Acceptance: a `status` call in the sample fleet returns ≤ 2k chars. A check-in for one item is ≤ 250 chars.

### D. Metrics (`src/metrics/`, `portal-types/`, portal Metrics page)

Index result chars per tool call and chars per injected message kind. Add `Result p50` and `Result total` columns
to the tools table and an injections table. This is the measurement that A–C and E–F use.

Acceptance: `dogsled metrics --window week` shows result sizes that match a transcript sample.

### Guardrails (`packages/pi-guardrails`)

Shorten `wait-gate` to one sentence with the alternative. Add the bash timeout limit to the bash tool guideline, so the
first call uses a valid timeout. The lead does this with unit B.

### E. dotfiles (`oisin-ee/dotfiles`)

1. Manage `~/.hindsight/coding-agent.json` declaratively with the decision 3 values, on all three hosts.
2. Remove the stale `APPEND_SYSTEM.md` block and the unused `@vanillagreen/pi-background-tasks` package.
3. Remove the delegation conditions from the global `AGENTS.md` template (`harness/rules/root.tera`); the Dogsled
   skill keeps them.
4. Decision 5: remove the Paperclip skill links.

### F. rondo `AGENTS.md` (`oisin-ee/rondo`)

Keep project rules that always apply. Move Development data, Game lineups, fixture authentication, Build and publish,
and the marketing site to `docs/agents/*.md`, each linked with its condition. Delete sections that restate global rules
and history narratives.

Acceptance: `AGENTS.md` ≤ 8k chars. Every removed rule exists in a linked file or a global rule.

## Delivery

1. A–D merge to Dogsled `main` and ship in one release. Local adoption waits for the published GitHub release.
2. E and F merge in their repositories. They need no Dogsled release.
3. After adoption, run `dogsled metrics --window week` and compare with the Evidence table.
