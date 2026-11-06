# Portal Git files sidebar

Status: the owner selected B, **By Stage**, on 2026-11-06 and authorized the implementation plan.
The implementation plan follows. Production work has not started.

## Scope

The portal gets a Files tab beside Todos in the existing right sidebar.
The list shows the selected agent's checkout on its host, not combined worker checkouts.
Rows show repository-relative paths and Git status only. They do not open files or diffs.
Staging, discard, committed task history, and the Herdr sidebar remain outside this scope.

The design question is how to show file paths and two Git status fields within the existing 300px sidebar.
The mockups preserve the console palette, typography, inbox, conversation, and composer.
They extend the current `web/src/agent_view.rs` layout without changing production files.

## Artifacts

- Interactive mockups: [index.html](../.impeccable/mocks/git-files-sidebar/index.html).
- Side-by-side comparison: [comparison.png](../.impeccable/mocks/git-files-sidebar/comparison.png).
- Full portal: [desktop.png](../.impeccable/mocks/git-files-sidebar/desktop.png).
- Compact portal: [compact.png](../.impeccable/mocks/git-files-sidebar/compact.png).
- Editable wireframes: [git-files-sidebar.squig.json](git-files-sidebar.squig.json).

The HTML file works locally without a server, network connection, or build.
Its review toolbar is not part of the proposed product UI.
All file statuses, branch names, conversations, and task counts are synthetic examples.
The screenshots come from this HTML, not image generation.

## Alternatives

| Option | Benefit | Trade-off |
| --- | --- | --- |
| A: Flat list | One row per file, with separate index and worktree status | Readers must recognize Git status letters |
| B: By stage | Explicit Staged, Unstaged, Untracked, and Conflicts groups | A file with both staged and unstaged changes appears twice |
| C: By folder | Shared path prefixes appear once | Folder headings take more room for small changes |

B, **By Stage**, is the selected design. It replaces the earlier recommendation for A.
Conflicts appear first when present, followed by Staged, Unstaged, and Untracked. Empty groups stay hidden.
Each group shows its own count. The Files tab counts unique paths, including files that appear in both stage groups.
The comparison artifacts retain all three alternatives. The desktop and compact screenshots show the earlier A preview, not the selected design.
The dot in options A and C makes an unchanged status field visible. It is a display convention, not raw Git output.
The Files count is the number of unique paths. Option B has seven paths across eight rows in the default example.
Renames show the previous path below the current path.
Conflicts have a text label, not color alone.
Git status markers stay neutral to preserve the console's existing color meanings.

## Behavior and responsive layout

At 1400px and wider, Todos and Files share the 300px right sidebar.
Below 1400px, Files joins Chat and Todos in the conversation header.
Below 1100px, Agents also becomes a header tab. The inbox stays visible.
The minimum product width remains 800px. Phone layouts remain outside scope.

The selected checkout, branch, and host appear above the files.
File lists scroll within their panel. The composer stays at the bottom of the viewport.
Rows are not links or buttons. Paths wrap instead of losing required text.
Todos remains accessible from the same tab strip.

The state selector demonstrates changes, clean, loading, stale, unavailable, non-repository, conflict, and long-list states.
Stale data retains its last snapshot with an explicit label.
An unavailable status never appears as a clean checkout.
The mockup does not fetch Git data or send messages.

## Evidence

Impeccable's Operate, new-work, and craft-floor guidance informed this extension of the existing console.
Visual references were `DESIGN.md`, current portal code, and the running portal on 2026-11-06.
The HTML uses plain controls for design review only. Production controls must use the existing Dioxus components.

Browser inspection covered the comparison at 1440×1020 and the portal at 1600×1000, 1000×900, and 800×900.
The compact portal fits the viewport without document overflow. Its inbox and composer remain visible.
All eight fixture states showed distinct content without undefined labels.
Default row counts were 7, 8, and 7 for A, B, and C.
The long-list fixture has 37 unique paths and scrolls inside the panel.
Squig measured all text nodes without overflow or missing glyphs. The canvas has 63 editable nodes across four variations.

These checks prove the design artifact only. They do not prove Git collection, remote delivery, or production accessibility.

## Resume the editable canvas

From the existing Squig checkout:

```sh
pnpm --dir /Users/oisin/dev/squig squig serve /Users/oisin/dev/dogsled/plans/git-files-sidebar.squig.json
```

The companion prints a new local editor link. Its token is not stored in this plan.
The companion used during review stops after the saved artifacts are verified.

## Implementation plan

Source review: `01423b6`, 2026-11-06. This section replaces the plan committed in `dfe5c0b`.
That plan routed Files around a defect in agent-view updates instead of fixing it, and added a second observer and SSH relay.
The scope remains read-only paths and Git status. No model generates summaries, labels, or groups.

### The defect to fix first

The agent view reloads everything whenever anything changes.

- `src/observation/details.rs:187` sets `conversation_revision` to the whole detail revision.
  A screen change therefore looks like a conversation change.
- `src/web/agents.rs:711` emits an agent event on every detail revision.
  The event carries only state, blocked prompt, and session (`portal-types/src/agents.rs` `AgentEvent`).
- `web/src/agent_view.rs:258` answers every event with `refresh += 1`.
  One counter (line 131) drives the conversation, agent, screen, board, review, and question fetches (lines 145–205).
- `web/src/agent_view.rs:265` also bumps the counter every 5 seconds while the stream is live.
  Prompt, key, and question actions bump it again (lines 305, 322, 491).
- The board, agent list, and inbox already arrive on `/api/stream` and live in `web/src/app.rs` (lines 209–295).
  The agent view fetches them again through `/api/board` and `/api/agents`.
- `src/web/agents.rs:298–337` (`peer_events`) forwards every remote reply, changed or not, on each local portal publication.

Result: each terminal repaint of the selected agent downloads and re-renders its full transcript, plus the board and inbox lists.
Adding Files to this path would add a Git-triggered reload. Building a separate path around it would leave the defect in place.

### Design

**Per-part revisions.** The detail payload gets one revision per part: conversation, screen, and files.
A part's revision increases only when that part's content changes. `AgentEvent` carries these revisions.
The browser refetches a part only when its revision differs from the one it holds.

**One source per datum.** The agent view reads agent state, the board, and the inbox from the existing portal stream context.
It stops fetching `/api/board` and `/api/agents`.

**Reviews and questions follow the inbox.** `web/src/review_view.rs:111–130` and `web/src/question_form.rs:66–79` poll every 2 seconds.
`ReviewView` also bumps a `tick` counter on every poll (line 123), which refetches the open review (line 193).
The portal snapshot already carries every pending review and question as an inbox item.
`InboxItem` gains a `revision`: the review's `revisions` count, or 0 for a question, which never changes.
The lists are refetched when the set of inbox ids changes. An open review is refetched when its inbox revision changes.
Both `POLL` constants, both loops, and `tick` are deleted.

**The blocked guard moves to the server.** The `send` closure (`web/src/agent_view.rs:267–290`) fetches the whole agent list before every prompt.
It refuses to send when the agent is blocked. The `prompt` route (`src/web/agents.rs`) forwards text without that check.
The route reads the agent's live Herdr state on its own host (`agent.list` locally, `herdr --machine <host> agent get` remotely) and answers 409 for a blocked agent unless the body has `reply: true`.
The client deletes its pre-send fetch and shows the 409 message.
A prompt that appears between the check and the send is still possible. This change does not claim to close that window; Herdr's `agent.prompt` has no conditional form.

**No timer while live.** The 5-second interval is removed.
When the agent stream fails or closes cleanly, the view shows the disconnect, reconnects with backoff, and then refetches all parts once.
A clean close is a disconnect, not an idle agent.
Actions do not trigger refetches; their effects arrive as events.

**Files is one more part, not a new pipeline.** The per-pane detail worker already owns viewer interest, session fencing, and the 5-second tick.
Git runs on a second thread inside that worker, so a slow repository never delays the conversation or screen.
The worker merges the latest files result into the detail payload with its own revision.
`GET /api/agents/{host}/{pane}/files` serves it through `detail_response`, like `conversation` and `screen`.
Remote agents use a new `PeerOperation::Files`, through the existing `peer_read`.

**Remote detail is a held subscription, not a poll.** `peer_events` (`src/web/agents.rs:298–337`) runs one SSH `agent-detail` call per local portal publication.
A new host command, `dogsled agent-detail <pane> --follow`, keeps the execution host's detail subscription open.
It writes one JSON line per changed part revision, read from that host's serve. It never reads Git or transcripts itself.
The portal server holds one such process per remote pane that has a viewer, shared by all its viewers.
The last viewer's disconnect kills the process group. A process exit or invalid line is a visible disconnect, followed by reconnect with backoff.
The conversation, screen, and files routes for remote agents keep using one-shot `peer_read`, triggered only by a changed revision.
An execution host without `--follow` reports "remote agent stream unsupported". It does not fall back to polling.

### Git capture

Run at the checkout root resolved from the agent's working directory:

```text
git --no-optional-locks -C <root> status --porcelain=v2 --branch -z --untracked-files=normal
```

- `--no-optional-locks` keeps the background read from taking the index lock while the agent commits.
- `--untracked-files=normal` is Git's default. An untracked directory is one row, as in `git status`.
- Parse ordinary, rename/copy, unmerged, and untracked records. Keep index and worktree codes separate.
  Keep the rename source. Treat all seven unmerged codes as conflicts, including `AA` and `DD`.
- Paths are exact. A path that is valid UTF-8 is a string. Any other path also carries its raw bytes,
  so two files never collapse into one row or one count.
- Branch headers give the branch, detached HEAD, or unborn HEAD. Nothing is invented.

`src/shell.rs:238` collects output with `wait_with_output` and has no size limit.
Add a byte limit to `execute_bytes` for all callers: stop reading at the limit, kill the process group, and return an error.
Files uses 1 MiB. Over the limit, Files shows "too many changes to list", never a partial list or a clean tree.

Read Git only through `src/shell.rs`. Do not use `src/loss.rs::dirty_paths`: it drops status codes and can filter deletions.

### States

| State | Meaning |
| --- | --- |
| Pending | No Git result yet for this session and checkout |
| Current | Git succeeded. Zero entries is the only state shown as "Working tree clean" |
| Stale | The last read failed. The previous snapshot stays, labelled with its capture time |
| Not a repository | The working directory is not inside a Git checkout |
| Unavailable | No working directory, or Git failed with no earlier snapshot |

Each result is tagged with the agent session and checkout root it was read for.
The worker discards a result whose session or root no longer matches. A stale snapshot is kept only for the same root.

### By Stage projection

One pure function in `portal-types` turns entries into groups and counts, so the server tests and the UI use the same rules.

1. Conflicts: all unmerged records, and nothing else.
2. Staged: non-conflict entries with an index change. Unstaged: non-conflict entries with a worktree change. A path can appear in both.
3. Untracked: untracked entries. Ignored files are never listed.
4. Empty groups are hidden. Each group is sorted by path and shows its own count.
5. The Files tab count is the number of unique paths.

### UI

- Add `Files` to `LeadPanel` in `web/src/app.rs`. Put the tab's rendering in `web/src/files.rs`.
- Use the existing Dioxus Tabs, Accordion, Item, ScrollArea, Skeleton, and Status components. Do not edit generated component sources.
- Files is a third tab in the right-column tab control that LR2 (`feat/lead-workers-tab`) adds: `Todos`, `Workers`, `Files`.
  It reuses LR2's segmented-tab component and `LeadPanel`; it adds `LeadPanel::Files`, not a second control. Todos is selected first.
  Below 1400px, Files joins LR2's header tabs after Workers. Resizing keeps an explicit Files selection.
- Rows are not interactive. Paths wrap. A rename shows its previous path underneath.
- Selecting another agent clears the shown files before the new agent's files arrive.

### Units

All units use the `writer` profile and run in order. Each starts from the revision that contains its predecessor.

| Unit | Deliverable | Owned scope | Needs | Acceptance |
| --- | --- | --- | --- | --- |
| U1 | Per-part detail revisions in agent events | `src/observation/details.rs`, `src/observation/tests/details.rs`, `portal-types/src/agents.rs`, `src/web/agents.rs` | — | A screen-only change leaves the conversation revision unchanged. The local event stream sends an event only when a part revision, state, blocked prompt, or session changes. |
| U2 | Held remote detail subscription | `src/web/agents.rs`, `src/cli.rs`, `src/main.rs`, `tests/cli/observation_portal.rs` | U1 | Two viewers of one remote pane share one SSH process. Unrelated portal publications start no SSH call. The last disconnect kills the process. Process exit and an unsupported peer appear as disconnects. |
| U1b | Portal publication revision changes only with content | `src/observation.rs` (`Publisher::replace`), its tests, and tests that assume a bump on every cycle | — | `Publisher::replace` always increments `revision` (`src/observation.rs:149`) and always sends (`:201`). A DETAILS-only wake (`src/service.rs:333-360`) therefore republishes identical content under a new revision, and every portal subscriber reloads. After U1b, an identical input leaves the revision unchanged and sends nothing. This also removes the race in `interests_scope_ticks_and_viewers_share_one_capture` (`src/observation/tests/details.rs:159`). |
| U3 | Prompt route refuses blocked agents | `src/web/agents.rs` and its tests | U2 | A prompt to a blocked local or remote agent answers 409 and reaches no Herdr call. |
| U4 | Agent view refetches only changed parts | `web/src/agent_view.rs` (`AgentSession` data hooks and actions only), `web/src/conversations.rs`, `web/src/api.rs`, `web/src/component_tests.rs` | U3, U1b, LR2 landed | `refresh`, `POLL`, and the interval are gone. A screen event refetches only the screen. Agent, board, and inbox come from the portal stream. Sending makes no agent-list request. Failure and clean close both reconnect and refetch once. |
| U5 | Reviews and questions follow the inbox | `portal-types/src/inbox.rs`, `src/web/inbox.rs`, `web/src/review_view.rs`, `web/src/question_form.rs`, `web/src/component_tests.rs` | U4 | Both 2-second loops and `tick` are gone. A new inbox id refetches the list once. A changed review revision refetches only that review. |
| U6a | Git status capture, wire types, By Stage projection (`plans/briefs/unit-files-capture.md`) | `src/shell.rs` (output limit for every caller), `src/error.rs`, new `src/git_status.rs`, new `portal-types/src/files.rs`, `portal-types/src/lib.rs`, `src/main.rs` module list | U4a | Real temporary repositories cover each state, the seven conflict codes, renames, staged-and-unstaged paths, non-UTF-8 names, unborn and detached HEAD, linked worktrees, and the byte limit. |
| U6b | Files detail part and route | `src/observation/details.rs`, `portal-types/src/agents.rs` (`DetailRevisions.files`), `src/web/agents.rs` | U6a, U5 | A blocked Git read does not delay the conversation part. A files change bumps only the files revision. A remote files change arrives through the U2 subscription. |
| U7 | By Stage Files tab | new `web/src/files.rs`, `web/src/app.rs`, `web/src/agent_view.rs` (right column and header tabs), `web/assets/console.css`, `web/src/component_tests.rs` | U6, LR2 landed | Component tests cover each state, group, and count. Browser check at 1600, 1400, 1200, 1000, and 800px against design B. A files change refetches only files. |

U1, U1b, and U2–U5 remove every forced reload in the agent view. U6a, U6b and U7 add Files on top.
U6 was split into U6a and U6b on 2026-11-06 because capture plus the detail part is more than one session's work. `FilesState` gains `TooMany` for the 1 MiB limit.

**Concurrent work on `origin/main` `aef6cce`** (briefs on `origin/docs/portal-mocks`, `plans/briefs/live-reasoning-*.md`):

| Branch | Overlap | Order |
| --- | --- | --- |
| `feat/live-reasoning-wire` (LR1) | `portal-types/src/agents.rs`: LR1 adds fields to `LiveState`, `ActiveTool`, `Role`, `ToolSummary`; U1 changes `AgentEvent` and adds `DetailRevisions` | Separate items. Whichever lands second rebases; no shared field. |
| `feat/lead-workers-tab` (LR2) | `web/src/agent_view.rs`, `web/src/app.rs`, `portal-types/src/lib.rs`, `web/assets/console.css`, `web/src/component_tests.rs` | U4 and U7 start from a base that contains LR2. U6 adds its `files` module export to `portal-types/src/lib.rs` after LR2. |
| LR3 (live reasoning stream) | `web/src/agent_view.rs` `Conversation`, `ToolRecords`, `LiveActivity` | U4 and U7 do not edit those components. |

U1–U3 touch no file LR2 or LR3 owns, so they continue now.
After U7, the lead does the integration review: a real edit in a test checkout reaches the running portal for a local and a remote agent.
The review also counts requests for 60 idle seconds with an open agent: zero conversation, board, review, or question fetches.

### Checks

Each unit runs its focused tests. The final revision runs:

```sh
moon run dogsled:check dogsled:web-lint
cargo test -p portal-types
cargo test -p web
```

No production code has changed. No implementation test has run yet.
