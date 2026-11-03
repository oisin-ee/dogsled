# Portal redesign: the main screen

Status: approved by the owner on 2026-11-02 ("go"). Implementation is in progress on branch `plan/portal-console`.
This revision replaces the 2026-10-27 layout (top bar, 288px navigation, Inbox popover, tabbed agent screen).
Product context: `PRODUCT.md`. Surface brief and direction contract: `.impeccable/surfaces/web.md`.
Sketches: `.impeccable/mocks/decision/v6-console.html` (main screen) and `v6-console-review.html` (plan review open). `?tab=` in the sketch URL picks the open tab.
Wireframe: `plans/portal-redesign.squig.json`, one Squig canvas with one variation per frame: `wide-main`, `wide-review`, `wide-empty`, `medium-chat`, `medium-todos`, `narrow-chat`, `narrow-agents` and `narrow-review`. Open it with `pnpm squig serve <absolute path>` from a Squig checkout.
Screenshots: `v6-console.png` (wide, 1440×900), `v6-console-medium*.png` (1280×800), `v6-console-narrow*.png` (1024×768), `v6-console-review.png` and `v6-console-review-narrow*.png`.
The owner chose the dark console card (`v5-canon.html`), then on 2026-11-02:
- moved the inbox from a top strip into a second left sidebar, as in the Pull Request card;
- set the plan review to cover everything except the inbox sidebar;
- kept branches to land out of the inbox, because agents land their own branches;
- asked for a tabbed layout on smaller screens.
Other sketches from the round stay in `.impeccable/mocks/decision/` (`v4-*`, `v5-*`). They carry no approval.

## Why

The running portal (`127.0.0.1:7465`, screenshot from the owner on 2026-11-02) has these problems:

- The whole document scrolls, so the header scrolls away. Since the component migration (2026-11-02) the page frame uses only the upstream components theme and browser defaults (`web/assets/dx-components-theme.css`, loaded at `web/src/app.rs:268`). Nothing fixes the frame to the viewport; `web/assets/main.css` was removed.
- The inbox is a `Sheet` behind an Inbox button (`web/src/app.rs`). The owner cannot see waiting items without opening it.
- Todos and the branch are tabs behind the conversation (`web/src/agent_view.rs:483-485`).
- Markdown is parsed with `Parser::new` and no options (`web/src/review_view.rs:247`), so tables, strikethrough and task lists do not parse.
- The composer carries helper text ("Enter sends · Shift+Enter adds a line · …", `web/src/agent_view.rs:679`) and a text Send button.

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
4. **Conversation, centre (about 610px at 1440).** Header: title, state, `role · harness · model · host · branch`, and icon buttons for Interrupt and Open in Herdr. Messages sit newest at the bottom at a 640px maximum measure: owner messages as right-aligned bubbles, agent turns as rendered Markdown, tool calls as one mono line each, and a live status line. The composer is pinned below: an attach icon (images through `POST /api/attachments`, shown as thumbnails above the field), the field and a send icon.
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
- Syntax highlighting: `syntect` 5.3.0 (`html`, `regex-fancy`, no defaults) with the `two-face` 0.5.2 grammar set (`syntect-fancy`, no defaults). It is the only candidate with no C toolchain that covers rust, bash, toml, json, yaml, typescript, diff, markdown, python and sql. Tree-sitter options (syntastica, arborium, tree-sitter-highlight, inkjet) compile C grammars; giallo needs Oniguruma. Output is CSS classes with a `syn-` prefix, coloured in `markdown.css`. Measured cost (unit 4): the release `.wasm` grows from 3,984,587 to 6,173,363 bytes (brotli 0.75 MB to 1.85 MB, gzip 1.0 MB to 2.3 MB). The grammar dump is already compressed, so transfer compression saves little.

## Components

Keep the official Dioxus components in `web/src/components/` (from `plans/dioxus-components.md`): `SidebarProvider`, `Sidebar`, `Item`, `Tabs`, `Textarea`, `Button`, `Badge`, `ScrollArea`, `Progress` and the rest. No hand-built controls, and the component files stay as installed.
The owner rejected the plain upstream look (2026-11-02), so the console adds portal stylesheets loaded after `dx-components-theme.css`:

- `web/assets/console.css`: the theme variables set to the console palette and fonts, the fixed page grid and its three widths, and the shell, conversation layout and todos.
- `web/assets/markdown.css`: rendered Markdown and code colours.
- `web/assets/review.css`: the plan review and question layouts.

Overrides target the components' own classes and data attributes. Icons come from `dioxus-icons`.

## Units

Briefs are in `plans/briefs/portal-console-*.md`. Progress: units 1 to 5 are done; unit 6 (finish) is next. Unit 3 merged at `026abee` (bundle `.wasm` 3,984,587 bytes; no document scroll at 1440×900, 1280×800 and 1024×768). Unit 4 merged at `4dc16b9` (`.wasm` 6,172,931 bytes; highlighted code and tables checked in a live chat at the same widths). Unit 5 merged at `af8a047`, with the lead fix `a58fbbb` that keeps the review grid inside a 744px column at 1024 wide. A two-revision fixture review and a live question form were checked at all three widths. The finish audit (2026-11-03) is in `plans/briefs/portal-console-finish.md`. `impeccable detect` reports only Inter, which the contract chose; `.impeccable/config.json` records that.

1. **Wireframe** (lead). The Squig canvas `plans/portal-redesign.squig.json`: wide 1440×900, medium 1280×800 and narrow 1024×768, with the empty-inbox and plan-review states.
2. **Highlighting choice** (selection-lead, read-only). One wasm-ready Rust highlighter for fenced code (`portal-console-highlighting.md`).
3. **Main screen** (writer). `console.css`, the fixed grid and its three widths, the top bar, the inbox sidebar (replaces the `Sheet` and the Inbox button), the agents sidebar, and the lead panel: header with tabs, conversation, icon-only composer and the todos column. Owns `web/src/app.rs`, `web/src/board.rs`, `web/src/agent_view.rs` and `web/assets/console.css`.
4. **Markdown** (writer). Parser options, the chosen highlighter, message styles and `markdown.css`. Owns the render functions in `web/src/review_view.rs`, the `Conversation` component in `web/src/agent_view.rs`, and `web/Cargo.toml`.
5. **Plan review and questions** (writer). The review over everything right of the inbox sidebar, the `Notes` tab below 1100px, questions and key prompts in the centre column, and `review.css`. Owns the `ReviewView` components in `web/src/review_view.rs` and `web/src/question_form.rs`.
6. **Finish** (lead, then writer). The lead runs `impeccable detect --json web/` and the finish review against the sketches. The writer fixes what they found (`portal-console-finish.md`). After that merges, the lead writes `DESIGN.md` from the final tokens.

Unit 3 starts after 1. Unit 4 starts after 3 merges and uses 2. Unit 5 starts after 4 merges, because both edit `web/src/review_view.rs`. Unit 6 starts after `origin/main` merges into `plan/portal-console`, because it edits `src/web/inbox.rs`, which `main` changed.

## Decided

- **Highlighter bundle cost** (owner, 2026-11-03). Keep every two-face grammar. Highlighting adds about 1.1 MB (brotli) to the bundle the portal serves over loopback; syntect's own set is about 360 KiB but has no TOML or TypeScript.

## Open decisions

- **Tool lines.** Transcript turns carry only `role` and `text` (`portal-types/src/agents.rs:57`), so the conversation cannot show tool calls as mono lines yet. That needs a transcript change on the server and is not in units 3 to 5.
- **Model and host in the lead header.** `AgentView` (`portal-types/src/agents.rs:22`) has `agent` (the harness) but no model or host, so the header shows `role · harness · branch`, and the host only for workers (from `RowView.host`). Showing the model needs the server to report it.
- **Completed todo order.** Todos carry no completion time, so "latest first" reverses the server's order. That order is an estimate.
- **Attachments to remote agents.** The composer attach control reuses `POST /api/attachments` and sends the stored paths as a "Reference images:" block, as review change requests already do. The files are stored on the portal host, so an agent on another host cannot open them.

## Verification

- At 1440×900, 1280×800 and 1024×768, `document.scrollingElement.scrollHeight` equals the viewport height, and the top bar and composer stay in place while the conversation scrolls.
- At each width the inbox sidebar shows every item. At 1280 the `Chat` and `Todos` tabs switch the lead panel; at 1024 the `Agents` tab also shows the agents list, and selecting a lead there opens `Chat`.
- With twenty inbox items, the inbox header and count stay in place, each row is reachable by scrolling the sidebar, and each row opens its agent.
- With a plan review open, the inbox sidebar stays visible and usable, and no part of the agents, conversation or todos columns shows.
- A Markdown fixture with a heading, list, inline code, fenced code and a table renders each element.
- The composer contains no visible text apart from what the owner types.
- With Herdr unreachable, the composer and key buttons are disabled.
- Build each worktree's bundle with its own Cargo target directory. `#[css_module]` class names hash the absolute CSS path (`manganis-core` 0.7.10 `create_module_hash`), so a target directory shared with another worktree serves stale component CSS whose class names do not match the wasm.
