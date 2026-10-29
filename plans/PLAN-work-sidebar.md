# Unified work sidebar and background-job replacement

## Context

Replace the three stacked Pi displays for todos, Wrangle agents, and background jobs with one persistent sidebar. The user prefers a sidebar over the previously recommended summary strip. This is planning only; implementation and installation are not authorized.

## Approach

Recommend a genuine right-hand companion sidebar hosted by Herdr, next to the current Pi pane. This reserves real terminal width and supports both Pi terminal modes without taking over Pi's renderer. The first version replaces pi-background-tasks, not merely its display. The user rejected being asked to choose a host surface without technical context; the recommendation below is based on release source inspection.

Keep todo state in pi-todo and execution state in Wrangle. The sidebar is a view, not a second task database. Original widgets yield through their owning extensions, not through repeated suppression by another extension.

### What runs where

Pi extensions do **not** run inside Herdr's native sidebar. This proposal adds a separate terminal application (`wrangle sidebar`) in a right-hand Herdr split. Existing Pi widget components are not embedded or transferred; the new application renders their underlying data.

```text
Pi process                                  Herdr companion process
pi-todo → pi.events → Wrangle Pi adapter
                         ↓ derived snapshot → wrangle sidebar
Wrangle service → agent/job records ────────→ lists and details
                 ↑ controls ──────────────── log / stop / cancel
                 ↓ durable inbox
             Wrangle Pi adapter → model notification
```

`pi.events` only connects extensions inside the Pi process. The owned snapshot publication is the cross-process bridge; it must be built. Job and agent controls use Wrangle directly. Todo updates remain with pi-todo; version one displays them rather than creating a second todo editor. The sidebar shows stale/disconnected state when Pi stops, while Wrangle jobs continue independently. This is a new integration, not configuration that makes arbitrary Pi extensions run in Herdr.

### Proposed sidebar behavior

- Right-hand Herdr split, initially 36 columns when space permits. Header identifies its Pi session. Bind to the launching session/pane rather than global focus; remote agents and jobs still appear under that owner.
- Sections: Needs attention, Todos, Agents, Jobs. One scrollable list with collapsible headings; keep counts visible, completed work collapsed, and failures visible until inspected.
- `Enter` opens details/logs within the sidebar; arrows/PageUp/PageDown scroll; explicit actions focus an agent or confirm stopping/cancelling work; Escape returns to the sidebar list. Opening or updating never steals editor focus.
- Closing the sidebar leaves jobs running and does not restore three stacked widgets. `/work` reopens it; `/todos list` and `/wrangle` remain explicit inspection commands. Narrow panes wrap/truncate safely, with Herdr zoom available; do not replace the requested sidebar with an automatic bottom dock.
- Implement the interactive view as `wrangle sidebar` in the existing Rust binary using released Ratatui/Crossterm. This keeps job/owner queries and deployment with Wrangle, without a new Node renderer process or raw ANSI/input implementation. This is a new UI-library dependency, not a copied renderer. Pi's standalone TUI is viable but would require a separate installed runtime/dependency graph because installed Pi is a compiled binary.

### State and presentation contract

- pi-todo publishes versioned, validated snapshots over `pi.events`, plus a request handler for initial state. Include session ID, active branch leaf, publisher generation, monotonic revision, and tasks. Publish after tool updates and session/branch transitions.
- Extend `TodoPresentation` with an explicit external-presentation mode. Wrangle publishes that mode only after its sidebar integration is configured; no direct manipulation of another extension's widget keys.
- Wrangle's Pi adapter stores an atomic, private **derived read snapshot** under its existing ledger, keyed by stable Pi session identity and registered terminal/pane. This is rebuildable display data, not authoritative task persistence. The current Pi process provides identity and validates branch data; the sidebar never guesses a leaf from the session file's last line.
- Publish on change and use a presence lease to label disconnected publishers stale. Reject older revisions/generations; session replacement clears or changes the binding. Closing/reloading Pi does not make stale tasks appear live.
- Use `board::build` for agent/job rows and extend its read model to include retained completed/failed jobs, session association and notification state. Legacy pane-owned items remain visibly labelled, not silently reassigned to a new session.

### Target layout

```text
Pi conversation                         │ WORK · current session
                                        │ ! reviewer needs input
Assistant output remains full height.   │
                                        │ TODOS · 3/7
                                        │ → Verify changes
                                        │   Update docs
                                        │
                                        │ AGENTS · 2
                                        │ ● builder · netcup
                                        │ ! reviewer · local
                                        │
                                        │ JOBS · 1
                                        │ ▸ tests · local · 2m
> Your message…                         │   Enter details · / filter
```

This is a conceptual layout, not a tested preview. Herdr owns the outer split; Pi remains unchanged inside its pane. There is no additional bottom task widget. Width is user-resizable; automatic opening applies to interactive lead sessions, not every delegated reader/writer. A child can explicitly open its own view.

### Execution replacement contract

- Extend the existing Wrangle run API with a mutually exclusive shell-command form, title, session identity, and notification settings. Keep argv launches intact. Shell jobs default to the caller's host and exact cwd; explicit remote jobs use the selected host's environment and show that host clearly. Queued jobs remain queued, never secretly fall back to a different executor.
- Preserve the caller's environment for local shell jobs through a permission-restricted launch record on that host, consumed by `wrangle exec`. Never put environment values in argv, status JSON, logs, or cross-host records. Remove the launch record after successful consumption/cancellation; pending queued records retain the same protections. Remote jobs do not forward the caller's environment. Validate shell, cwd, mode, and host before admission.
- Add a `job` Herdr plugin entrypoint with fixed argv `wrangle exec`; pass only a validated job ID/state-root reference in its launch context, not command text or environment secrets. It opens a no-focus tab through `plugin.pane.open`, loads the private launch record and reuses `jobs::capture`. This is new Wrangle integration with Herdr's existing argv pane support—not an existing Wrangle entrypoint. Retain admission/launch receipts and avoid shell text injection.
- Background shell jobs use nice 10, Linux best-effort ionice level 7, and normal macOS nice support. Do not raise host resource ceilings. Do not add the optional systemd transient-service backend; missing required helpers produce an explicit capability error rather than silently dropping the policy.
- Stop sends TERM to the verified process group, waits the configured grace (initially 5 seconds), then KILLs survivors. Track process identity so recovery never signals a recycled PID. Wrapper loss is `interrupted/exit unknown`, not success and not an automatic rerun.
- Jobs survive Pi reload/exit; the sidebar and session reader may disappear without killing them. Resume of the same session reclaims display and pending notifications. An unrelated new session does not receive the old session's events. Host reboot ends processes; reconcile and retain logs, never rerun commands automatically.
- Preserve output-triggered wakes: substring or bounded Rust regex, cross-chunk matching, debounce (2 seconds), `transition` / `first-match-only` / `always`, session-scoped dedupe, 2,000-character tails, ten wakes/20,000 inline-byte budget. Exit events bypass output budgets. Reject unsupported regex syntax explicitly; JavaScript lookarounds/backreferences are not silently reinterpreted.
- Persist job events before inbox delivery, using stable event IDs. Deliver through the existing inbox reader, steering busy Pi sessions and waking idle ones. Keep a job-event pending record through mailbox expiry and terminal replacement. Acknowledge only after transcript persistence; replay deduplicates already-persisted events.
- Preserve explicit background launch and arm-next-bash. Monitor auto-diversion runs in the supported Bash tool execution adapter **after guard hooks**, with the original foreground tool as delegate. Do not spawn processes in an observation hook. Check both direct Wrangle run calls and codemode calls through pi-guardrails.
- Remove `bg_task`, `bg_status`, `/bg`, and their old injected instructions at cutover. Teach the replacement Wrangle commands/tools; do not maintain competing aliases or registries. Keep Wrangle's unlimited default for generic argv jobs, but migrate this user's configured 1,800-second background-shell default, with explicit no-timeout for persistent servers/watchers.

### Sidebar lifecycle and ownership

- Add a `work` entrypoint to Wrangle's Herdr plugin. Bind it to the launching terminal's stable identity plus Pi session identity, not the pane that happens to be globally focused. Follow authoritative pane-move updates without silently switching to another session.
- Open the plugin split without focus, locate its returned pane/split in `pane.layout`/`layout.export`, then set only that split's ratio through `layout.set_split_ratio`. Never use `layout.apply` on the live tab: it replaces processes.
- Reuse an existing matching companion on reload/reconnect; close only a sidebar created by this integration. Manual close stays closed until `/work` opens it again. Sidebar failure is shown as an explicit Pi status, not a successful handoff or three resurrected widgets.
- Use authoritative Wrangle snapshots, Herdr lifecycle subscriptions and bounded refreshes. Pending remote reads never block keyboard input. Show stale/disconnected state, source host, and last successful refresh.
- Completed jobs remain inspectable from existing job records/logs after their tabs/items close. Start with the existing retention period; pending undelivered events must not be pruned. Closing the view never acknowledges a failure or stops work.
- Noninteractive modes create no sidebar. Without Herdr, report the dependency clearly; do not invent an unapproved fallback process manager.

## Files to modify

Paths marked **new** are proposed owned modules, not third-party patches.

| Owner                         | Critical paths                                                                                                                                                                                       |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| oisinpi todo                  | `packages/pi-todo/src/{index,lifecycle,presentation,state}.ts`; **new** `src/events.ts`; existing adapter/index tests plus event/presentation tests                                                  |
| oisinpi guardrails and bundle | `packages/pi-guardrails/src/pi/tool-inputs.ts` and its behavior tests; `packages/oisinpi/package.json`; `pnpm-lock.yaml`; package READMEs/changesets                                                 |
| Wrangle execution             | `src/{jobs,work,runtime,cli,mcp,config,service,inbox,watchdog,board}.rs`; **new** `src/job_events.rs` and `src/session_view.rs`; `tests/cli/jobs.rs`, watchdog/inbox tests                           |
| Wrangle UI and Pi adapter     | **new** `src/sidebar.rs`, `pi/work.ts`, `pi/background.ts`; `pi/{index,inbox,board}.ts` and tests; `herdr-plugin.toml`; `Cargo.toml`, `Cargo.lock`, `package.json`, README and owned skill           |
| Managed installation          | `/Users/oisin/dev/dotfiles/conf.d/harness/settings/pi/settings.json`, relevant harness/host mise pins and owned background-task guidance; update only canonical sources, using existing provisioning |

Wrangle is a separate owned checkout at `/Users/oisin/dev/wrangle`. Dotfiles is another owned checkout. Plan approval must cover those cross-repository changes; no dependency source or installed copies are edited.

Concurrent todo work was committed during research: the checkout is now `5a8f4e4099853a85bbdb93128718d195dc5a298b`, with only this plan untracked. `TodoPresentation` owns transient expansion and calls `replay()`; extend that owner rather than replacing it. `src/index.ts` refreshes after every tool call and resets on `session_tree`. Recheck HEAD before dispatch.

## Reuse

- `packages/pi-todo/src/state.ts`: `latestSnapshot()` and branch-backed `todo-state` entries.
- `packages/pi-todo/src/lifecycle.ts`: `replay()`, ownership detection, and unchanged reminder behavior.
- `/Users/oisin/dev/wrangle/pi/board.ts`: existing `Board`/`Row` data, `parseBoard()`, status grouping, terminal text sanitation.
- `/Users/oisin/dev/wrangle/pi/index.ts`: `wrangle board --json`, existing refresh/inbox lifecycle and panel integration.
- Wrangle job execution, records, saved logs, cancellation, and owner check-ins; do not duplicate the process runner.
- Herdr plugin split placement, argv-launched plugin tabs, and CLI/socket APIs for companion/job panes.

## Evidence and constraints

- Exact Pi v1.3.0 types DO expose `ViewportTUI.setLayoutRoot()` and extension factories receive the live TUI. The narrower blocker is preserving Pi's existing fullscreen root: there is no public getter. `pi-ui-extension@1.8.3` casts to `{ layoutRoot?: unknown }`, reads that private field, and also mutates private stack `entries`. See published commit `1257d0b6f572e69056968dbf020f7b966f7d5ee0`, `extensions/sidebar/index.ts:25,89-96,136-147,174-187,240-242`. The earlier claim that extensions cannot acquire the TUI was inaccurate. Do not copy this private-root technique or patch Pi. A non-capturing overlay does not reserve width.
- A separate terminal process could use Pi's exported `TuiAltScreen` and components without private host access. Prefer Ratatui inside the existing Wrangle binary to avoid a separate Node UI distribution. Ratatui provides terminal backends, lists/layouts and `TestBackend`; pin compatible released versions in Wrangle's Cargo lockfile. References: https://ratatui.rs/concepts/backends/ and https://docs.rs/ratatui/0.30.2/ratatui/backend/struct.TestBackend.html .
- The published `pi-cockpit@0.24.4` tarball confirms runtime renderer assignment in `src/split-pane.ts:283-307` (`nextTui.render = wrappedRender`), alongside a non-capturing overlay at lines 194-197. It is not merely a public layout composition. Source: https://registry.npmjs.org/pi-cockpit/-/pi-cockpit-0.24.4.tgz .
- Canonical background-task installation is in `/Users/oisin/dev/dotfiles/conf.d/harness/settings/pi/settings.json`, including an actual 1800-second default timeout and `belowEditor` placement. Migrate this owned configuration, not installed node_modules or generated instruction copies.
- On the lead's local host, Herdr client and server both report 0.9.5, protocol 22. The remote reader observed a 0.9.4 server on netcup; verify/align host versions through managed provisioning before deploying the new plugin, without restarting active work during planning. Plugin panes support `--placement split --target-pane <owner> --direction right --no-focus`. Release source `src/app/api/plugins/panes.rs:82-160` creates an argv-launched split, initially 50/50. Set the new split's ratio through the public layout API rather than pretending plugin `width` applies to splits.
- Herdr's native sidebar DOES support custom display tokens and configurable multi-line Agent/Space rows. This corrects the earlier overly broad statement about no sidebar extension surface. Tokens can show a summary or current todo, but do not create arbitrary selectable task/job lists or new panel sections. Source: `src/config/sidebar.rs` at v0.9.5 and the configuration guide's Sidebar row layouts.
- Herdr socket API supports long-lived event subscriptions (`pane.closed`, `pane.moved`, `layout.updated`, agent status changes). Earlier research claiming events are only command hooks was incomplete. Use authoritative snapshots on reconnect rather than treating events as durable state.
- Wrangle 0.11.0 already manages agents and jobs. Its existing Herdr board is a popup, not a unified todo sidebar.
- Wrangle's current `jobs::capture` kills the process group immediately on stop; change the owning executor to TERM, grace, then KILL. `watchdog::tick` withholds completion check-ins while the owner works; the Pi inbox consumer already supports steering a busy owner. New durable job events must feed the inbox independently of idle-only watchdog check-ins.
- Current inbox delivery can fail after eight unacknowledged claims or terminal disappearance. Job-event pending state must outlive those transport failures and rebind only when the same Pi session resumes. Do not claim exactly-once delivery across crashes; deduplicate stable event IDs against the session branch and acknowledge after the message is stored.
- Preserve output matching, debounce, wake budgets, resource priority and explicit/automatic background launch in the replacement. No second executor, `bg_*` compatibility registry, or third-party patch.
- Source-level execution evidence: Wrangle release `b76587134efc7c6d289061c2ade04498bbcbf00a`, `src/jobs.rs`, `src/work.rs`, `src/watchdog.rs`, `src/inbox.rs`, and `pi/inbox.ts`. The lead's local Wrangle checkout matches that release; the remote reader's older checkout was not used as authority.
- Public references: https://herdr.dev/docs/plugins/#panes ; https://github.com/oisin-ee/wrangle/blob/v0.11.0/README.md ; https://pi.dev/packages/pi-cockpit . Installed Pi references: `docs/extensions.md`, `docs/tui.md`, `examples/extensions/widget-placement.ts`, and `examples/extensions/custom-footer.ts` under `/Users/oisin/.local/share/mise/installs/pi/1.3.0/pi/`.

## Steps

- [x] Research exact Pi/Herdr releases and both existing sidebar implementations; confirm first-release replacement scope.
- [x] Inspect current todo presentation changes, Wrangle job/inbox contracts, guardrails and canonical installation settings.
- [ ] **Unit 1 — background execution and delivery (Wrangle):** implement run/stop/environment/session event semantics with CLI/MCP callers and regression tests. Acceptance: `cargo test --test cli jobs::`, targeted job-event/watchdog/inbox tests, `npm run typecheck`, `npm test`. Own execution/service/Pi background files and lockfiles; no generated runtime data in Git.
- [ ] **Unit 2 — todo feed (oisinpi, parallel with Unit 1):** publish validated snapshots and presentation mode with `TodoPresentation` as the real consumer; preserve reminders and branch behavior. Acceptance: `moon run pi-todo:test pi-todo:lint`. Own todo source/tests; existing generation emits ignored `src/generated/*`, never hand-edit it. Reconcile concurrent todo work before writing.
- [ ] **Unit 3 — integrated sidebar (after Units 1–2):** implement `wrangle sidebar`, session-view ingestion, Herdr lifecycle, `/work`, and cooperative widget removal. Acceptance: Wrangle Rust/TS checks plus the isolated terminal lifecycle scenario below. Own UI/board/session-view/plugin files, dependencies, and tests. Include actual todo and job producers; no fixture-only feature.
- [ ] **Unit 4 — guardrails, cutover and delivery (after Unit 3):** classify the final execution schema, migrate caller instructions/config, publish matching owned versions, and provision through existing managed tasks after approval. Acceptance: `moon run pi-guardrails:test pi-guardrails:lint`, package/CLI contract tests, integrated smoke test and old-package absence check. Do not uninstall while old background tasks remain running; retain their logs and let them finish or request explicit cancellation.

## Verification

Existing verification owners: oisinpi `.moon/tasks/javascript.yml` runs Oxlint and Vitest; `packages/pi-todo/moon.yml` owns generated WASM dependencies. Wrangle `mise.toml` defines `test`, `lint`, `typecheck`, `test:shim`, and `check`. Use affected/scoped targets, no checks during this planning turn.

1. **Unit/contract proof:** snapshots reject invalid/old generations; session/branch switches and codemode updates show the correct todos without changing reminders. Test local cwd/environment, queue-to-job identity, graceful stop/descendants, regex chunk boundaries, budgets, wrapper loss, busy delivery, ack expiry and same-session resume.
2. **UI proof:** use Ratatui's `TestBackend` for 36-column, narrow and wide renders, long Unicode labels, empty/dense/error states, keyboard selection, scrolling, and logs. Inject host and clock dependencies; do not use real production jobs in tests.
3. **Isolated Herdr scenario:** create a named test server, run Pi with the selected packages, open the sidebar, enter an unsent draft, start controlled local jobs and a test child, change todo branch, resize, focus/log/stop, close/reopen the sidebar, reload/restart Pi and resume the same session. Verify one sidebar, zero task widgets, preserved draft/focus, terminal-safe output, retained logs, no duplicate completion wake, and no automatic command rerun. Test Pi regular and fullscreen modes. Stop only test-owned resources when finished.
4. **Final gates:** first query affected tasks. Run only the selected oisinpi targets and Wrangle's final `mise run check` once at the integrated revision, with a timeout based on the repository's normal run. Do not duplicate hook/child-proven checks. Capture and inspect real terminal screenshots before claiming visual quality.
5. **Cutover proof:** confirm matching Wrangle binary/Pi package versions on each intended host; the background-task package, tools, UI and appended instructions are absent; the sidebar and all replacement behaviors work. Preserve old task logs. Remove old packages only with their supported uninstall/provisioning path, not by editing node_modules or installed instruction blocks by hand.

No tests, UI prototypes, installations, source edits, or live pane rearrangements were performed during planning. Approval of this plan is the implementation gate.
