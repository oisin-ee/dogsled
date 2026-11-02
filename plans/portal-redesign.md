# Portal redesign: the main screen

Status: direction chosen by the owner on 2026-11-02. Implementation is not approved yet.
This revision replaces the 2026-10-27 layout (top bar, 288px navigation, Inbox popover, tabbed agent screen).
Product context: `PRODUCT.md`. Surface brief and direction contract: `.impeccable/surfaces/web.md`.
Chosen sketch: `.impeccable/mocks/decision/v5-canon.html` (screenshot `v5-canon-sketch.png`).
Other sketches from the round stay in `.impeccable/mocks/decision/` (`v4-*`, `v5-*`). They carry no approval.

## Why

The running portal (`127.0.0.1:7465`, screenshot from the owner on 2026-11-02) has these problems:

- The whole document scrolls, so the top bar scrolls away. `web/assets/main.css:29-39` sets `body { overflow: hidden }` and a `100dvh` grid, yet the page does not show that dark theme. The served build or its stylesheets do not match the source.
- The inbox is a button with a popover. The owner cannot see waiting items without opening it.
- Todos and the branch are tabs behind the conversation.
- Each turn is a bordered card headed "Agent". Markdown is parsed with `Parser::new` and no options (`web/src/review_view.rs:232`), so tables, strikethrough and task lists do not parse.

## Owner requirements

1. The inbox is never hidden. Every waiting item is visible and clickable at all times.
2. Todos, agent chat and the agents list are on the main screen.
3. The composer is pinned to the bottom. It has no helper text, only an attach icon, the field and a send icon.
4. Chat Markdown is rendered by a library, never by a hand-written parser.

## Direction

The category standard, played straight: a dark agent console at the craft level of Linear, Cursor and Conductor.
Palette, type and the full contract are in `.impeccable/surfaces/web.md`.

## Layout (1440×900 first, works from 1280×800)

The screen is one fixed grid. The document never scrolls. Each region scrolls inside itself.

1. **Top bar, 44px.** `Dogsled` and the host load of `local`, `netcup`, `momokaya-2` (bar and `busy/cores`). A stream outage shows "Live updates paused, retrying" here.
2. **Inbox strip, 132px, full width.** "Waiting on you" with the count, then one card per item: kind icon, kind, age, title, `agent · repository`, and inline actions (key buttons for a prompt, `Land` and `Prune` for a branch). Cards share the width (minimum 200px), so about seven fit at 1440px before the strip scrolls sideways. Selecting a card opens its agent with the item in front.
3. **Agents list, 260px, left.** Repository, then each lead (status dot, title, todo progress), then its workers indented and muted. A worker is raised only when the owner must act.
4. **Conversation, centre.** Header: title, state, `role · harness · model · host · branch`, and icon buttons for Interrupt and Open in Herdr. Messages sit newest at the bottom at a 680px measure: owner messages as right-aligned bubbles, agent turns as rendered Markdown, tool calls as one mono line each, and a live status line. The composer is pinned below.
5. **Todos, 320px, right.** The selected lead's todos: in progress (with its worker), pending, completed (latest first, count), abandoned (count).

Pending items keep their precedence: a plan review, then questions, then a key prompt replace the conversation in the centre column while they wait.

## States

- Herdr down: the agents list still shows ledger rows. The centre says "Herdr is not reachable. The board still works; conversation and messages do not." The composer and key buttons are disabled and lose their colour.
- Empty inbox: the strip shrinks to one line, "Nothing is waiting on you."
- No agent selected: the centre shows "No agents are running" with the start command, or a prompt to pick a lead.

## Data

The client already has what this layout needs: `GET /api/inbox` (`portal-types/src/inbox.rs`), `GET /api/agents` with `repository`, and board rows with `owner_pane`, `title`, `cleanup` and `unseen` plus todos keyed by pane (`portal-types/src/lib.rs`).
One gap: `InboxKind` has `review`, `question`, `prompt` and `unseen_worker`, but no `to_land`. The sketch shows a branch to land in the strip. Decide whether `to_land` rows join the inbox (server change) or stay in the agents list.

## Markdown

- Keep `pulldown-cmark` (0.13.5, already in `web/Cargo.toml`). Turn on its GitHub-flavoured extensions for tables, strikethrough and task lists. Confirm the option names against the 0.13.5 documentation in the implementing unit.
- Keep the existing safety rules in `render_markdown`: raw HTML stays text and unsafe links are dropped.
- Style headings, lists, inline code, code blocks and tables in the conversation stylesheet.
- Syntax highlighting in code blocks needs a maintained Rust library that builds for `wasm32-unknown-unknown`. None is chosen yet; research it before the conversation unit.

## Components

Keep the official Dioxus components from `plans/dioxus-components.md`. Restyle them through the theme stylesheets (`web/assets/components-theme.css`, `dx-components-theme.css` stays generated and unchanged). No hand-built replacements.

## Units (draft, for approval)

1. **Wireframe.** Save the layout as a Squig canvas at `plans/portal-redesign.squig.json` (desktop 1440×900 and 1280×800, with the empty-inbox and pending-review states).
2. **Shell.** Diagnose why the served portal scrolls and ignores `main.css`, then build the fixed grid, tokens, top bar, inbox strip and agents list (`web/src/app.rs`, `web/src/board.rs`, `web/assets/main.css`).
3. **Conversation.** Markdown options and styles, message layout, tool lines, status line and the icon-only composer (`web/src/agent_view.rs`, `web/src/review_view.rs` `render_markdown`, `web/assets/agent_view.css`).
4. **Todos column.** Move todos out of the tab into the right column.
5. **Pending items.** Fit the plan review, questions and key prompt into the centre column (see open decisions).
6. **Finish.** `impeccable detect --json web/`, the finish review against the sketch, and `DESIGN.md`.

Units 2 to 5 depend on 1. Unit 3 depends on the highlighting research.

## Open decisions

- **Plan review width.** The review needs about 1040px (720px document and a 300px notes margin). The centre column is about 860px at 1440px. Options: cover the todos column while a review is open, or put notes inline under each passage.
- **Inbox overflow.** Past about seven items the strip scrolls sideways, so some items leave the screen. Options: accept it, or wrap to a second row.
- **Branches to land in the inbox.** See Data.

## Verification

- At 1440×900 and 1280×800, `document.scrollingElement.scrollHeight` equals the viewport height, and the top bar and composer stay in place while the conversation scrolls.
- With eight inbox items, each card is reachable and opens its agent.
- A Markdown fixture with a heading, list, inline code, fenced code and a table renders each element.
- The composer contains no visible text apart from what the owner types.
- With Herdr unreachable, the composer and key buttons are disabled.
