# Fleet leads

Status: decisions recorded 2026-11-04. Stage 1 is approved for implementation. Stage 2 is planned.

## Goal

The user starts a lead from the portal or the CLI and works with it. The user does not choose or care which Herdr host runs it.

A lead is a top-level agent. It has no owner pane, no done/ask/blocked contract, and no automatic close. It stays open until the user closes it.

## Decisions

| Topic | Decision |
| --- | --- |
| Portal UX | A **New** button opens a blank conversation in the main panel. The header has a project picker. The normal composer is below it. The first Send creates the lead. |
| What starts | A top-level lead, through a new `dogsled lead` command. The portal, CLI, and MCP share one implementation. |
| Hosts | Any fleet host (stage 2). Stage 1 is the local host only. |
| Capacity | Pick the host with the most room. Never count a lead against capacity, and never queue one. |
| Repo list | Config-driven, in `config.toml`. `dogsled repo add` and `dogsled repo remove` edit that list. |
| Harness | `pi` only. No harness picker. |
| Order | Local first, then fleet placement and remote chat. |

## Current facts

- `runtime::request` requires `HERDR_PANE_ID` as the owner (`src/runtime.rs:100`). A lead has no owner, so it cannot use the child request path unchanged.
- `work::agent_argv` adds the child contract as the system prompt (`src/work.rs:2277`). A lead must not get it.
- `work::pane` already opens a tab in the workspace that shows the repository, or a new workspace (`src/work.rs:1432`).
- `checkout::ensure` provisions a checkout on any host from the origin URL and a home-relative path (`src/checkout.rs:90`). A repo-list entry uses the same pair.
- The only repository list in config is `cleanup.repositories`, and only `doctor` reads it (`src/config.rs:71`).
- Every portal agent route uses the local Herdr socket (`src/web/agents.rs`, `AppState.agents`).

## Stage 1: local leads

### 1a. Repo list

- Add `[[repos]]` to `config.toml` with `origin` and `path` (home-relative). The origin identity uses `checkout::identity`.
- Add `dogsled repo add [<path>]`, `dogsled repo remove <origin-or-path>`, and `dogsled repo list`. `add` reads the origin from the checkout at `<path>`, or from the current directory.
- `add` and `remove` edit the config file so that the file stays the only source of truth.
- Decide during implementation whether `cleanup.repositories` merges into this list. Do not keep two lists that mean the same thing.

### 1b. `dogsled lead`

- `dogsled lead --repo <origin-or-path> --message <text>` (also `--message-file`). The MCP tool `lead` takes the same arguments.
- Record the lead in its own host-local table, not as an `Item`. `Item.owner_pane` and its SQL column are required (`src/items.rs:14`, `src/tables.rs:52`), and a fake owner would misstate the child contract. The record holds the request ID, the name `lead-<request-id>`, the repo, the pane, the terminal identity, the launch phase, and explicit closure.
- Leads stay in the observed agent inventory but are excluded from the admission counts (`src/plan.rs:48`).
- Writer brief with units L1 to L4: `plans/briefs/lead-command.md`.
- Open a tab with the same placement rule as `work::pane`. Start `pi` with `herdr agent start <name> --kind pi --pane <pane>` and no contract flags. Then submit the first message.
- `close` and `read` accept a lead. A lead never takes part in handoff or the watchdog.

### 1c. Portal New button

- Put **New** in the Agents column header (`web/src/app.rs:309`) and in the narrow-screen tab header. Replace the "Start an agent with `dogsled spawn`" empty state with the same button.
- The blank conversation reuses the composer and the `Draft` context. The project picker lists the repo list. Its default is the repo of the selected agent.
- Add `POST /api/leads` with `{ repo, prompt }`. It calls the `dogsled lead` code on a blocking task and returns `201 { pane }`. The portal then selects that pane.
- Add the shared body type to `portal-types/src/agents.rs` and `api::create_lead` to `web/src/api.rs`.
- Squig wireframe: `plans/fleet-leads.squig.json`, before the UI is built.

## Stage 2: fleet leads

### Host choice

- `dogsled lead` picks the eligible host with the most room from the fleet snapshot. It does not reserve capacity and does not queue.
- `checkout::ensure` provisions the repository on that host.

### Remote chat

Herdr already forwards its API to saved machines: `herdr --machine <label> <command>` (herdr 0.9.5, `herdr --skill`, "To control a saved SSH machine"). `herdr --machine netcup agent list` works from this host. That covers `agent list`, `agent read`, `agent prompt`, `agent send-keys`, `agent focus`, `agent start`, and `tab create` on a remote host.

Two gaps remain:

1. **Forwarding is a CLI feature, not a socket feature.** The portal uses the socket directly (`herdr_socket::Client`). The socket schema (`herdr api schema --json`, protocol 22) has no machine field. For a remote agent, the portal must call the `herdr --machine` CLI.
   - Live events (`events.subscribe`, `events.wait`) are socket methods only. The v0.9.5 CLI reference lists the forwarded commands (`workspace`, `worktree`, `tab`, `pane`, `notification`, `agent` except `attach`, `api snapshot`, `status server`, plugin commands), and none of them subscribes. Source: https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/cli-reference.mdx, "Saved SSH machines".
   - Unverified: `herdr --machine <label> agent wait` against a live remote pane. The test job (`herdr --machine netcup agent wait w1W5:p1 --status done`) could not run on 2026-11-04: local and netcup refused work at disk below 15%. Prove it before stage 2 relies on it; otherwise remote state polls.
2. **Transcripts are files on the remote host.** The conversation view reads the agent's session file (`agent_session.value`, for example a path under `/var/lib/workloads/home/dev/` on netcup). Herdr does not forward file reads. Add a `dogsled host conversation <pane>` command that the portal runs through the existing `Fleet::remote` SSH path.

The agent list in the portal merges the local socket with `herdr --machine <label> agent list` for each enabled machine. Each agent row carries its host label, and every agent route takes the host.

## Open items

- The exact `[[repos]]` schema and whether it replaces `cleanup.repositories`.
- Stage 2: the poll interval for remote agent state, and how the portal shows an unreachable machine.
