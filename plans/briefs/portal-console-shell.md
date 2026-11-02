# Portal console: the main screen

Owner: lead (pane wWH:p36). Base: `origin/plan/portal-console` at the SHA in the spawn. Profile: writer. Unit 3 of `plans/portal-redesign.md`.

## Outcome

The portal opens as the dark console in `plans/portal-redesign.md` (Layout, Smaller screens, States): a fixed viewport grid with a 44px top bar, the always-visible inbox sidebar, the agents sidebar, the lead panel (header, conversation, pinned composer) and the todos column, folding into header tabs at 1100–1399px and 800–1099px. The document never scrolls.

## Design sources

- Contract: `.impeccable/surfaces/web.md` (palette, type, radii, no shadows or gradients, yellow only for "waiting on you").
- Sketches (match these): `.impeccable/mocks/decision/v6-console.html` (`?tab=` picks a tab) and its PNGs `v6-console.png`, `v6-console-medium*.png`, `v6-console-narrow*.png`, `v6-console-review.png`.
- Wireframe: `plans/portal-redesign.squig.json` (variations `wide-*`, `medium-*`, `narrow-*`).

## Current code (verified at the base)

- `web/src/app.rs` `App` (line 186): `SidebarProvider` > `Sidebar` (header with "Dogsled" and an Inbox button, repo groups, a footer with status and host load) > `SidebarInset` (selected `AgentView` or the "Waiting on you" card) and a `Sheet` (id `portal-inbox`) holding `InboxList`. Only `dx-components-theme.css` is loaded (line 268). Nothing fixes the frame to the viewport.
- `web/src/board.rs`: `navigation_groups`, `RepoGroup` (line 193), `NavigationAgent` (line 240), the per-repository `to_land` count.
- `web/src/agent_view.rs` `AgentSession` (line 112): one `ScrollArea` around `Tabs` with triggers pending / Conversation / `Todos {n}` / Branch (lines 476–486); header `Item` with Interrupt and "Open in Herdr" text buttons; `Conversation` (line 636) renders each turn as a `Card`; `Composer` (line 654) has a placeholder, helper text (line 679) and a text Send button; `BlockedPanel` (line 691); `Todos` (line 780).
- Components under `web/src/components/` use `#[css_module]`, so their class names are scoped. Restyle them by setting the theme variables and through their data attributes and `class`/attribute props. Leave `web/src/components/` and `web/assets/dx-components-theme.css` unchanged.

## Edits

1. Add `web/assets/console.css`, loaded after the components theme: console palette and fonts as theme variable values, the page grid (`100dvh`, `overflow: hidden` on the document, each region its own scroll), the three widths, top bar, sidebars, lead panel, composer and todos.
2. `app.rs`: replace the `Sheet` and the Inbox button with the inbox sidebar (header "Waiting on you" + count, rows with kind icon, kind, age, title, `agent · repository`). A prompt row shows the key buttons from that pane's `AgentView.blocked.options` (already in the `agents` signal) and sends with `api::send_key`. The open row gets the yellow left edge. Move host load and the stream status into the top bar. Keep the inbox, deep-link and selection logic and its tests.
3. `board.rs`: agents sidebar rows per the plan (status dot, title, todo progress; workers indented and muted; muted "N branch to land" line).
4. `agent_view.rs`: lead panel header (title, state, `role · harness · model · host · branch`, icon buttons for Interrupt and Open in Herdr with `aria-label`s). Todos become the right column at ≥1400px; below that, `Chat` / `Todos 28/32` tabs (and `Agents 7` below 1100px, which shows the agents list; picking a lead switches to `Chat`). Use the official `Tabs`. Drop the Branch tab; show the branch in the header meta. Composer: attach icon, `Textarea` with no placeholder, send icon; no helper text. Owner turns as right-aligned bubbles; agent turns without the card chrome, at a 640px measure. Keep `Conversation`'s call to `render_markdown`; unit 4 restyles Markdown.
5. Pending items: a plan review renders over everything right of the inbox sidebar (the agents column and lead panel hide). Keep `ReviewView { embedded: true }` and `QuestionForm` as they are inside; unit 5 restyles them. Questions and `BlockedPanel` replace the conversation in the centre column.
6. States from the plan's States section, including Herdr down (composer and key buttons disabled).
7. Icons come from `dioxus-icons` (enable the feature it needs in `web/Cargo.toml` only if no enabled set has the icon).

## Owned paths

`web/src/app.rs`, `web/src/board.rs`, `web/src/agent_view.rs`, `web/src/component_tests.rs`, `web/assets/console.css`, and the `dioxus-icons` line of `web/Cargo.toml` with `Cargo.lock`. Not `review_view.rs`, `question_form.rs`, `web/src/components/`, or the server.

## Acceptance

From the repository root: `moon run :web-lint :web-build` and `cargo test -p web` exit zero. Add a `dioxus-ssr` test that the composer renders no text and that the inbox renders with zero items. If a browser tool works on your host: run `cargo run -- web --port 47999` after the bundle and, at 1440×900, 1280×800 and 1024×768, report `document.scrollingElement.scrollHeight` against `innerHeight` (must be equal) with a screenshot path for each.

## Checks

Allowed: the acceptance commands, `cargo fmt -p web --check`, the browser check above. Not `moon run :check`; the lead runs it once. Commit hooks stay enabled. No suppressions or `#[allow]`. Publication: commit and push your branch only.

## Reply

Branch, SHA, changed paths, command results, the three scrollHeight readings, and any plan detail you could not meet with the official components (name the component and the gap).
