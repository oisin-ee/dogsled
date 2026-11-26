# Portal speed: causes and fixes

Status: proposal, revision 2, 2026-11-27. Not approved. No code changed. The owner decided the targets and the branch list on 2026-11-27 (see Decisions).
Owner request: every portal interaction must feel as fast as Herdr.
Related: the north-star design book (`docs/design/north-star/`, branch `design/north-star`) sets the end state and the speed budgets. This plan removes the measured causes of the delays.

## Measured now

Measured on the live Mac portal at 0.43.2 on 2026-11-26, with `curl`, a headless Chromium tab, the service log, and a process sample of `dogsled serve`.

| Interaction | Measured | Target |
| --- | --- | --- |
| A second portal tab is open | Every request in both tabs stalls for more than 4 s, with no upper limit | No effect |
| Sidebar, agent state, Needs you | A new publication every 3.2 to 7.2 s; the log shows single passes of 20 s and 64 s | 300 ms after the event |
| Open a thread on this machine (1.4 MB history) | 830 ms; 743 ms of that is a wait before the request starts | Known history in the same frame, new part in 200 ms |
| Open a thread on another machine | 1.1 to 1.8 s | 300 ms |
| New agent text in an open thread | Read on a 5 s tick, then the whole history (1.0 to 1.4 MB) is sent again | 300 ms, only the new turns |
| Inbox | Each tab polls every 2 s; each poll takes 0.4 to 1.3 s and runs one SSH command per machine | No poll |
| Start a lead | The New screen stays until a naming call (limit 15 s, not measured) and the launch both end. The launch took 4.0 to 7.4 s for five of the last six leads and 122 s for one | The conversation opens 100 ms after Send |

## Causes

### C1. Three streams per tab use up the browser's connections

`web/src/api.rs` opens three `EventSource` streams for each tab: `/api/stream` (`watch`), `/api/agents/{host}/{pane}/events` (`watch_agent`) and `/api/agents/{host}/{pane}/activity/events` (`watch_activity`). The portal serves HTTP/1.1 (`Cargo.toml:148`, measured `nextHopProtocol: http/1.1`). A browser allows six such connections per origin across all tabs when the server does not use HTTP/2 ([MDN, EventSource](https://developer.mozilla.org/en-US/docs/Web/API/EventSource)).

Proof: in one tab, three more streams made `fetch('/api/agents')` stall for more than 4 s; it took 2 ms after the streams closed. The owner's browser held 12 connections to port 7465 during the measurement.

This cause produces "Live updates paused. Reconnecting.", the 10 to 20 s load, and old history that stays on screen.

### C2. One slow pass publishes everything

`service::run` (`src/service.rs:618`) runs one serial pass per wake. Each pass collects every domain (`Collector::refresh`, `src/observation/collect.rs:263`), composes the other machines, then publishes. The sidebar, agent states, the inbox and a new lead all wait for the pass.

Log, per 60 s: `collect` uses 46 to 64 s. `collect.host` 20 to 33 s, `collect.snapshot` 14 to 34 s, `collect.terminals` 6 to 11 s.

Child processes of `serve` in 20 s: `herdr pane process-info` 21 calls at about 0.4 s each (`src/probe.rs:68`, one per pane, through the `herdr` shim); `iostat -c 2 -w 1` four calls that each sleep 1 s (`src/probe.rs:554`); 23 `herdr agent list` and `herdr pane list` calls. The project already has a Herdr socket client (`src/herdr_socket.rs`) that needs no process.

Not yet attributed: the store reads inside `collect.host` and `collect.snapshot` when no terminal sample runs (1.6 to 1.8 s on average). Unit S4 starts with timings below phase level.

### C3. Transcripts are read on a 5 s tick and sent whole

`DETAIL_TICK` is 5 s (`src/observation/details.rs:18`). A change in live activity also wakes the reader. Each change makes the browser read the full history again (`web/src/conversations.rs:183`): 1.0 to 1.4 MB.

A first visit also waits for the per-pane reader to start: the browser may not request the conversation while the event says `pending` (`web/src/agent_view.rs:1266`). That wait was 743 ms of the 830 ms.

### C4. Each read from another machine starts an SSH command

`peer_call` (`src/web/agents.rs:338`) runs `ssh <host> dogsled agent-detail …` for each conversation, files, settings, reviews, questions and inbox read. Each takes 0.3 to 0.5 s. Opening one thread on another machine makes six of them in two waves. `/api/inbox` (`src/web/inbox.rs:18`) runs one per machine for each poll of each tab.

### C5. Starting a lead blocks on naming and on the launch

`POST /api/leads` returns only when the first prompt was accepted (`src/web/agents.rs:677`). Before the launch, `leads::start` calls `naming::summarize` (`src/leads.rs:209`), which runs `pi --print` with a 15 s limit (`src/leads/naming.rs:58`). The New screen keeps "Starting a lead…" for the whole time (`web/src/new_lead.rs:217`).

After the reply, the new pane is not in the publication until the next pass (C2). The conversation then shows the pane id as its title, "This agent is no longer available", and "Live updates paused."

### C6. Sidebar rows that belong to nobody

From the live board: 13 `to_land` rows, 5 jobs, and 3 rows with no repository.

- The rows under "Repository unknown" are two closed items of six days ago and one item in an unknown state. `item_repository` reads the repository from a live agent or a job's checkout (`src/board.rs:781`); these items have neither.
- Of 15 branch names in these rows, 8 have no branch on GitHub, 5 have a closed, unmerged pull request, 1 has an open pull request with a commit 6 minutes old, and 1 is in active work with no pull request. The list mixes live work, stopped work and records of branches that are gone.
- 11 rows have `ownership_stale`. A job with no known owner shows "Job · owner unavailable" (`web/src/board.rs:272`) and has no action.
- `dogsled status` shows 73 resource holds across the fleet. A lead that started two workers and one job kept 8 of them after all three closed: 6 validity records and 2 launch references. `dogsled unit settlements` and `dogsled unit retry-reference-releases` returned nothing for that lead. Dogsled counts each hold as "needs you" and the owner has no action for it. `docs/lifecycle.md` lists expiry of finished records as not yet implemented.

## Units

Each unit ends with the same measurement that found its cause.

| Unit | Change | Removes | Owns |
| --- | --- | --- | --- |
| S1 Live socket | One WebSocket per tab (`/api/live`) carries the portal publication, the open agent's events, its activity and the inbox. The three `EventSource` streams and the inbox poll go away. A WebSocket does not count against the HTTP connection limit in Chromium ([Chromium test page](https://chromium.googlesource.com/chromium/src.git/+/0a105493be280251894ce8d2ca67e6e70eddcc43/chrome/test/data/websocket/multiple-connections.html)). axum 0.8.9 `ws` and gloo-net 0.6.0 `websocket` are features of libraries the project already uses; axum `ws` adds `tokio-tungstenite` as a new dependency. | C1, the poll of C4 | `src/web/live.rs` (new), `src/web/mod.rs`, the stream handlers in `src/web/agents.rs` and `src/web/activity.rs`, `web/src/api.rs`, `web/src/app.rs`, the stream code in `web/src/agent_view.rs` and `web/src/activity.rs` |
| S2 Lead start | `POST /api/leads` returns when the lead record is saved. The launch continues in the service. The portal opens the conversation at once with the owner's message and the launch phase from the record. The name arrives later and replaces the first words of the message. A failed launch shows in that conversation with Retry. | C5 | `src/leads.rs`, `src/leads/`, `src/lead_store.rs`, `create_lead` in `src/web/agents.rs`, `web/src/new_lead.rs`, the loading states in `web/src/agent_view.rs` |
| S3 Transcript push | The per-pane reader watches the transcript file and reads on a change. It keeps the incremental parser it has. The live socket sends only the new turns; the browser appends them. The browser shows what it holds at once and never waits for `pending` to draw known history. | C3 | `src/observation/details.rs`, `src/activity.rs`, the conversation part of `src/web/live.rs`, `web/src/conversations.rs` |
| S4 Publication pass | Add timings below phase level. Read Herdr through the socket client. Move the CPU sample to its own sampler so no pass sleeps. Make a ledger wake collect only the domains that the store change touches. Target: a pass of less than 300 ms with no terminal sample. | C2 | `src/service.rs`, `src/observation/collect.rs`, `src/probe.rs`, `src/node.rs`, `src/observation/timing.rs` |
| S5 Other machines | The service already follows each machine with one long stream. Carry inbox items and the open thread's content on streams that stay open, so no read starts an SSH command. | C4 | `src/observation/peers*.rs`, `src/web/peer_stream.rs`, `src/web/inbox.rs`, `peer_call` callers |
| S6 Sidebar | Every row belongs to a lead. A lead's page has a PRs tab for its pull requests and those of its workers. The thread header names the machine on the control that opens the raw session in Herdr. A hold of a closed worker or job is released or expires and never counts as "needs you". See decisions 2 and 3. | C6 | `web/src/board.rs`, `web/src/app.rs`, `src/board.rs`, `src/web/board.rs` |

Order: S1 and S4 start together; they share no file. S3's reader change starts with them; its delivery follows S1. S2 follows S1 because both change `src/web/agents.rs` and `web/src/agent_view.rs`. S5 follows S1 and S3. S6 follows the owner's decisions.

The image-drop unit (`feat/composer-image-drop`) changes the composer in `web/src/agent_view.rs` and lands before S1.

## Decisions

Open, for the owner:

1. **Transport.** Recommended: one WebSocket per tab (S1). It adds one dependency, `tokio-tungstenite`, through axum. Alternative: one combined `EventSource` per tab. It removes the stall for two tabs, still stalls at six, and keeps the inbox poll.
2. **Jobs and "Repository unknown".** Recommended: a job shows only under the lead that started it. A job or closed item with no lead and no checkout is a dead record; the service removes it. No row has a "Repository unknown" group.
3. **Holds.** Recommended: add to S6 that a hold of a closed worker or job is released or expires, and is never counted as "needs you".

Decided by the owner on 2026-11-27, with the north-star book:

- **Branches.** "Branches to review" leaves the sidebar. A lead's thread ends with a "Landed" receipt. A PRs tab on the lead's page lists its pull requests and those of its workers. The service deletes a record whose branch is gone from GitHub and from the machine. A branch that still has commits and has no lead becomes one Needs you item per repository with two actions: start a lead to land them, or drop them.
- **Targets.** The targets in the table above are the acceptance test. Each unit ends with the measurement of its row.
- **Machine names.** A machine name appears in the capacity panel and on the Herdr control of a thread header, and nowhere else.

## Limits

- All numbers come from one Mac and one evening. The other two machines were measured only from the Mac.
- The store-read share of a pass is not attributed below phase level (see C2).
- The branch states were read from GitHub in `oisin-ee/rondo`, also for the three rows with no repository. Branches that exist only on a machine were not read.
- `momokaya-2` answered 502 "agent detail unavailable" for two conversation reads while its screen and files reads worked. The cause is not diagnosed.
