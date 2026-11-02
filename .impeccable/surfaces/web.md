---
version: 1
slug: "web"
primary_target: "web"
related_targets: []
---

# Portal main screen

Mode: Operate. Surface: the `dogsled web` main screen at 1280×800 and up (designed at 1440×900), on a desktop monitor beside Herdr.

## Brief

- Task: see what waits on the owner and act on it, watch and message a lead, follow its todos, check host load.
- Owner requirements (2026-11-02):
  1. The inbox is never hidden. Every waiting item is visible and clickable at all times.
  2. Todos, agent chat and the agents list sit on the main screen, not in a hidden sidebar or tab.
  3. The composer is pinned to the bottom. No helper text; icons and controls, not instructions.
  4. Agent chat renders Markdown through a library (`pulldown-cmark`), never a hand-rolled parser.
- The screen fits the viewport. Chrome stays anchored; only the inbox, agents list, conversation and todos scroll inside their own regions.
- The palette is not pinned to Herdr's theme (owner, 2026-11-02).
- Layout steer (owner, 2026-11-02): no inbox strip across the top. Inbox and agents are two left sidebars, as in the Pull Request sketch.
- A plan review opens over everything right of the inbox sidebar (owner, 2026-11-02). The inbox stays visible and usable.

## Direction contract

THESIS: The category standard, played straight: the dark agent console that Linear, Cursor and Conductor users already know. It refuses the current stack of bordered "Agent" cards and the tabbed hiding of todos.

OWN-WORLD: Near-black ground #0E0F11 with two raised panels (#141518, #1B1C20) and 1px #26272C rules. Text #E6E6E9 / #9A9DA6 / #62656E. Indigo #5E6AD2 for selection, primary action and done; yellow #F2C94C only for "waiting on you"; green #4CB782 for running. Inter for UI, JetBrains Mono for code, paths and commands. 6–10px radii, no shadows, no gradients.

STORY: The owner returns, reads the inbox sidebar first, clears an item in place (keys, Land, Prune) or opens its agent, then reads the lead's conversation and todo progress and sends a message.

FIRST VIEWPORT: 44px top bar (Dogsled, host load bars). Below it four columns, each with a 52px header row aligned across the screen: 280px inbox sidebar on the raised panel ("Waiting on you" and a yellow count; one row per item: kind icon, kind, age, title, agent · repository, inline actions), 248px agents sidebar (repository → lead → muted workers), conversation (title, state, role · harness · model · host · branch, icon actions; messages newest at bottom, 640px measure; composer pinned at the bottom with attach and send icons only), 300px todos column (in progress, pending, completed, abandoned). A plan review replaces the agents, conversation and todos columns: plan at a 680px measure, 300px notes margin, decision bar pinned at the bottom; the opened inbox row carries a yellow left edge.

FORM: The category standard (standing exit, chosen on safer re-roll 1); bar: Linear, Cursor, Conductor. seed:e4107608a77f64a0. Sketches: `.impeccable/mocks/decision/v6-console.html` (main screen) and `v6-console-review.html` (plan review open). `v5-canon.html` is the card as chosen, before the layout steer.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

## Unresolved

- Code-block syntax highlighting: no library chosen yet.
