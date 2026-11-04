# Design: `dogsled lead`, a top-level lead with no owner

Plan: `plans/fleet-leads.md`, stage 1b. Base: `origin/main` a0ce8764258c4a70d968a53522a4e6f1b9c5ef6c. Read-only design unit. Write the result to `plans/briefs/lead-command.md`.

## Outcome

`plans/briefs/lead-command.md` is a writer brief, under 60 lines per unit, for `dogsled lead`. It names every file and line to change and contains no open design question.

## What is decided

- A lead is a top-level `pi` agent. It has no owner pane, no done/ask/blocked contract, and no automatic close. It stays open until the user closes it.
- It never counts against capacity and never queues. Stage 1 places it on the local host. Stage 2 picks the eligible host with the most room from the fleet snapshot, with no reservation.
- Its repo comes from the `[[repos]]` list (`plans/briefs/repo-list.md`, being built in parallel): `path` (home-relative) and `origin`.
- Three callers share one function: the CLI `dogsled lead --repo <origin-or-path> --message <text>|--message-file`, an MCP tool `lead`, and the portal route `POST /api/leads { repo, prompt } -> 201 { pane }`.
- It starts with `herdr agent start <name> --kind pi --pane <pane>` and no contract flags, then submits the first message with `agent.prompt`.

## Facts to start from

- `runtime::request` requires `HERDR_PANE_ID` (`src/runtime.rs:100`, `:139`). The portal has no pane.
- `work::agent_argv` adds the child contract (`src/work.rs:2277`).
- `work::pane` picks the tab or workspace for a cwd (`src/work.rs:1432`).
- `launch.rs` around `:2150` builds `herdr agent start`. Request validity is issued in `runtime::request_with_id`.
- `Item` (`src/items.rs:9`) has a required `owner_pane: String` and a `kind`.

## Questions to answer, each with file:line evidence

1. How is a lead recorded? A new item kind `lead`, or no item at all? What do `status`, the board (`src/board.rs`), and the portal board rows do with it?
2. Which launch steps can a lead reuse (pane allocation, validity, launch receipts, startup diagnostics), and which must it skip (admission, queue, reservation, contract, handoff)?
3. What must `close`, `read`, `send`, the watchdog (`src/watchdog.rs`), handoff (`src/done.rs`, `src/launch.rs` handoff functions), and the `serve` sweep do with a lead? None of them may close or settle it on their own.
4. How is it named, so `read`/`close`/`send` accept the name?
5. Stage 2 fit: what `dogsled host …` command does a remote host need for the same function through `Fleet::remote` (`src/hosts.rs:354`)?
6. Split: how many writer units, which files each owns, and the order. The portal UI is a separate unit; give it only the route and the shared type in `portal-types/src/agents.rs`.

## Owned paths

`plans/briefs/lead-command.md` only. Do not edit code.

## Acceptance

The brief answers all six questions with file:line evidence. Every edit it lists names its file and function.

## Checks

None. This is a docs-only unit.
