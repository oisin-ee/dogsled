# Portal redesign: the agent screen

Status: approved by the owner on 2026-10-27. Product context: `PRODUCT.md`.
Mocks (approved direction, final; no Squig pass): `.impeccable/mocks/decision/v3-*.html` and their `.png` screenshots.

## Layout

One screen, three regions, 1440×900 first.

1. **Top bar.** `Dogsled`, an **Inbox** button with a count, and the host load of `local`, `netcup`, `momokaya-2` (busy/cores bar and number).
2. **Left navigation (288px), always shown.** Every repository, with its agents under it.
   - A lead (an agent the owner works with directly) is a full row: status dot, task title, `role · host`, and a tag when it waits on the owner (`Plan`, `Question`, `Prompt`).
   - Its workers sit under it, indented and compact: one 18px line, a 6px grey dot, the title in `--overlay1`, and a short right-aligned status (`working`, `done`, `job`) in a muted colour. Worker status is not coloured.
   - A worker is raised in yellow only when the owner must act: a key prompt that only the owner can answer (`prompt`), or the worker is idle and its lead has not picked it up (`idle 18 h · unseen`).
   - A repository ends with a `To land · N branches` line when it has finished branches.
   - Rows never show unit IDs. Rows with no known repository are attributed (below); no "Unsorted" or "Older" bucket.
3. **Agent screen (main).** Header: title, state (yellow when waiting on the owner), `role · harness · model`, host, branch, and actions. Tabs: the pending item first (when there is one), then `Conversation`, `Todos N`, `Branch`.
   - **Plan review pending** (`v3-plan`): the plan fills the screen, Plannotator style. The document is centred at 720px, 16/27 type. Selecting text opens a note popover (quote, note, images, `Quote in message`, `Save note`). Notes sit in a 300px right margin with a whole-plan note. `Request changes` and `Approve plan` are in the header. A one-line bar at the bottom shows the lead's last turn and an "Ask the lead about this plan" field.
   - **Questions pending** (`v3-question`): the interview fills the screen, pi-interview style. Centred 760px form, numbered cards, `Recommended` badges, code and media content, `Send answers` and `Answer in conversation instead`.
   - **Key prompt pending** (`v3-blocked`): the prompt text from the screen, one button per option read from it, Esc/Enter/arrows, and a reply field.
   - **Nothing pending** (`v3-conversation-inbox`): the conversation, centred at 760px, newest at the bottom, tool calls as mono lines, a live status line, and the composer ("Message this agent"; Enter sends, Shift+Enter adds a line, it waits for the next turn while the agent works). `Interrupt` and `Open in Herdr` in the header.
4. **Inbox.** The button opens a 520px panel listing every plan review, interview and owner-only prompt across all agents, newest first: kind, title, `agent title · repository`, age. Selecting an item opens that agent with the item's tab in front. The count is honest: reviews + interviews + owner-only prompts + unseen idle workers.

When an item is answered in either place it leaves both the Inbox and the agent's tab.

## Failure and empty states

- Herdr down: the navigation still lists ledger rows; the agent screen says "Herdr is not reachable. The board still works; conversation and messages do not." and disables the composer and key buttons.
- Stream down: a thin top-bar notice "Live updates paused, retrying"; data stays visible.
- Empty inbox: the panel says "Nothing is waiting on you."
- No agent selected: the screen shows the inbox items inline, or "No agents are running" with the start command.

## Data the client needs (server and `portal-types`)

| Need | Today | Change |
| --- | --- | --- |
| Lead → worker tree | Board `Row` has no owner; `items.rs` has `owner_pane` | Add `owner_pane: Option<String>` to `board::Row` and `RowView`. |
| Leads | Leads are Herdr agents, not ledger items | Navigation joins `/api/agents` (leads, by `cwd` → repository) with board rows (workers, by `owner_pane`). Add `repository: Option<String>` to `AgentView`, resolved from `cwd` the same way rows get it. |
| Task title | Rows carry `u-…` names | Add `title: Option<String>` to `RowView`: the todo `current` for the row's pane, else the unit contract title, else the name. |
| Repository for null rows | 56 rows with `repository: null` | Resolve from the item's worktree or branch at board build time; the remainder are cleanup debt, not a bucket. |
| Cleanup cause | none | Add `cleanup: Option<CleanupView>` to `to_land` rows: `merged_local_not_pushed`, `never_merged`, `dirty`, with the clearing action. |
| Unseen idle worker | `quiet_ms` only | Add `unseen: bool`: a worker is idle or done and its owner has not read or closed it since. |
| Inbox | separate `/api/reviews`, `/api/questions`; prompts in `/api/agents` | Add `GET /api/inbox`: the pending items above with `pane`, kind, title, age, and the count; publish changes on `/api/stream`. |

## Client modules (`web/`)

- `app.rs`: top bar, inbox panel, `DeepLink` (`review`, `question`, `pane`) opens the agent with the item's tab.
- `board.rs` → navigation: repository → lead → compact workers.
- `agent_view.rs`: header, tabs, pending-item precedence (review, then questions, then prompt), conversation and composer.
- `review_view.rs`: full-screen document with selection popover and margin notes.
- `question_form.rs`: full-screen card form with media (owned by the `fix/portal-question-media` unit first).
- CSS in `web/assets/`: Catppuccin Mocha tokens, Inter and JetBrains Mono, 1px rules, radius 4px (6px on cards and popovers).

## Order

1. `fix/portal-question-media` (running): question media and info panels, and the card form styling.
2. Server data: the table above, in `portal-types`, `src/board.rs`, `src/web/{board,agents,mod}.rs`, plus a new `src/web/inbox.rs`.
3. Client redesign on top of 1 and 2.
4. `impeccable detect --json web/` on the finished UI, then fix its findings.

The land and prune lifecycle fix that stops `to_land` debt from building up is separate work.
