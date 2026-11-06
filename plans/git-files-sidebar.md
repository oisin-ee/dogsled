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
Review and question lists are refetched when the inbox in the portal snapshot changes, not on agent events.

**No timer while live.** The 5-second interval is removed.
When the agent stream fails, the view reconnects with backoff and then refetches all parts once.
Actions do not trigger refetches; their effects arrive as events.

**Files is one more part, not a new pipeline.** The per-pane detail worker already owns viewer interest, session fencing, and the 5-second tick.
Git runs on a second thread inside that worker, so a slow repository never delays the conversation or screen.
The worker merges the latest files result into the detail payload with its own revision.
`GET /api/agents/{host}/{pane}/files` serves it through `detail_response`, like `conversation` and `screen`.
Remote agents use a new `PeerOperation::Files`, through the existing `peer_read`.

**Remote events.** `peer_events` sends an event only when the remote event differs from the last one sent.
Unchanged remote replies no longer cause browser refetches.
Its trigger stays the local portal publication. Replacing that trigger with a held remote subscription is a separate change, not part of Files.

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
- At 1400px and wider, Todos and Files are tabs in the right sidebar. Todos is selected first.
  Below 1400px, Files joins the header tabs. Resizing keeps an explicit Files selection.
- Rows are not interactive. Paths wrap. A rename shows its previous path underneath.
- Selecting another agent clears the shown files before the new agent's files arrive.

### Units

All units use the `writer` profile and run in order. Each starts from the revision that contains its predecessor.

| Unit | Deliverable | Owned scope | Needs | Acceptance |
| --- | --- | --- | --- | --- |
| U1 | Per-part detail revisions in agent events | `src/observation/details.rs`, `src/observation/tests/details.rs`, `portal-types/src/agents.rs`, `src/web/agents.rs` | — | A screen-only change leaves the conversation revision unchanged. Identical consecutive local or remote events are not sent twice. |
| U2 | Agent view refetches only changed parts | `web/src/agent_view.rs`, `web/src/conversations.rs`, `web/src/api.rs`, `web/src/component_tests.rs` | U1 | No 5-second interval. A screen event refetches only the screen. Board, agent, and inbox come from the portal stream. Stream failure reconnects and refetches once. |
| U3 | Files capture, part, and route | `src/shell.rs`, new `src/git_status.rs`, new `portal-types/src/files.rs`, `portal-types/src/lib.rs`, `src/observation/details.rs`, `src/web/agents.rs`, `src/main.rs` module list | U2 | Real temporary repositories cover each state, the seven conflict codes, renames, staged-and-unstaged paths, non-UTF-8 names, unborn and detached HEAD, linked worktrees, and the byte limit. A blocked Git read does not delay the conversation part. |
| U4 | By Stage Files tab | new `web/src/files.rs`, `web/src/app.rs`, `web/src/agent_view.rs`, `web/assets/console.css`, `web/src/component_tests.rs` | U3 | Component tests cover each state, group, and count. Browser check at 1600, 1400, 1200, 1000, and 800px against design B. A files change refetches only files. |

After U4, the lead does the integration review: a real edit in a test checkout reaches the running portal for a local and a remote agent.

### Checks

Each unit runs its focused tests. The final revision runs:

```sh
moon run dogsled:check dogsled:web-lint
cargo test -p portal-types
cargo test -p web
```

No production code has changed. No implementation test has run yet.
