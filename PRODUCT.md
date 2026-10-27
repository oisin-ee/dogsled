# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

One owner: the developer who runs Dogsled. They start lead agents in Herdr, and the leads start worker agents (writers, reviewers, scouts, architects) on three hosts: `local`, `netcup` and `momokaya-2`. The owner opens the portal on a large desktop monitor beside Herdr, often after time away, and wants to know at once what needs them.

## Product Purpose

Dogsled places, tracks, messages and cleans up coding agents across hosts. The portal (`dogsled web`, `127.0.0.1:7465`) is one browser window that shows Herdr and Dogsled together, so the owner stops switching between Herdr tabs, Glimpse windows and stray browser tabs.

Success: in one glance the owner sees what waits for them, acts on it in place, and returns to their own work.

## Operating Context

The first screen serves five jobs:

1. Answer what needs the owner: agent questions (`interview`), plan reviews (`submit_plan`), and blocked agents (key prompts).
2. Land finished branches ("to land").
3. Watch and steer running agents: read the conversation and send a message.
4. Follow each lead's todo progress per repository.
5. Check host capacity on `local`, `netcup` and `momokaya-2`.

Dogsled exists to remove cruft. A closed agent whose branch sits in "to land" for days is a Dogsled failure, not history to keep. The portal never files these rows away in an archive section. It shows each one as a decision with its cause (merged but `main` not pushed, or never merged) and the action that clears it (push, land, or prune). The owning fix is Dogsled's land and prune lifecycle, so these rows stop existing; the portal only makes the debt visible and actionable.

## Capabilities and Constraints

- Local only: binds `127.0.0.1`, rejects foreign `Host` and `Origin`. Remote, phone, SSO and push are deferred.
- The UI is Rust only: a Dioxus 0.7 web client compiled to WASM, embedded in `dogsled`. No terminal emulator; conversations come from transcripts.
- Board data: `GET /api/board` and the `/api/stream` SSE (rows grouped `to_land`, `attention`, `running`, `waiting`; todos keyed by pane; host load). Agents: `/api/agents`, conversation, screen, prompt, keys, focus. Reviews and questions have their own APIs and a one-shot fallback page when the service is down.
- Herdr can be unavailable (`herdr socket is not available`). The board still works from the ledger; agent detail and messaging do not.
- Board rows carry unit IDs as names (`u-8234e2c…`); the task title lives in the todo for the same pane.

## Brand Commitments

- Colours come from Herdr's theme, the same source as `src/sidebar/palette.rs` (Catppuccin Mocha today).
- Dogsled vocabulary: to land, attention, running, waiting, needs you, unit, lead, worker, host, pane.

## Evidence on Hand

- Live board and API responses from the running service.
- `plans/web-portal.md`: the portal plan and its layout intent.
- No user research beyond the owner's own use.

## Product Principles

1. What needs the owner comes first and is counted honestly across rows, questions and reviews.
2. Show the task, not the identifier. IDs and panes are secondary detail.
3. Act where you look: answer, approve, message or land without leaving the item.
4. Degrade visibly: when Herdr or a stream is down, say what still works and disable what does not.
5. No cruft. Finished work is landed or removed; the screen never normalises leftovers with archive or "older" buckets.
6. A blocked agent's request is that agent's screen. A plan review (Plannotator style) or an interview (pi-interview style) fills the agent's main area while it waits; the conversation becomes a tab. The Inbox button lists every waiting item and opens its agent with the item in front.
7. The owner works with leads, not their workers. Workers sit under their lead, muted, with muted status. A worker is raised only when it idles and its lead has not picked it up.
