# Portal redesign: the main screen

Status: direction chosen by the owner on 2026-11-02. Implementation is not approved yet.
This revision replaces the 2026-10-27 layout (top bar, 288px navigation, Inbox popover, tabbed agent screen).
Product context: `PRODUCT.md`. Surface brief and direction contract: `.impeccable/surfaces/web.md`.
Sketches: `.impeccable/mocks/decision/v6-console.html` (main screen) and `v6-console-review.html` (plan review open), each with 1440×900 and 1280×800 screenshots (`v6-console*.png`).
The owner chose the dark console card (`v5-canon.html`), then moved the inbox from a top strip into a second left sidebar, as in the Pull Request card, and set the plan review to cover everything except the inbox sidebar.
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
Below the top bar are four columns. Each column starts with a 52px header row, and the rows line up across the screen.

1. **Top bar, 44px.** `Dogsled` and the host load of `local`, `netcup`, `momokaya-2` (bar and `busy/cores`). A stream outage shows "Live updates paused, retrying" here.
2. **Inbox sidebar, 280px, far left, raised panel.** "Waiting on you" with a yellow count. One row per item: kind icon, kind, age, title, `agent · repository`, and inline actions (key buttons for a prompt, `Land` and `Prune` for a branch). The list scrolls vertically; the header and count stay. Selecting a row opens its agent with the item in front, and the open row gets a yellow left edge.
3. **Agents sidebar, 248px.** Repository, then each lead (status dot, title, todo progress), then its workers indented and muted. A worker is raised only when the owner must act.
4. **Conversation, centre (about 610px at 1440, 450px at 1280).** Header: title, state, `role · harness · model · host · branch`, and icon buttons for Interrupt and Open in Herdr. Messages sit newest at the bottom at a 640px maximum measure: owner messages as right-aligned bubbles, agent turns as rendered Markdown, tool calls as one mono line each, and a live status line. The composer is pinned below.
5. **Todos, 300px, right.** The selected lead's todos: in progress (with its worker), pending, completed (latest first, count), abandoned (count).

### Pending items

- **Plan review.** Opens over the agents, conversation and todos columns. Only the top bar and the inbox sidebar stay visible. The review has a header (title, revision, lead, `Plan` / `Changes since revision N`, and a close icon), the plan at a 680px measure, a 300px notes margin, and a decision bar pinned at the bottom (note for the whole plan, `Request changes`, `Approve plan`). Closing it, deciding, or selecting another inbox row returns to the main screen.
- **Questions and key prompts.** They replace the conversation in the centre column while they wait, in that order after a plan review.

## States

- Herdr down: the agents list still shows ledger rows. The centre says "Herdr is not reachable. The board still works; conversation and messages do not." The composer and key buttons are disabled and lose their colour.
- Empty inbox: the sidebar keeps its header with a zero count and shows "Nothing is waiting on you."
- No agent selected: the centre shows "No agents are running" with the start command, or a prompt to pick a lead.

## Data

The client already has what this layout needs: `GET /api/inbox` (`portal-types/src/inbox.rs`), `GET /api/agents` with `repository`, and board rows with `owner_pane`, `title`, `cleanup` and `unseen` plus todos keyed by pane (`portal-types/src/lib.rs`).
One gap: `InboxKind` has `review`, `question`, `prompt` and `unseen_worker`, but no `to_land`. The sketch shows a branch to land in the inbox. Decide whether `to_land` rows join the inbox (server change) or stay in the agents list.

## Markdown

- Keep `pulldown-cmark` (0.13.5, already in `web/Cargo.toml`). Turn on its GitHub-flavoured extensions for tables, strikethrough and task lists. Confirm the option names against the 0.13.5 documentation in the implementing unit.
- Keep the existing safety rules in `render_markdown`: raw HTML stays text and unsafe links are dropped.
- Style headings, lists, inline code, code blocks and tables in the conversation stylesheet.
- Syntax highlighting in code blocks needs a maintained Rust library that builds for `wasm32-unknown-unknown`. None is chosen yet; research it before the conversation unit.

## Components

Keep the official Dioxus components from `plans/dioxus-components.md`. Restyle them through the theme stylesheets (`web/assets/components-theme.css`, `dx-components-theme.css` stays generated and unchanged). No hand-built replacements.

## Units (draft, for approval)

1. **Wireframe.** Save the layout as a Squig canvas at `plans/portal-redesign.squig.json` (desktop 1440×900 and 1280×800, with the empty-inbox and plan-review states).
2. **Shell.** Diagnose why the served portal scrolls and ignores `main.css`, then build the fixed grid, tokens, top bar, inbox sidebar and agents sidebar (`web/src/app.rs`, `web/src/board.rs`, `web/assets/main.css`).
3. **Conversation.** Markdown options and styles, message layout, tool lines, status line and the icon-only composer (`web/src/agent_view.rs`, `web/src/review_view.rs` `render_markdown`, `web/assets/agent_view.css`).
4. **Todos column.** Move todos out of the tab into the right column.
5. **Pending items.** The plan review over everything right of the inbox sidebar, and questions and key prompts in the centre column (`web/src/review_view.rs`, `web/src/question_form.rs`).
6. **Finish.** `impeccable detect --json web/`, the finish review against the sketch, and `DESIGN.md`.

Units 2 to 5 depend on 1. Unit 3 depends on the highlighting research.

## Open decisions

- **Branches to land in the inbox.** See Data.
- **Narrow conversation at 1280.** With both sidebars and the todos column, the conversation is about 450px wide at 1280×800. Code blocks scroll sideways inside themselves. Decide whether that is acceptable or the todos column narrows below 1440.

## Verification

- At 1440×900 and 1280×800, `document.scrollingElement.scrollHeight` equals the viewport height, and the top bar and composer stay in place while the conversation scrolls.
- With twenty inbox items, the inbox header and count stay in place, each row is reachable by scrolling the sidebar, and each row opens its agent.
- With a plan review open, the inbox sidebar stays visible and usable, and no part of the agents, conversation or todos columns shows.
- A Markdown fixture with a heading, list, inline code, fenced code and a table renders each element.
- The composer contains no visible text apart from what the owner types.
- With Herdr unreachable, the composer and key buttons are disabled.
