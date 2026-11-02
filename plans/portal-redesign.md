# Portal redesign: the main screen

Status: direction chosen by the owner on 2026-11-02. Implementation is not approved yet.
This revision replaces the 2026-10-27 layout (top bar, 288px navigation, Inbox popover, tabbed agent screen).
Product context: `PRODUCT.md`. Surface brief and direction contract: `.impeccable/surfaces/web.md`.
Sketches: `.impeccable/mocks/decision/v6-console.html` (main screen) and `v6-console-review.html` (plan review open). `?tab=` in the sketch URL picks the open tab.
Screenshots: `v6-console.png` (wide, 1440×900), `v6-console-medium*.png` (1280×800), `v6-console-narrow*.png` (1024×768), `v6-console-review.png` and `v6-console-review-narrow*.png`.
The owner chose the dark console card (`v5-canon.html`), then on 2026-11-02:
- moved the inbox from a top strip into a second left sidebar, as in the Pull Request card;
- set the plan review to cover everything except the inbox sidebar;
- kept branches to land out of the inbox, because agents land their own branches;
- asked for a tabbed layout on smaller screens.
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

## Layout

The screen is one fixed grid. The document never scrolls. Each region scrolls inside itself.
The wide layout (1400px and wider, designed at 1440×900) is described here. Smaller screens fold columns into tabs; see Smaller screens.
Below the top bar are four columns. Each column starts with a 52px header row, and the rows line up across the screen.

1. **Top bar, 44px.** `Dogsled` and the host load of `local`, `netcup`, `momokaya-2` (bar and `busy/cores`). A stream outage shows "Live updates paused, retrying" here.
2. **Inbox sidebar, 280px, far left, raised panel.** "Waiting on you" with a yellow count. One row per item: kind icon, kind, age, title, `agent · repository`, and inline actions (key buttons for a prompt). The list scrolls vertically; the header and count stay. Selecting a row opens its agent with the item in front, and the open row gets a yellow left edge.
3. **Agents sidebar, 248px.** Repository, then each lead (status dot, title, todo progress), then its workers indented and muted. A worker is raised only when the owner must act. A repository with closed branches not yet landed shows a muted line such as "1 branch to land" (the existing per-repository count in `web/src/board.rs`).
4. **Conversation, centre (about 610px at 1440).** Header: title, state, `role · harness · model · host · branch`, and icon buttons for Interrupt and Open in Herdr. Messages sit newest at the bottom at a 640px maximum measure: owner messages as right-aligned bubbles, agent turns as rendered Markdown, tool calls as one mono line each, and a live status line. The composer is pinned below.
5. **Todos, 300px, right.** The selected lead's todos: in progress (with its worker), pending, completed (latest first, count), abandoned (count).

### Smaller screens

The inbox sidebar never folds. Other columns fold into tabs as the window narrows, so the conversation keeps at least about 570px.
The tabs are one segmented control in the conversation header, built on the official Dioxus `Tabs` component.

| Width | Columns | Tabs in the header |
|---|---|---|
| 1400px and wider | inbox, agents, conversation, todos | none |
| 1100 to 1399px | inbox, agents, one lead panel | `Chat`, `Todos 28/32` |
| 800 to 1099px | inbox, one panel | `Agents 7`, `Chat`, `Todos 28/32` |

- The header always shows the selected lead (title, state and meta, Interrupt, Open in Herdr). The title truncates before the tabs or icons do.
- The composer stays pinned under `Chat` and `Todos`. It is hidden under `Agents`.
- Selecting a lead in the `Agents` tab opens `Chat`. Selecting an inbox row opens its agent in `Chat`, or its pending item.
- Under 800px the page scrolls sideways. Phones are deferred (`PRODUCT.md`).

### Pending items

- **Plan review.** Opens over the agents, conversation and todos columns. Only the top bar and the inbox sidebar stay visible. The review has a header (title, revision, lead, `Plan` / `Changes` tabs, and a close icon), the plan at a 680px measure, a 300px notes margin, and a decision bar pinned at the bottom (note for the whole plan, `Request changes`, `Approve plan`). Closing it, deciding, or selecting another inbox row returns to the main screen. Below 1100px the notes margin folds into a third tab, `Notes 2`.
- **Questions and key prompts.** They replace the conversation in the centre column while they wait, in that order after a plan review.

## States

- Herdr down: the agents list still shows ledger rows. The centre says "Herdr is not reachable. The board still works; conversation and messages do not." The composer and key buttons are disabled and lose their colour.
- Empty inbox: the sidebar keeps its header with a zero count and shows "Nothing is waiting on you."
- No agent selected: the centre shows "No agents are running" with the start command, or a prompt to pick a lead.

## Data

The client already has what this layout needs: `GET /api/inbox` (`portal-types/src/inbox.rs`), `GET /api/agents` with `repository`, and board rows with `owner_pane`, `title`, `cleanup` and `unseen` plus todos keyed by pane (`portal-types/src/lib.rs`).
No server change is needed. Branches to land are not inbox items: agents land their own branches (owner, 2026-11-02). `InboxKind` stays `review`, `question`, `prompt` and `unseen_worker`.

## Markdown

- Keep `pulldown-cmark` (0.13.5, already in `web/Cargo.toml`). Turn on its GitHub-flavoured extensions for tables, strikethrough and task lists. Confirm the option names against the 0.13.5 documentation in the implementing unit.
- Keep the existing safety rules in `render_markdown`: raw HTML stays text and unsafe links are dropped.
- Style headings, lists, inline code, code blocks and tables in the conversation stylesheet.
- Syntax highlighting in code blocks needs a maintained Rust library that builds for `wasm32-unknown-unknown`. None is chosen yet; research it before the conversation unit.

## Components

Keep the official Dioxus components from `plans/dioxus-components.md`. Restyle them through the theme stylesheets (`web/assets/components-theme.css`, `dx-components-theme.css` stays generated and unchanged). No hand-built replacements.

## Units (draft, for approval)

1. **Wireframe.** Save the layout as a Squig canvas at `plans/portal-redesign.squig.json` (wide 1440×900, medium 1280×800 and narrow 1024×768, with the empty-inbox and plan-review states).
2. **Shell.** Diagnose why the served portal scrolls and ignores `main.css`, then build the fixed grid with its three widths, tokens, top bar, inbox sidebar, agents sidebar and the header tabs (`web/src/app.rs`, `web/src/board.rs`, `web/assets/main.css`).
3. **Conversation.** Markdown options and styles, message layout, tool lines, status line and the icon-only composer (`web/src/agent_view.rs`, `web/src/review_view.rs` `render_markdown`, `web/assets/agent_view.css`).
4. **Todos column.** Move todos out of the old tab into the right column, and into the `Todos` tab below 1400px.
5. **Pending items.** The plan review over everything right of the inbox sidebar (with the `Notes` tab below 1100px), and questions and key prompts in the centre column (`web/src/review_view.rs`, `web/src/question_form.rs`).
6. **Finish.** `impeccable detect --json web/`, the finish review against the sketch, and `DESIGN.md`.

Units 2 to 5 depend on 1. Unit 3 depends on the highlighting research.

## Open decisions

None. The code-highlighting library is research inside unit 3.

## Verification

- At 1440×900, 1280×800 and 1024×768, `document.scrollingElement.scrollHeight` equals the viewport height, and the top bar and composer stay in place while the conversation scrolls.
- At each width the inbox sidebar shows every item. At 1280 the `Chat` and `Todos` tabs switch the lead panel; at 1024 the `Agents` tab also shows the agents list, and selecting a lead there opens `Chat`.
- With twenty inbox items, the inbox header and count stay in place, each row is reachable by scrolling the sidebar, and each row opens its agent.
- With a plan review open, the inbox sidebar stays visible and usable, and no part of the agents, conversation or todos columns shows.
- A Markdown fixture with a heading, list, inline code, fenced code and a table renders each element.
- The composer contains no visible text apart from what the owner types.
- With Herdr unreachable, the composer and key buttons are disabled.
