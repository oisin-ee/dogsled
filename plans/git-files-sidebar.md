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

Source review: `128f2d3`, 2026-11-06. The selected layout is B, not the A previews in the saved desktop screenshots.
The scope remains read-only paths and status. No model generates summaries, labels, or groups.

### Ownership and approach

The missing capability is a full checkout-status observation, not another renderer for the existing dirty flag.
`src/facts.rs::worktree` covers linked worktrees and returns cleanup facts.
`src/loss.rs::dirty_paths` drops status metadata and can filter deletions. Neither is the Files data source.

`serve` owns Git reads, sampling, checkout identity, and publication.
A file observer lives beside the existing detail observer in `src/observation/files.rs`.
It uses the existing terminal capture and native-session identity. It does not discover agents independently.
A separate worker prevents a slow Git command from delaying transcript delivery or the main observation loop.
The HTTP handlers and browser consume its publications. They do not run Git, rediscover hosts, or read foreign filesystem paths.

The observer shares a capture across viewers of the same checkout on one execution host.
The canonical Git administrative directory identifies the checkout, not the repository origin or branch name.
Different worktrees of one repository remain separate. Several panes in the same checkout share file facts but retain distinct pane/session envelopes.
Opening a selected agent establishes interest so its Files count can update while Todos is visible.
Closing the agent view releases that interest. The last viewer stops its worker and any owned subprocess.

Use a dedicated files channel through the existing observation IPC socket.
Do not attach file payloads to the transcript payload or change `AgentEvent` to refresh conversations whenever Git changes.
The service supplies the existing native-session binding to this channel. File-specific checkout binding does not become another session authority.

| Approach | Decision |
| --- | --- |
| Browser or HTTP-handler Git polling | Reject: multiplies collection and breaks producer ownership |
| Add full lists to every fleet snapshot | Reject: collects unviewed checkouts and enlarges unrelated publications |
| Run Git synchronously in the transcript worker | Reject: delays conversation loading and detail publication |
| Serve-owned, viewer-scoped file channel | Proposed: shares collection and isolates Git latency |

Memory guidance agrees with this boundary: **One observer per host** and **Conventions and patterns** assign observation to `serve`.
Current source evidence is `src/observation/details.rs`, `src/observation/ipc.rs`, and `src/service.rs`.

### Git capture and typed data

Use the existing byte-preserving process boundary in `src/shell.rs`.
It already removes inherited Git routing variables and kills timed-out process groups.
Its current `wait_with_output` call does not cap output bytes. Add a bounded-output path there before using it for file enumeration.
Do not add a second command runner inside the observer.

Resolve the checkout from the captured agent CWD, including subdirectories, ordinary checkouts, and linked worktrees.
`src/worktree.rs::snapshot` is existing read-only prior art that also supports unborn main checkouts.
Do not call registration helpers that write generation markers. A repository without an origin remains valid.
Keep checkout resolution errors distinct from a successful finding that the directory is not a repository.

Run the status command at the resolved root, with arguments passed separately:

```text
git --no-optional-locks -C <root> status --porcelain=v2 --branch -z --untracked-files=all --ignore-submodules=none
```

Parse bytes before producing display text. Preserve both status fields and rename source paths.
Recognize ordinary, renamed/copied, unmerged, and untracked records. Ignore unknown optional headers, not unknown entry formats.
Retain submodule flags and label submodule changes without recursively listing its contents.
Represent unborn and detached HEAD states explicitly. Do not invent a branch or require a valid HEAD commit.
Escape control characters and invalid UTF-8 bytes for display without merging distinct filenames into one row identity.
Never use an escaped display string as a filesystem path.

Define the wire contract in `portal-types/src/files.rs`, exported by `portal-types/src/lib.rs`:

| Field | Contract |
| --- | --- |
| Envelope | Protocol version, execution host, pane, native-session key, serve epoch, monotonic revision |
| Checkout | Opaque observation binding, root display path, branch state, optional HEAD |
| Entry | Stable byte-preserving path identity, display path, optional previous path, index/worktree codes, conflict and submodule details |
| Snapshot | Entries and last successful capture time, with exact unique-path count |
| Availability | Pending, current, stale with retained same-binding snapshot, not a repository, or unavailable with a typed reason |

The proposed resource limits are a five-second sample cadence, matching the current detail cadence, and no overlapping captures per checkout.
Use the existing shell command deadline policy, not a new retry timer that hides a failed read.
Cap raw status output at 1 MiB, diagnostic output at 64 KiB, and parsed entries at 10,000.
These are proposed engineering limits, not measured repository sizes.
A limit violation reports an explicit failure. A prior snapshot can remain visible only as stale data for the same checkout binding.
It never presents a partial list as complete or clean.
The JSON codec must bound escaped output and fit the existing 16 MiB observation frame ceiling.

Revalidate the captured checkout binding before publishing a completed command.
A pane/session change, CWD change to another checkout, checkout replacement, or publisher restart invalidates the old result.
Discard in-flight results for the former binding. Do not display its files under the new checkout heading.
A branch change in the same checkout publishes the new branch and status together.
Read failures retain a previous snapshot only for the same binding, with a stale label and its original success time.
Receipt time and keepalive traffic never become a successful Git capture time.

Primary Git evidence, checked against installed Git 2.56.0:
[porcelain v2](https://git-scm.com/docs/git-status#_porcelain_format_version_2),
[NUL path format](https://git-scm.com/docs/git-status#_pathname_format_notes_and_z), and
[background refresh](https://git-scm.com/docs/git-status#_background_refresh).
These document separate status fields, explicit unmerged records, rename paths, and the optional-lock restriction.
No new Git library or dependency is proposed.

### Local and remote delivery

Expose `GET /api/agents/{host}/{pane}/files/events` as a files-only SSE stream.
The first event is a complete state, including Pending if capture has not finished. Later events replace that state.
Use the existing same-origin/Host protections, no-store policy, SSE keepalive, and shutdown cancellation.
There are no path parameters supplied by the browser, write methods, or arbitrary Git arguments.

| Condition | Response |
| --- | --- |
| Known agent with a valid subscription | 200 SSE, event name `files`, versioned complete state |
| Malformed address | 400 using the existing error envelope |
| Positively absent host or pane | 404, not an empty file list |
| Local publisher unavailable or unsupported | 503 before stream start; an unavailable event and stream close after start |
| Remote transport unavailable | 502 before stream start; a stale/unavailable event and stream close after start |

A known agent without a checkout, a Git failure, and a non-repository directory are domain states inside the stream.
A successful empty Git result is Current with zero entries. Only that result means the working tree is clean.

Remote delivery needs a maintained remote interest, not repeated one-shot detail calls.
`src/web/agents.rs::peer_events` currently repeats a one-shot read on portal publications.
Copying that path would repeatedly acquire and drop file interest, tying Git reads to unrelated board traffic.
Instead, a serve-owned relay holds one remote files subscription per execution-host/pane pair and shares it among local viewers.
It uses the existing host inventory, remote argument construction, process environment isolation, and observation IPC.
A hidden files-stream host command forwards the execution host's publications without collecting Git itself.
The portal receives local IPC publications for both local and remote agents and does not spawn SSH per viewer.

The relay preserves execution-host epochs, revisions, native sessions, and checkout bindings.
It does not normalize remote paths on the portal host. Disconnects remain visible, and closing the last viewer cancels the remote process group.
An older peer without the files protocol is unavailable, not a reason to fall back to remote Git commands.
This feature does not rewrite the existing conversation or activity transport.

### By Stage projection and UI

One pure projection produces groups and counts from typed entries. It has no filesystem, network, or model access.
The shared contract owns the grouping rules so tests and renderers use the same policy.

1. Put all unmerged records in Conflicts only, including `AA` and `DD`, not only codes containing `U`.
2. Put non-conflict index changes in Staged and non-conflict worktree changes in Unstaged. A path can occur in both groups.
3. Put untracked paths in Untracked only. Exclude ignored paths.
4. Hide empty groups and sort each group by repository-relative path. Count unique paths in the Files tab and rows within each group.
5. Show a rename's current path and previous path. Keep status text accessible without color or a hover tooltip.

Use the portal's existing Dioxus Tabs, Accordion, Item, ScrollArea, Skeleton, and status components.
Do not copy the mockup's plain controls into production or edit the generated component sources.
Add `Files` to `LeadPanel` in `web/src/app.rs` and isolate file subscription/reducer/rendering code in `web/src/files.rs`.
`web/src/agent_view.rs` embeds it beside Todos and passes the selected address and existing session identity.

On wide screens, the right sidebar has a separate Todos/Files tab state from the main conversation view.
On narrower screens, Files joins the existing lead tabs. Resizing preserves an explicit Files selection without hiding the composer or inbox.
Todos remains the initial sidebar selection. Selecting another agent clears its file snapshot before accepting the new subscription.
Rows remain plain read-only content, with wrapping paths and bounded scrolling. No diff, staging, discard, or generated summaries appear.

A file-channel update does not increment the conversation refresh counter.
File transport failures affect only Files. They do not mark the agent idle, finished, or unreachable for conversation actions.
The reducer rejects out-of-order frames and mismatched bindings. A new epoch resets accepted revision state and invalidates prior current data.

### Ordered implementation units

These units are planned, not dispatched. All writers use the `writer` profile.
Each dependent unit starts only after its predecessor is accepted and available in its base revision.
Shared paths have sequential ownership rather than concurrent edits.

| Unit | Deliverable and owned scope | Prerequisite | Acceptance |
| --- | --- | --- | --- |
| F1 — Typed Git status capture | `portal-types/src/files.rs`, its module export, `src/git_status.rs`, its module registration, and bounded byte-output changes in `src/shell.rs` | None | Real temporary repositories prove status parsing, grouping, path identity, root resolution, deadlines, byte limits, and process cleanup. Wire round trips preserve all states. |
| F2 — Local files observer | `src/observation/files.rs`, files-related integration in `src/observation.rs`, `src/observation/ipc.rs`, `src/service.rs`, `src/service/wake.rs`, and observer tests | F1 | Two viewers share one capture. Unviewed checkouts have no files worker. Session/checkout rebinding discards old results. Slow Git cannot block transcript or board publication. Disconnect and shutdown release work. |
| F3 — Host-qualified delivery | `src/observation/files_remote.rs`, files-only integration in the F2 observer/service/IPC modules, files-stream wiring in `src/cli.rs` and `src/main.rs`, `src/web/files.rs`, router registration, and CLI/HTTP tests | F2 | Actual routes stream local and remote snapshots, share remote interest, preserve bindings and error states, and cancel on disconnect. More browser clients add no Git capture or per-client SSH work. |
| F4 — By Stage portal | `web/src/files.rs`, `web/src/api.rs`, `web/src/app.rs`, `web/src/agent_view.rs`, `web/assets/console.css`, module registration, and component/reducer tests | F3 | Selected design works at 1600, 1400, 1200, 1000, and 800px. Groups, duplicate stage membership, unique counts, stale/error states, keyboard tabs, and independent scrolling pass. File updates do not refresh transcripts. |
| F5 — Integrated proof | `tests/cli/observation_files.rs`, required test registration, and the Evidence section of this plan | F4 | A real Git edit reaches the running portal through serve and IPC, including a remote relay fixture. The final integrated revision passes the repository gates and a visual review against B. |

F1 tests cover main and linked checkouts, subdirectory CWD, no origin, unborn HEAD, detached HEAD, staged deletion, rename, copy records, and submodule flags.
Parser fixtures cover all seven conflict codes, filenames with spaces/newlines, invalid UTF-8 bytes, malformed records, and output limits.
Real Git cases prove staged-plus-unstaged membership, untracked files, and ignored-file exclusion.

F2 and F3 tests use deterministic clocks, manual ticks, acknowledgements, and call counters, following `src/observation/tests/details.rs`.
They also cover two panes sharing a checkout, two worktrees of one repository, a late old result, publisher restart, and remote host failure.
`tests/cli/observation_portal.rs` is the existing real-HTTP/IPC harness example.
F3 must include a transport fixture that executes the remote forwarding command, not only a mocked HTTP body.

F4 tests cover clean versus unavailable, retained stale rows, all groups, long paths, large lists, duplicate names in different directories, and resize transitions.
Browser checks use the repository browser workflow against an isolated build and test state, not the installed portal service.
No local installation, release, or fleet rollout is part of this work.

### Checks and review gate

Run focused tests for each unit at its exact revision. Preserve existing assertions and observer boundary tests.
For final integration, use the repository targets from `moon.yml`:

```sh
moon run dogsled:check dogsled:web-lint
cargo test -p portal-types
cargo test -p web
```

`dogsled:check` includes formatting, Clippy, Rust tests, and the Pi checks. Its Rust targets build the web bundle first.
Use `dx` for generated web assets. Do not edit generated output by hand.
Report unavailable checks or pre-existing failures separately. Start no hosted CI run solely to obtain evidence.

Source review found no unresolved product choice after the By Stage selection.
The independent files channel and shared remote relay are proposed architecture, not existing functionality.
Before dispatch, review their scope against this plan rather than silently replacing them with browser polling or one-shot remote reads.
Plan review against `128f2d3` found no dependency cycle or conflicting ownership after F3's observer integration paths were made explicit.
F1 through F5 are sequential. Current source confirms the nearby mechanisms, but no implementation test has run.
Production implementation and its runtime acceptance remain unperformed.
