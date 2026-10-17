# Plan: one wrangle board, event-driven child close, and Pi delegation without Shepherdr

Status: draft for review, 2026-10-17. After approval, this file moves to `docs/plans/2026-10-17-wrangle-board.md`.

## Context

### What the user asked for

- One panel in Pi that shows everything the lead owns: Pi children, Claude and Codex children, jobs, and queued tickets. Each row names its harness and model, or says "job" and gives the command.
- No Shepherdr "herdr agents" box and no second view of the same children.
- No "wrangle close when done" chore. Children close because an event happened, not because the lead remembered.
- Already decided: a read-only child (clean worktree, no commits) closes as soon as it is idle or done. A writer child closes when its branch lands.

### Why Shepherdr has to go

- Shepherdr 0.2.19 and 0.2.21 always draw the widget. `src/fleet.ts` `refresh()` calls `renderAgentWidget` (`src/widget.ts:61-104`, id `herdr-agents`) for every agent it watches. Neither version has a setting to turn it off.
- Pi has no setting that hides another extension's widget. Clearing it from wrangle is a hack, because Shepherdr redraws it on the next refresh.
- So the only clean way to remove the box is to stop Shepherdr from watching children. That means wrangle starts and messages Pi children itself, and Shepherdr is uninstalled (`~/.pi/agent/settings.json:51`).
- This reverses a 2026-10-13 decision. `docs/design.md` (from 4a8d5d2) says "Wrangle does not replace Shepherdr. Shepherdr keeps agent spawn and parent/child messages."

### What Shepherdr does today, and how much it is used

The counts are `agents` tool calls in this machine's Pi sessions modified in the last 14 days (`~/.pi/agent/sessions/*/*.jsonl`).

| Action | Calls | Wrangle replacement |
|---|---|---|
| `send` | 747 | Children: `wrangle send`. Other agents (605 of the send and assign calls go to panes this session did not spawn, mostly lead-to-lead): see question 2. |
| `spawn` | 278 | `wrangle agent --profile`. `agent_type` values: general 142, opus 73, explorer 18, architect 12, astra 10, opus-architect 4, reviewer 3, pi 2, opus-orchestrator 1. |
| `help` | 223 | None needed. The CLI has `--help`, and the skill teaches the commands. |
| `read` | 181 | `wrangle tail` (exists for the watchdog). The final reply comes with the completion message. |
| `assign` | 87 | `wrangle send` (the work arrives the same way). |
| `list`, `find` | 128 | `wrangle status` and the board. |
| `watch`, `unwatch` | 70 | None needed. Wrangle watches every item it starts. |
| `attach`, `detach` | 0 | Dropped. |
| `blocking: true` | 9 | Dropped. `20-delegation.md:29` already forbids it. |

Other Shepherdr features: the completion message to the lead (`herdr-agent-event`, 2194 messages, and `herdr-agent-message`, 1002 messages), profiles, remote machines over SSH, the peer inbox, the `/herdr` control panel, and the board. Wrangle already covers remote hosts (`src/hosts.rs` `remote_argv`).

### Facts the design depends on (verified)

- **Herdr 0.9.5 starts any agent kind.** `herdr agent start <NAME> --kind pi --pane <ID> [-- AGENT_ARGS]` and `herdr agent prompt <TARGET> <TEXT>` work for pi, claude, codex, and others (`herdr agent start --help`). `src/work.rs:243-270` already uses them for non-Pi kinds.
- **Profiles map to Pi flags.** Shepherdr builds `--model <m> --thinking <t> --append-system-prompt <prompt.md> <pi_args>` (`src/profiles.ts:283-293`) and adds `--name <label>` (`src/launch.ts:213`). All three hosts have the same eight profiles. Locally, `~/.pi/agent/shepherdr/profiles` is a symlink to `~/.config/mise/conf.d/harness/settings/pi/shepherdr/profiles`. Only `reviewer` has a `prepare.mjs`, which adds git review context.
- **The actual model is in each transcript.**
  - Pi assistant messages carry `provider: "alias"`, `model: "opus"`, `responseModel: "claude-opus-5-5"`, and `thinkingLevel: "high"`. `model_change` entries carry `provider`/`modelId`.
  - Codex `turn_context` carries `model: "gpt-5.6-sol"` and `effort: "high"`. `task_complete` carries `last_agent_message`.
  - Claude assistant messages carry `message.model` (`claude-opus-5` on netcup). `<synthetic>` also appears and must be skipped.
- **Wrangle already watches children and prompts owners.**
  - The watchdog (`src/watchdog.rs`) marks idle or done as `finished` and urgent (`:250-266`). It prompts an idle owner with `herdr agent prompt <owner> <note>` (`:530-533`).
  - It scans at most once per minute (`:462-467`).
  - The note includes the last five lines of activity (`pane_tail`, `activity::recent`), not the child's final reply.
- **Wrangle already knows how to close safely.** The sweep (`src/sweep.rs:121-182`) closes orphans through `close::local(..., pushed=true)` or a remote `close --pushed`. `close::local` refuses busy panes, dirty worktrees, and, with `pushed`, unpushed commits (`src/close.rs`). `integration::check` reports whether a branch has landed (`src/integration.rs:22-34,91`).
- **Herdr reports a completion counter.** `herdr agent list` gives each agent `completion_seq`, `state_change_seq`, and `terminal_id`, next to `agent_status` (checked live on Herdr 0.9.5).
  - A new completion means `completion_seq` went up. That is an event, not a state guess.
  - `terminal_id` tells a new terminal apart from an old one when a pane ID is reused.
  - `probe::RawAgent` (`src/probe.rs`) does not read any of these yet.
- **The Pi API can draw a styled panel.**
  - `ctx.ui.setWidget(key, (tui, theme) => Component)` gives a component factory with theme colours (`types.d.ts:101-104`).
  - `truncateToWidth` and `visibleWidth` are in `@earendil-works/pi-tui`.
  - `pi.sendMessage(..., { triggerTurn, deliverAs: "steer" | "followUp" | "nextTurn" })` exists.

## Approach

Rust owns all data and policy. `pi/` only draws. Every harness gets the same behaviour through the CLI, and the same commands as model-callable tools through one MCP server that the CLI serves (item 10).

1. **Facts per agent pane, measured on the child's host.** Each host's `serve` adds the following to each `probe::Agent` it publishes in `host.json`:
   - `model`, `thinking`, and `reply_id` from the transcript.
   - For a pane in a linked worktree: `branch`, `ahead` (commits not on base), `dirty`, and `landed` (`integration::check`).
   - Git facts are recomputed only when HEAD, the base ref, or `git status` changes, so the 10 s probe stays cheap.
2. **Item records say what was asked for.** `Item` gains `harness`, `profile`, `model` (requested), and `role` (`reader` or `writer`, from the profile). Old records deserialize with defaults.
3. **`wrangle board --owner <pane> --json`.** This joins the owner's items, the queue, and the peers' facts into rows, each with:
   - group: `to land`, `running`, `attention`, or `waiting`
   - harness and model label, for example `pi · astra→opus 5.5`
   - host label (never a host id) and activity
   - age and quiet time
   - commits ahead
   - totals per host

   One shared module builds this for `wrangle status`, the prompt context, and the Pi panel.
4. **The inbox (the delivery layer).** Every message to an agent goes through one ledger inbox on the target's host. This covers a child's final reply, a blocked report, a check-in, a peer message, and a follow-up task.
   - **Storage:** `inbox/<terminal_id>.jsonl`, one record per message: `{id, from: <herdr_sender>, kind, text, created_ms, state, attempts, next_ms}`.
   - **Sending:** `wrangle send <target> [--host H] --message-file -` resolves a name or pane on any host. It appends the record locally, or through `host inbox put` over the existing SSH path.
   - **Pi targets:** the Pi extension watches its own inbox file with `fs.watch` and also polls on its 10 s timer. It injects each record with `pi.sendMessage({customType: "wrangle"})`, using `deliverAs: "steer"` while the session runs and `triggerTurn` while it is idle. Then it runs `wrangle inbox ack <id>`.
   - **Claude and Codex targets** pull their own records through the `Stop` and `UserPromptSubmit` hooks (see the research section on typing).
   - **Nothing ever types into a pane.** A Pi session without the extension, or any other harness, has no reader. Its records fail at once and are reported to the sender, so they are never left waiting where nothing will deliver them.
   - **Retries:** an unacknowledged record is retried with exponential backoff (10 s, doubling, capped at 5 min) until it is delivered or its terminal is gone. A record whose terminal is gone is reported to the sender's inbox.
   - **Message renderer:** `pi.registerMessageRenderer("wrangle")` draws a card with the sender, the kind, and the text.
5. **Retries with backoff everywhere it matters.** One `Backoff {attempts, next_ms}` helper in Rust, with a base of 10 s, doubling, a 5 min cap, and jitter, is used for:
   - SSH reads of an unreachable peer, in `cached_peer`. The host report gains `retry_ms`, so the panel can say "retry in 40 s".
   - Inbox deliveries.
   - Failed launches (`herdr agent start` timeout): the ticket is re-queued with backoff and reported to the owner after the last try.
   - "Retry now" in the panel clears the backoff.
6. **Closing is driven by events, in the sweep.** The sweep loop generalises the orphan pass to every open agent item. When an item is finished (idle or done):
   - First it captures the final reply into the ledger, so closing loses nothing.
   - Then it closes the child when `ahead == 0 && !dirty` (reader), or when `landed && !dirty` (writer), using the existing close path.
   - Otherwise the item stays open, shows as TO LAND, and the prompt line says "To land: web-tests (+2)".
   - Blocked and working children never close. Nothing closes on a timer.
7. **Wrangle starts and messages Pi children.**
   - `wrangle agent --profile <name>` resolves the profile on the target host. It runs `herdr agent start <name> --kind pi --pane <p> -- --name <label> --model … --thinking … --append-system-prompt <path> <task>`.
     - The task is Pi's positional `messages...` argument (`pi --help`: `pi [options] [--] [@files...] [messages...]`), so nothing is typed into the pane.
     - `--kind claude|codex` passes the task as the agent's initial prompt argument in the same way. `src/work.rs:259-268` stops calling `herdr agent prompt`.
   - `wrangle send <name|pane> [--host H] --message-file -` sends a follow-up.
   - When `completion_seq` goes up, `serve` reads the final reply from the transcript and puts it in the owner's inbox (decision 1). A blocked child sends a blocked report the same way. The watchdog's 30 min check-in stays for quiet children.
   - The Pi `tool_call`/`tool_result` hooks on `agents` (`pi/index.ts:71-92`) and `pi/placement.ts` go away.
8. **The `/wrangle` control panel** (`ctx.ui.custom`, pi-tui `SettingsList`, the same pattern as Shepherdr's `src/control-panel-view.ts`) has four tabs:
   - **Status** (the default): each host with reachable, service, load, and retry state; then your items and queue. Enter means "retry now" on a host.
   - **Hosts:** add, rename, enable, or disable a machine. Wrangle runs `herdr machine add/rename/enable/disable` and calls `herdr machine reconnect` in a terminal when SSH needs authentication.
   - **Profiles:** each profile's name, model, and thinking. Enter opens `prompt.md` in `$EDITOR`.
   - **Inbox:** messages received and sent, each with its state (pending, retry in N, delivered, failed).

   All data comes from `wrangle status|inbox|profiles --json`.
10. **The wrangle commands are tools (user feedback, review 1).** `wrangle mcp` is a stdio MCP server built into the CLI, using the official Rust SDK (`rmcp` 3.5.1, `github.com/modelcontextprotocol/rust-sdk`, updated 2026-10-05).
    - **One definition.** Each tool's schema and handler live in Rust (`src/mcp.rs`) and call the same functions as the CLI subcommands. No harness copies a schema.
    - **Tools:**

      | Tool | Runs | Replaces |
      |---|---|---|
      | `status` | `board`, `status` | `agents list`, `agents find` |
      | `spawn` | `agent --profile`/`--kind` | `agents spawn` |
      | `run` | `run` | a job via bash |
      | `send` | `send` | `agents send`, `agents assign` |
      | `inbox` | `inbox list` | none (new) |
      | `read` | `tail` plus the stored final reply | `agents read` |
      | `close` | `close` | none |
      | `stop` | `stop` | none |
      | `cancel` | `cancel` | none |
      | `place` | `place` | none |

    - **Nothing that waits.** No tool waits. `spawn` and `run` return at once, with `placed` or `queued`. `wait` and `logs --follow` stay CLI-only, because of the wait gate and `20-delegation.md:29`.
    - **Caller identity.** The server reads `HERDR_PANE_ID` from the environment its harness gives it. That pane owns what the call starts, as the CLI does today (`caller()`, `src/runtime.rs`).
    - **Registration in each harness:**
      - **Pi:** `pi.registerMcpServer("wrangle", {command: "wrangle", args: ["mcp"], exposure: "direct"})` in the `pi-wrangle` extension (`docs/extensions.md:195-204`). `direct` declares the tools like built-ins; the docs recommend it for "small, frequently used tool sets" (`docs/mcp.md:189-197`). They appear as `mcp__wrangle__<tool>`.
      - **Claude Code:** `plugins/wrangle/.mcp.json` with `{"wrangle": {"command": "wrangle", "args": ["mcp"]}}`. Claude Code loads `.mcp.json` at the plugin root (plugins reference, "mcpServers").
      - **Codex:** the plugin manifest's `mcpServers` names `codex-mcp.json`, which forwards the Herdr variables with `env_vars` (developers.openai.com/codex/plugins/build, codex/mcp).
    - **Guardrails.** The block in `harness-guardrails` names the `send` tool as the way to message another agent.
    - **Reversal.** This brings back an agent-facing tool. 4a8d5d2 removed the Pi `wrangle` spawn tool (`docs/design.md:193`). The difference now: the tool lives in Rust and serves all three harnesses, not a TS shim around Shepherdr.
    - **New dependency.** `rmcp` brings `tokio`. It stays inside `src/mcp.rs`, and every other command stays synchronous.
11. **The board widget** is a themed component in `pi/board.ts`. It follows the mockup below and refreshes every 10 s through `wrangle board`. On a narrow terminal it drops the activity column first, then the host column.

```
 wrangle  3 running · 1 to land · 1 waiting     local ▰▱▱▱ 1/12  netcup ▰▱▱▱ 2/16  momokaya-2 ▰▰▰▱ 5/7
 TO LAND
  ● web-tests      pi · opus 5.5        netcup      +2 commits · 4 files                 done 3m
 RUNNING
  ◐ api-dups       pi · astra→opus 5.5  momokaya-2  edit src/api/dups.rs                 14m
  ◐ review-auth    codex · gpt-5.6      netcup      cargo test -p auth                    6m
  ▸ cargo test     job                  local       running                               4m
  ! i18n-catalog   pi · sol             netcup      quiet 11m · last: rg "t(" web/       22m
 WAITING
  ⋯ rust-lints     pi · sol             for netcup (1.6 load/core)
```

## Files to modify

- `src/probe.rs`: `Agent` gains `terminal_id`, `completion_seq`, model, and worktree facts.
- `src/inbox.rs` (new): records, put/ack/list, and delivery.
- `src/backoff.rs` (new).
- `src/hosts.rs`: backoff in `cached_peer`, and `retry_ms` on the report.
- `src/activity.rs`: model and final-reply extraction for each harness.
- `src/items.rs`: new `Item` fields, and `finished` stays.
- `src/board.rs` (new): rows and groups.
- `src/sweep.rs`: settle every finished item, not only orphans.
- `src/watchdog.rs`:
  - The check-in note becomes an inbox record, and `herdr agent prompt` (`:530-533`) is removed.
  - The final reply travels in its own record, not in the note.
  - The finished-child urgency goes, because close is now automatic.
  - The 30 min check-in stays only for quiet or stalled children.
- `src/work.rs`: `Action::Agent` gains profile, and Pi launch args.
- `src/profiles.rs` (new).
- `src/cli.rs`, `src/runtime.rs`: `board`, `send`, `--profile`, `--message-file`, and `mcp`.
- `src/mcp.rs` (new): the tool definitions and the stdio server. `Cargo.toml` gains `rmcp` (server and stdio transport features) and `tokio`.
- `plugins/wrangle/.mcp.json` (new): Claude Code registration.
- `pi/index.ts`: `pi.registerMcpServer("wrangle", …)`.
- `src/commands.rs`: `close_line` becomes `land_line`.
- `pi/index.ts`, plus new files: `pi/board.ts` (widget), `pi/panel.ts` (`/wrangle`), `pi/inbox.ts` (watch, inject, ack), and the message renderer. Delete `pi/placement.ts` and its test.
- `package.json`: rename it to `@oisin-ee/pi-wrangle` and publish it to npm, with `pi.extensions: ["./pi/index.ts"]`. Add `@earendil-works/pi-tui` as a peer and a dev dependency. Add a release job.
- `plugins/wrangle/hooks/hooks.json` and a new Codex hooks file: inbox pull at `Stop` and `UserPromptSubmit`, using a new `wrangle hook inbox --harness claude|codex --event stop|prompt` command.
- Guardrails (dotfiles, then `@oisin-ee/pi-guardrails`): refuse `herdr agent prompt`, `herdr agent send-keys`, `herdr pane send-text`, `herdr pane send-keys`, and `herdr pane send-input` aimed at another agent's pane. Point to the `send` tool (`mcp__wrangle__send`) or `wrangle send`. Allow `send-keys` only with the answer-prompt marker (open question 3).
- `~/dev/pi-prewalk`: replace `packages/pi-prewalk-kit` with `packages/oisinpi`, and update the root README.
- Docs and tests: `README.md` (lines 68-137, 188-212), `docs/design.md`, `CHANGELOG.md`, `plugins/wrangle/skills/wrangle/SKILL.md`, `tests/cli/*.rs`, `src/watchdog_tests.rs`, `pi/index.test.ts`.
- Dotfiles (`~/dev/dotfiles`), done after the wrangle release:
  - Remove `npm:@howaboua/pi-shepherdr@0.2.19` from Pi settings.
  - Move the profiles directory.
  - Update `conf.d/harness/rules/topics/20-delegation.md:22-29,80`, `conf.d/harness/docs/tools.md:113`, and the global `AGENTS.md` line that names Shepherdr's `agents` tool.

## Reuse

- Pane start and prompt: `work::launch` (`src/work.rs:243-270`).
- Remote execution: `Fleet::remote_setup` and the `host launch` path (`src/work.rs` ~333-356).
- Close safety: `close::local` and the sweep's `settle`/`close` (`src/sweep.rs:121-182`).
- Landed check: `integration::check`.
- Activity text and ANSI stripping: `activity::recent` and `plain`.
- Delivery bookkeeping: watchdog `State.deliveries` and its pending/uncertain logic.
- Peer cache: `Fleet::cached_peer` (`src/hosts.rs:317-367`).
- Panel pattern: Shepherdr's `SettingsList` and tab view (read only, as a reference; nothing copied).
- Steer and idle delivery: `pi.sendMessage` options, as Shepherdr's inbox uses them (`src/peer-inbox.ts:~300-376`).
- Tool handlers: the existing `runtime.rs` command functions (`submit`, `Command::Close`, `work::cancel`, …). `src/mcp.rs` maps arguments onto them, and no logic is duplicated.
- MCP protocol: `rmcp`, the official SDK. Wrangle does not hand-roll JSON-RPC.

## Steps

- [ ] U1 Rust data: model and worktree facts in `probe::Agent`, new `Item` fields, `wrangle board --json`, `land_line`. Acceptance: `cargo test` covers the board groups, the model label for each harness, and old `items.json` still loading.
- [ ] U1b Inbox and backoff: `src/inbox.rs`, `src/backoff.rs`, `wrangle send`, `wrangle inbox put|ack|list`, `serve` delivery to non-Pi targets, and backoff in `cached_peer`. Acceptance: `cargo test` covers backoff timing, a retry until ack, a record dropped when its terminal is gone, and remote put through a fake SSH.
- [ ] U2 Event-driven close: capture the reply, then close readers on finish and writers on land. Acceptance: sweep tests show that dirty, unpushed, blocked, and working children stay open, and that a clean reader and a landed writer close.
- [ ] U3 Pi delegation in Rust: profiles, `wrangle agent --profile`, a reply to the inbox when `completion_seq` goes up, and launch retry. Acceptance: `tests/cli` uses a fake herdr to check the exact `herdr agent start … -- --model …` argv and a delivered note that contains the reply.
- [ ] U3b Tools: `wrangle mcp` with the ten tools, plus the Pi, Claude, and Codex registrations. Acceptance:
  - A `tests/cli` test drives `wrangle mcp` over stdio with `initialize` and `tools/list`, and checks the ten names and their schemas.
  - A `tools/call send` against a fake herdr writes one inbox record owned by `HERDR_PANE_ID`.
  - `spawn` returns within 1 s with `queued` when every host is full.
  - Live: `/mcp` in a Pi session shows `wrangle` with ten direct tools (`pi mcp list` reads only `mcp.json`, not extension registrations). `claude mcp list` and `codex mcp list` show `wrangle` connected.
  - Codex gets the server from the plugin manifest (`mcpServers` → `codex-mcp.json`, with `env_vars` forwarding the Herdr variables), so dotfiles write no `[mcp_servers]` entry.
- [ ] U4 Pi face:
  - `pi/board.ts` widget, `pi/panel.ts` `/wrangle`, `pi/inbox.ts` (steer or trigger, then ack), and the message renderer.
  - Remove the `agents` hooks.
  - Acceptance: `npm run typecheck && npm test`, with widget widths of 60, 100, and 160 columns, steer versus idle delivery, and ack after inject. Then a manual look at a live session.
- [ ] U5 Docs and skill: README, design, CHANGELOG, SKILL.md, then release.
- [ ] U5b No more typing into terminals: the Claude and Codex inbox hooks, and the guardrails block. Acceptance:
  - Hook unit tests cover `Stop` with records (block plus reason), `Stop` with `stop_hook_active` and no records (no output), and `UserPromptSubmit` (additionalContext).
  - A guardrails test refuses `herdr agent prompt w1:p1 hi` and allows `wrangle send`.
  - A live check finds zero check-ins merged with a user draft after a week, using the same transcript scan as the 12-case finding.
- [ ] U5c `oisinpi` collection: publish `@oisin-ee/pi-wrangle` and `@oisin-ee/pi-guardrails`, and create `packages/oisinpi`. Acceptance: `pi install npm:@oisin-ee/oisinpi` in a scratch agent directory loads prewalk, todo, wrangle, and guardrails, and none of them imports another (checked with a grep on the published tarballs).
- [ ] U6 Dotfiles: uninstall Shepherdr, move the profiles, rewrite the delegation rule. Acceptance: a fresh Pi session shows no `herdr-agents` widget, and a `general` child spawned with `wrangle agent --profile general` reports back.

U1 and U1b run in parallel. U2 needs U1. U3 needs U1 and U1b. U3b needs U3. U4 needs U1 and U1b. U5 and U6 run last, after the release.

## Verification

- `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `npm run typecheck`, and `npm test`.
- A live run on netcup:
  1. Spawn one reader and one writer with `wrangle agent --profile`.
  2. Confirm that the reader's reply reaches the lead and its pane, worktree, and workspace close.
  3. Confirm that the writer shows as TO LAND until its PR merges, then closes on the next sweep.
- Screenshots of the panel at narrow and wide widths, compared with the mockup.
- One delegation round in each of Pi, Claude, and Codex, using only the tools (`spawn`, then `send` a follow-up, then `read`), with no bash `wrangle` call. Each lead gets the child's reply through its inbox path.

## Decisions (user, 2026-10-17)

0. **Review 1:** the wrangle commands are also exposed as tools for the agent (Approach item 10).

1. **Delivering a child's final reply:**
   - The Pi extension injects it as a custom message.
   - Claude and Codex leads pull it through their `Stop` and `UserPromptSubmit` hooks. This supersedes the earlier "Rust prompts them", because typing into a terminal mangles drafts (round 3).
   - Both paths use the inbox described below, so each reply has one record and one delivery.
2. **Peers:** `wrangle send` works on any agent pane on any host, with a sender header. It replaces the 605 lead-to-lead `send`/`assign` calls.
3. **Profiles:** they move to `~/.config/wrangle/profiles/<name>/{profile.json,prompt.md}` and keep the same format.
4. **Reviewer:** `prepare.mjs` is dropped. The reviewer's `prompt.md` tells it to gather the git context itself.
5. **Shepherdr features to replace:**
   - The control panel, as `/wrangle`. Status is the main tab, and it has automatic exponential-backoff retries.
   - The peer inbox, and "we should actually start using" it.
   - The board/shared context and attach/detach are dropped (0 uses in 14 days).

## Research: messages typed into the terminal mangle the user's draft (2026-10-17)

**The user's requirement:** the inbox must be the *only* way a message reaches an agent. Today agents type into each other's terminals, and that mangles the user's own half-typed messages.

**Evidence**, from Pi sessions in the last 14 days (`~/.pi/agent/sessions/*/*.jsonl`):

- **12 user messages hold a user draft glued to a wrangle check-in.** The check-in text starts partway through the message. Examples:
  - `"1: yes\n2: wrangle check-in. …"` (2026-10-14 08:16)
  - `"tell pane w83:p2G what you know. i am wo` + `wrangle check-in …"` (2026-10-16 09:56)
  - Ten more, through 2026-10-16 16:03.
- **The cause is wrangle's own watchdog.** It delivers check-ins with `herdr agent prompt <owner> <note>` (`src/watchdog.rs:530-533`). Herdr types that text into the pane and submits it, together with anything the user has typed but not sent.
  - The "owner is idle" check (`:516-521`) cannot see a draft, because an agent whose user is typing still reports idle.
- **Agents type into terminals directly too.** In the same period, agents ran these through bash or codemode:
  - `herdr agent prompt`: 395 calls
  - `herdr agent send-keys`: 102
  - `herdr pane send-keys`: 33
  - `herdr pane send-text`: 14
  - Each of these can merge with a user's draft the same way.
- **Herdr has no way to deliver without typing.** The only text methods are `agent.prompt`, `pane.send_text`, `pane.send_input`, and `pane.send_keys` (`herdr api schema --json`, request `$defs`). All of them type into the terminal.
- **Pi can deliver without typing.** `pi.sendMessage` adds a message to the session and never touches the editor. Shepherdr's inbox works this way, so its `herdr-agent-message` deliveries do not appear among the 12.

**What this means for the plan:**

- Wrangle never types into a pane whose agent can read the inbox.
  - The watchdog check-in becomes an inbox record.
  - A child's reply, a blocked report, a peer message, and a follow-up task also become inbox records.
- Agents get one way to send: the `send` tool, which is `wrangle send` from a shell. `harness-guardrails` refuses any attempt to type into another agent's terminal (round 3).
- **Claude and Codex pull from the inbox through their own hooks (verified).** Wrangle never types into them.
  - **Codex** `Stop`: if the hook exits 0 with `{"decision":"block","reason":<text>}`, Codex continues and "creates a new continuation prompt that acts as a new user prompt, using your `reason` as that prompt text". The input includes `stop_hook_active` and `last_assistant_message` (developers.openai.com/codex/hooks, "Stop").
  - **Codex** `UserPromptSubmit`: plain stdout or `hookSpecificOutput.additionalContext` is added as developer context next to the user's prompt (same page, "UserPromptSubmit").
  - **Claude Code** offers the same pair. `Stop` with `decision: "block"` and `reason` keeps the turn going. `UserPromptSubmit` "cannot replace the prompt; it only injects `additionalContext` next to it" (code.claude.com/docs/en/hooks, decision table).
  - **Rule:** at `Stop`, the hook claims pending records and returns them as the `reason`, which starts a new turn. At `UserPromptSubmit`, it returns them as `additionalContext`, and the user's own text stays intact. The hook returns nothing when `stop_hook_active` is set and the inbox is empty.
  - **Limit:** an idle Claude or Codex agent with no turn running gets a message only at its next stop or prompt. The panel shows these records as `waiting for <name>'s next turn`.
  - **Hooks to update:** `plugins/wrangle/hooks/hooks.json` (Claude: add `Stop`) and the Codex `[hooks]` that wrangle installs through dotfiles (`~/.codex/hooks.json`: add `Stop` and `UserPromptSubmit`).

## Research: Shepherdr's control panel and peer inbox

- **Control panel.** `/herdr` (`src/controller.ts:127`) opens `ctx.ui.custom` with a pi-tui `SettingsList` (`src/control-panel.ts:82`, `src/control-panel-view.ts:85`). It has three tabs (`src/control-panel-items.ts:6`):
  - **Status:** "Refresh and reconnect", plus one row per machine with status, `connecting`, the reason, and the monitoring issue. Enter reconnects. It retries only when the user asks; there is no backoff.
  - **Settings:** the command allowlist path and "Prioritize delegation".
  - **Connections:** adds an SSH machine through `herdr machine add`.
- **Peer inbox.** Each Pi session opens a localhost TCP server. It writes a `{port, token}` descriptor next to its session file (`peerInboxPath`, `src/remote/shepherdr-peer.mjs:12`). A sender reads the descriptor, through SSH for a remote host, and sends one JSON frame that holds `<herdr_sender …/>` and the text.
  - The receiver calls `pi.sendMessage({customType: "herdr-agent-message"})`. It uses `deliverAs: "steer"` while the session runs, and `nextTurn` or a triggered turn when it is idle (`src/peer-inbox.ts:~300-376`).
  - It rejects a frame while the session changes context.
  - It reaches busy agents without typing into the terminal. Raw `herdr agent prompt` cannot do that, and it skips sender attribution (README line 86).
- **Wrangle today.**
  - Peer snapshots are cached for 10 s (`src/hosts.rs:13`, `cached_peer` `:317-367`). On failure, wrangle keeps the last snapshot and marks `reachable: false`. The next status call tries SSH again, with a 5 s `ConnectTimeout` and no backoff.
  - `herdr machine status --json` reports `reachable` for each saved machine without prompting for authentication.
  - `herdr machine reconnect` authenticates interactively in a terminal.

## Open questions

None are blocking. Both defaults below are open for annotation in review:

1. **Repo rename.** `oisin-ee/pi-prewalk` becomes `oisin-ee/oisinpi`, and prewalk and todo stay as packages inside it. The alternative is to keep the repo name and only add `packages/oisinpi`.
2. **Claude channels (not in scope).** An MCP server can push messages into an idle Claude Code session with `notifications/claude/channel` (code.claude.com/docs/en/channels-reference). That would remove the limit that an idle Claude agent waits for its next turn. Channels are a research preview, though, and a non-allowlisted server needs `--dangerously-load-development-channels` on every launch. They stay out of scope until they leave preview.
3. **The send-keys exception.** `herdr agent send-keys <pane> <keys>` is allowed only when the command line carries `# wrangle: answer-prompt`. The guard records each use in its audit log. Every other form of typing into another agent's pane is refused.

Round 3 answers:

- The kit is one collection, and no package in it depends on another.
- The kit is named `@oisin-ee/oisinpi`.
- `harness-guardrails` blocks agents from typing into another agent's terminal.
- Claude and Codex agents pull messages through their hooks.

Round 2 answers:

- `/wrangle` gets all four tabs: Status, Hosts, Profiles, and Inbox.
- Backoff applies to unreachable hosts, undelivered messages, and failed launches.
- A busy Pi session is steered.
- The transport is a ledger inbox.


## Packaging: `@oisin-ee/oisinpi`, one collection (decided 2026-10-17)

- **Shape.** `oisinpi` is a collection package and nothing more. It lists each of the user's packages in `dependencies` and `bundleDependencies`, and points `pi.extensions` and `pi.skills` at their `node_modules/...` paths (the current kit's pattern, `docs/packages.md:90`).
- **Members.** `@oisin-ee/pi-prewalk`, `@oisin-ee/pi-todo`, `@oisin-ee/pi-wrangle`, and `@oisin-ee/pi-guardrails`.
  - No member imports or calls another.
  - Prewalk and todo stay free of wrangle.
  - Only the collection's manifest names them all.
- **Where the code lives.**
  - `@oisin-ee/pi-wrangle` is wrangle's `pi/` directory, published from the wrangle repo as an npm package. The extension stays next to the CLI it calls, so one PR changes both.
  - `@oisin-ee/pi-guardrails` is `harness-guardrails`, moved out of dotfiles (`conf.d/harness/hooks/pi/extensions/harness-guardrails`, copied by `conf.d/harness/mise.toml:291`) into a package.
  - `oisinpi` itself lives in the `~/dev/pi-prewalk` monorepo and replaces `packages/pi-prewalk-kit`. The repo is renamed `oisin-ee/oisinpi`. GitHub redirects the old URL. The npm trusted-publisher entries must be re-pointed to the new repo name.
- **Settings.** `~/.pi/agent/settings.json` swaps `npm:@oisin-ee/pi-prewalk-kit@0.1.1` and `git:github.com/oisin-ee/wrangle@v0.9.1` for one `npm:@oisin-ee/oisinpi@<v>`, and removes `npm:@howaboua/pi-shepherdr@0.2.19`. Third-party packages stay as their own entries.

### Background research for packaging

**What the user asked:** "prewalk-kit" should become the user's own tools as one combined package, like rpiv or doompi.

**What exists today:**

- **`~/dev/pi-prewalk`** is a pnpm, moon, and changesets monorepo. Rust core crates are compiled to WASM, and each package under `packages/` is a thin TypeScript adapter.
  - `@oisin-ee/pi-prewalk-kit@0.1.1` is a meta-package. It lists `@oisin-ee/pi-prewalk` and `@oisin-ee/pi-todo` as `workspace:*` dependencies and `bundleDependencies`.
  - Its `pi.extensions` points at `./node_modules/@oisin-ee/<pkg>/src/index.ts` (`packages/pi-prewalk-kit/package.json`).
  - All packages share one version and publish to npm through trusted publishing.
- **Pi documents this pattern.** Other Pi packages used as dependencies "must be included in the published tarball and referenced through their `node_modules` resource paths" (`docs/packages.md:90`).
- **Comparable public packages:**
  - `@juicesharp/rpiv-pi` declares `extensions: ["./extensions"]` and `skills: ["./skills"]`. It installs its sibling tools through a `/rpiv-setup` dialog (pi.dev package page).
  - `@agimon-ai/doompi` is a full distribution that pins Pi itself (npm README).
  - The kit can follow the rpiv shape: one package that holds the user's own extensions and skills.
- **The user's own Pi tools today:**
  - `@oisin-ee/pi-prewalk-kit` (npm)
  - wrangle's `pi/` extension (`git:github.com/oisin-ee/wrangle@v0.9.1`)
  - `harness-guardrails`, a local extension in `~/.pi/agent/extensions`, managed by dotfiles
  - `herdr-agent-state.ts` is managed by Herdr and stays outside the kit.
- **Third-party packages stay out of the kit.** Bundling `pi-hypa`, `context-mode`, `plannotator`, and the others would vendor third-party code, which `00-scope-and-ownership.md` forbids. They stay as separate entries in `settings.json`.
- **Wrangle's Rust CLI stays a binary,** installed on every host through mise. The kit's wrangle extension calls it with `pi.exec`, as `pi/index.ts` does today. The extension must check the binary's version so the two cannot drift.
