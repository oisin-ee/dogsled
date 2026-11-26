---
version: 1
slug: "web"
primary_target: "web"
related_targets: []
---

# Portal main screen

Mode: Operate. Surface: the `dogsled web` main screen from 800px wide (designed at 1440×900), on a desktop monitor beside Herdr. Phones are deferred.

## Brief

- Task: see what waits on the owner and act on it, watch and message a lead, follow its todos, check host load.
- Owner requirements (2026-11-02):
  1. The inbox is never hidden. Every waiting item is visible and clickable at all times.
  2. Todos, agent chat and the agents list sit on the main screen, not in a hidden sidebar or tab.
  3. The composer is pinned to the bottom. No helper text; icons and controls, not instructions.
  4. Agent chat renders Markdown through a library (`pulldown-cmark`), never a hand-rolled parser.
- The screen fits the viewport. Chrome stays anchored; only the inbox, agents list, conversation and todos scroll inside their own regions.
- The palette is not pinned to Herdr's theme (owner, 2026-11-02).
- Layout steer (owner, 2026-11-02): no inbox strip across the top.
- One sidebar (owner, 2026-11-19, supersedes the two-sidebar steer): the separate "Waiting on you" column is removed. Agents that need the owner move to a "Needs you" section at the top of the agents sidebar, as in the Claude and Codex apps. Plan: `plans/combined-sidebar.md`.
- A plan review opens over everything right of the sidebar (owner, 2026-11-02). The waiting agents stay visible and usable.
- Branches to land are not inbox items; agents land them (owner, 2026-11-02). The agents list shows a muted per-repository count.
- Smaller screens use tabs (owner, 2026-11-02). The sidebar, with its Needs you section, never folds.
- Quiet console (owner, 2026-11-20, supersedes the colours, fonts and right column below): the owner chose variant B2 from `.impeccable/mocks/refresh-b/`. The live Now panel goes at the top of the right column, not above the composer. The right column keeps its Todos, Workers and Files tabs under Now. Only data that the portal has is shown; activity sparklines, CI stages, slots and the base commit wait for data.
- North star (owner, 2026-11-27, supersedes the items above where they differ): the direction is `docs/design/north-star/index.html`. Home is the composer with Needs you and what leads said since the last visit. A list row shows a lead's short name and its last words. "Branches to review" becomes a "Landed" receipt in the lead's thread, a PRs tab for the lead and its workers, and one decision per repository for branches with no lead. A machine name appears only in the capacity panel and on the Herdr control of a thread header. The speed budgets of the book's chapter 02 are the acceptance test of `plans/portal-snappy.md`. No layout is built from this before Squig wireframes and a layout plan exist.

## Direction contract

THESIS: The category standard, played straight: the dark agent console that Linear, Cursor and Conductor users already know. It refuses the current stack of bordered "Agent" cards and the tabbed hiding of todos.

OWN-WORLD: Slate navy ground #0F1626 with two raised panels (#131B2E, #1A2339), a selection tone #1F2943 and 1px #222C45 rules. Text #E8ECF5 / #9AA3B9 / #68728C. Indigo #8B95FF for selection, primary action and done; yellow #F2C94C only for "waiting on you"; green #4CC38A for running. IBM Plex Sans for UI and prose (15px messages), JetBrains Mono for code, branches, hosts and times; both shipped with the portal. 4–12px radii, no shadows, no glows, no gradients.

STORY: The owner returns, reads the Needs you section at the top of the sidebar first, clears an item in place (keys, Land, Prune) or opens its agent, then reads the lead's conversation and todo progress and sends a message.

FIRST VIEWPORT (as built, B2): 48px top bar (Dogsled, Metrics and New on the left, then the fleet tally; host load meters on the right). 292px sidebar with raised Needs you cards, then repository groups of lead rows with a state line. Conversation at a 640px measure with a 46px mono time column, owner messages in raised bubbles and finished turn work folded into one row. 300px right column: Now, then Todos, Workers and Files tabs. The paragraph below records the earlier first viewport.

EARLIER FIRST VIEWPORT: 44px top bar (Dogsled, Metrics and New on the left, host load bars on the right). Below it three columns, each with a 52px header row aligned across the screen: one 280px sidebar on the raised panel (Agents and its count; a Needs you section with a yellow count and one row per waiting agent: dot, agent title, age, then kind icon, kind and item title, inline key actions; then repository → lead → muted workers), conversation (title, state, role · harness · model · host · branch, icon actions; messages newest at bottom, 640px measure; composer pinned at the bottom with attach and send icons only), 300px todos column (in progress, pending, completed, abandoned). A plan review replaces the conversation and todos columns: plan at a 680px measure, 300px notes margin, decision bar pinned at the bottom; the opened waiting row carries a yellow left edge. Smaller screens: at 1100–1399px the todos fold into a segmented `Chat` / `Todos` control in the conversation header; at 800–1099px the review's notes margin becomes a `Notes` tab. The sidebar never folds.

FORM: The category standard (standing exit, chosen on safer re-roll 1); bar: Linear, Cursor, Conductor. seed:e4107608a77f64a0. Sketches: `.impeccable/mocks/decision/v6-console.html` (main screen) and `v6-console-review.html` (plan review open); `?tab=` picks the open tab. `v5-canon.html` is the card as chosen, before the layout steer.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Unresolved

- Code-block syntax highlighting: no library chosen yet.
