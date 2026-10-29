# Unified work strip and panel

Status: all five approved steps complete. Wrangle 0.12.1 and collection 0.5.0 are published; the managed local cutover and fresh-session acceptance passed. Existing jobs and their logs were preserved.

Execution evidence:

- Release: Wrangle `313f021` / v0.12.1, workflow `39147642730` succeeded. The initial v0.12.0 workflow was cancelled before publication to correct the Cargo lockfile; its tag was not rewritten.
- Collection `00f01cb` / 0.5.0: publication `39148119324` and CI `39148119389` succeeded. All fixed-group packages use 0.5.0; the collection bundles released pi-wrangle 0.12.1.
- Dotfiles commits `c6f3eb3` and `03f221a` are pushed. Wrangle binary, Pi bundle, Claude/Codex marketplaces, guardrails, and normal lockfiles align. Unrelated live `retry.enabled=false` remains uncommitted and untouched.
- Supported package lifecycle removed pi-background-tasks and its APPEND_SYSTEM block. The old tool reported no running tasks; persisted running records had no live processes. Logs and state were retained.
- Managed dry-run and `mise bootstrap --yes --only dotfiles,tools,task` succeeded. Scoped parts avoid unrelated services, system configuration, and repository synchronization. No services were restarted and no fleet rollout is claimed.
- Fresh installed Pi session verified `todo`, `/work`, and `/wrangle`; `bg_task` and `bg_status` were absent. The installed `/work` overlay opened and closed. Installed collection is 0.5.0 and Wrangle binary/package are 0.12.1. Test processes exited.

- Approved-plan commit: `fe7bfde`; todo publisher commit: `74a563f`. Todo tests: 43 passed; scoped lint passed.
- Wrangle contract committed as `54af5d4` after 24 tests and typecheck passed, including real pi-todo integration in both load orders.
- Independent caller-guidance worker commit `9e1e5e61674ce2c134f47f8ede96040fdf64fcb8` integrated without conflict as `1f4a622`. Worker reported 42 tests plus build/lint/format passing; unchanged proof was not rerun. Wrangle's skill update is committed as `8920ea4`; step 4 marked complete. User subsequently approved all remaining commits, including the requested LEFTHOOK=0 mechanism. The user's subsequent instruction authorizes the releases and managed cutover needed to finish; do not ask again. Preserve existing jobs and use supported lifecycle operations.
- User authorized `LEFTHOOK=0` for the Step 2 Pi-only commit after scoped checks. Wrangle contract committed as `54af5d4`; step 2 marked complete. This approval covered that commit only.
- Step 3 committed as `f3b71e1` and marked complete. Latest scoped checks: 36 tests passed (including both real provider load orders), typecheck passed. Independent review found late actions after shutdown and host-label/ID deduplication errors; both now have fixes and regression tests. Dialogs use the interaction AbortSignal, details close on abort, and history maps board labels through existing `machine list` IDs.
- Isolated regular/fullscreen Pi walkthroughs at 100 and 55 columns opened the panel, switched tabs, inspected todo details and returned with Escape. These walkthroughs seeded real branch entries; they did not execute the todo tool. Earlier wrapper-based todo-tool smoke was invalid because source ownership correctly rejected the wrapper; it is not acceptance evidence.
- Test-owned Wrangle jobs `r-1792339173225-73314-62` and `r-1792339180065-73314-70` ended with exits 0 and 7, and retained bounded reads worked after automatic closure. Two queued test tickets were cancelled. No existing jobs were stopped. Final walkthrough extended this evidence:
  - Actual todo tool ran through a deterministic test provider in real Pi regular/fullscreen sessions. Init/done, branch navigation, session resume, Alt+T and unchanged editor draft assertions passed. Resume correctly restored the persisted latest leaf (3 completed), while tree navigation showed the earlier branch (0 completed).
  - Direct Herdr Pi owner `w83:p2W` inspected retained success/failure job output, expanded history, confirmed queued cancellation, and confirmed stop of test job `r-1792340683937-51529-0` (exit 137, reason stopped).
  - Real owner check-in and agent reply reached Pi and were acknowledged. Agent details opened for test recipient `w83:p31`; panel message `ok` reached its inbox through `send`.
  - Remote smoke job `r-1792340697199-78996-0` was explicitly placed on netcup and stopped through Wrangle during cleanup. Nested Expect-in-Herdr was unsuitable for agent detection; final job acceptance used direct Pi in a test-owned pane instead.
  - User approved `LEFTHOOK=0` for the Step 3 commit only, after final scoped acceptance.
- Source inspection found `wrangle logs` drains an entire file (`src/runtime.rs:529`), so it cannot satisfy bounded interactive inspection. The panel uses existing `wrangle read <job> --host <host>` for its bounded tail (`src/watchdog.rs:307`) and shows `wrangle logs <id>` for full output. No execution-core change or new log runner.
- The initiative memory capture failed with server HTTP 500 because its LLM provider is unavailable. The approved plan remains the task record.

## Context

The user selected option 1: one compact summary strip above Pi's editor, with an on-demand work panel for todos, Wrangle agents, and background jobs. Replace pi-background-tasks with Wrangle-owned execution rather than adding another process manager.

Research baseline: installed Pi 1.3.0, Wrangle 0.11.0, Herdr 0.9.5, and pi-background-tasks 2.2.0. Pi supports widgets and custom interactive panels without modifying its renderer. Wrangle already runs jobs and owns their records, logs, cancellation, placement, and completion check-ins.

Current source has changed since the initial research: pi-todo now has an active-first `TodoWidget`, expanded rendering, and a TUI-mode guard. Preserve that standalone behavior; do not implement against the earlier full-list widget.

## Approach

- Extend the existing first-party `pi-wrangle` integration to own the shared display. Do not add a separate UI package, process manager, or dependency between pi-todo and Wrangle. It already owns the board refresh, inbox, commands, and panel infrastructure.
- Render a single terminal row above the editor. Summarize todo progress, agents, jobs, and the most important attention item. Keep Pi's editor and footer unchanged.
- An explicit `/work` command opens a bounded panel with All, Todos, Agents, and Jobs views. Escape returns focus to the editor. No automatic opening or focus theft.
- Label the two real scopes honestly: todos follow the current Pi session branch; agents/jobs follow Wrangle's calling Herdr owner pane. Do not imply that starting a new Pi session in the same pane transfers or resets Wrangle jobs. Fleet administration stays in `/wrangle`.
- Keep authoritative state with its existing owner: pi-todo owns branch-sensitive tasks and reminders; Wrangle owns processes and agent/job lifecycle. Presentation must not add a second task database.
- Add cooperative display ownership to the first-party todo and Wrangle integrations. Their standalone widgets remain available when the shared UI is absent. Do not overwrite another extension's widget on a timer.
- Preserve explicit host selection for jobs dependent on the local working tree, environment, or ports. Never silently place those jobs remotely.
- Do not modify Pi, Herdr, installed dependencies, or third-party package source. First-party Wrangle changes belong in its source repository, followed by a normal released dependency update.

## Settled scope

- Replace pi-background-tasks with Wrangle command primitives. Do not recreate its process manager or require feature parity. Use Wrangle's existing run, inspect/read, logs, stop, and completion delivery contracts.
- Do not add output-pattern wakeups, automatic bash interception, or a second notification/recovery layer for this feature.
- Todos are read-only in the panel: no adding, editing, reordering, or completing tasks. The existing todo tool remains the writer.
- Job/agent inspection and explicit existing Wrangle control actions remain available; no profile/configuration editing in `/work`.

## Interaction and data contract

```text
WORK  3/7 todos · 2 agents · 1 job · ! reviewer needs input  /work
> Existing editor draft stays here
```

The strip is one row, including at narrow widths: preserve the attention count, shorten labels, then omit secondary counts. Never wrap into stacked sections. Hide it when all providers are empty; show a concise unavailable/stale state instead of false zero counts on errors. When no item needs attention, use remaining width for the active todo. Completed and abandoned todos stay distinct; progress follows existing completed/total semantics.

`/work` opens All, Todos, Agents, and Jobs tabs in a bounded, keyboard-operated Pi overlay. Arrows select, Tab/Shift+Tab change views, Enter opens details, and Escape returns to the list or closes to the unchanged editor draft. Keep selection by stable row key during refresh. No new global shortcut: `/work` is always available; existing `/todos` and Alt+T open its Todos view when shared presentation is active. `/todos list` remains a plain readout. `/wrangle` retains host/profile/inbox administration.

All groups attention items first, then current todos and running/waiting work. Completed history stays collapsed by default. Details show full todo text, agent output/reply through `wrangle read`, or bounded job-log tails through `wrangle read`, with `wrangle logs <id>` shown for full output. Stop a running job or cancel a queued ticket only through an explicit action and confirmation. Agent messaging, when selected, uses `wrangle send`; do not type into agent terminals. Starting jobs remains the existing Wrangle tool/CLI, not a new panel launcher. No todo mutations, profile editing, or automatic close/prune actions in `/work`.

### In-process handoff

Use a small versioned `pi.events` protocol, documented with both adapters and exercised by their integration tests. No shared singleton imports, filesystem transport, network service, or generic provider registry.

- pi-todo publishes validated task snapshots after mutations and branch/session changes, and replies to a current-snapshot request. Include session/branch identity and publisher instance/revision so replaced contexts cannot overwrite the current display. Only the extension that actually owns `todo` participates.
- Wrangle announces shared-display availability only after its TUI presenter is ready. A request/response plus explicit claim acknowledgement makes both extension load orders work. pi-todo hides its own widget only after this handshake; it does not give up its state or tool.
- Release on shutdown/reload and when presentation cannot initialize. pi-todo then restores its normal widget. Missing/incompatible providers leave independent functionality usable and show an explicit integration error rather than disappearing.
- The `/todos` shortcut handoff is an event carrying the requested view, not an event that mutates tasks. Never emit or persist a todo-state entry for opening, closing, or selecting panel rows.

### Existing Wrangle reads

Keep the single board refresh and existing inbox reader. Render the strip from their current owner-scoped data. Pi-todo updates arrive immediately through events; job/agent refresh keeps the existing ten-second cadence, with explicit refresh after user actions.

The current board excludes closed items (`src/board.rs:275`). For the Jobs view's recent history, read `wrangle items --owner <caller> --json` and `wrangle ps --all --json` through the same in-flight refresh coordinator while the panel needs history. Join owner-ledger items to job records by recorded host and job ID; do not show all fleet jobs or trust a pane ID alone across hosts. Keep the panel cache transient. Logs remain in Wrangle's existing storage and retention policy. A missing/expired record is shown as unavailable, not invented history. Never run `logs --follow` or `wait` in a render/input callback.

Outside Herdr, keep todo-only presentation possible and report Wrangle unavailable; do not introduce a local execution fallback. Non-TUI modes retain the original tools and reminders with no custom UI.

## Files to modify

Use the existing two package boundaries: pi-todo publishes its validated branch snapshot; pi-wrangle consumes it and owns the work UI. No Rust state schema or WASM interface changes are needed.

- `packages/pi-todo/src/index.ts`, `presentation.ts`, `lifecycle.ts`, a small `work.ts` event adapter, and adjacent tests: publish branch state and cooperate with the shared display without changing reminder semantics.
- `packages/pi-todo/src/widget.ts`: retain its standalone rendering unchanged; route its existing Alt+T and `/todos` entrypoints to the shared Todos view when presentation is claimed.
- `/Users/oisin/dev/wrangle/pi/work.ts` and focused adjacent presentation modules/tests: work strip, panel, todo event adapter, and user-facing actions. Keep the existing administration panel separate.
- `packages/oisinpi/package.json`, `pnpm-lock.yaml`, package documentation and release metadata: bundle the selected integration and consume released Wrangle changes.
- `/Users/oisin/dev/wrangle/pi/index.ts`, `board.ts`, new work presentation/adapter files, and their tests: reuse board loading and owner scope; expose cooperative presentation and actions. Retain `panel.ts` administration behavior; adjust only display coordination if required.
- Wrangle `README.md` and release metadata; oisinpi `packages/pi-todo/README.md`, `packages/oisinpi/README.md` and `.changeset/*.md`: document the shared UI and normal standalone behavior.
- Wrangle execution/service source stays unchanged unless inspection proves an existing primitive cannot support the approved UI. Such a blocker is reported, not solved by adding a parallel job implementation.
- `/Users/oisin/dev/dotfiles/conf.d/harness/settings/pi/settings.json`: remove the background-task package declaration and its settings; update the released collection pin.
- `/Users/oisin/dev/dotfiles/conf.d/harness/mise.toml` and owning lockfile: update the Wrangle version to match the released Pi extension; use its existing `agent:pi-packages` reconciliation, not a new installer.
- `packages/pi-guardrails/src/pi/tool-timeouts.ts`, `src/policies/shell.ts`, and adjacent tests: remove instructions to use `bg_task`; retain wait/timeout protections and point long Pi jobs to Wrangle primitives.
- `/Users/oisin/dev/wrangle/plugins/wrangle/skills/wrangle/SKILL.md`: replace guidance to run waits through a background-task tool with non-blocking `wrangle run` and owner check-ins.
- Installed `~/.pi/agent/APPEND_SYSTEM.md` is generated package state. Use the package's provided removal lifecycle and verify removal of its marked block; do not hand-edit dependency files or silently stop existing jobs.

## Reuse

- `packages/pi-todo/src/state.ts`: `latestSnapshot()` and the branch-backed `todo-state` entry.
- `packages/pi-todo/src/lifecycle.ts`: `ownsTodo()`, `replay()`, and completion-reminder handling.
- `packages/pi-todo/src/presentation.ts`: `TodoPresentation` owns transient expansion and is the handoff point for standalone versus shared display.
- `packages/pi-todo/src/wasm.ts`: `parse()` and exported `TodoList`/`Status` types; no parsing rendered widget strings.
- `packages/pi-todo/src/widget.ts`: existing active-first terminal presentation.
- `/Users/oisin/dev/wrangle/pi/index.ts`: board refresh and the existing inbox reader; do not create duplicate polling or notification delivery.
- `/Users/oisin/dev/wrangle/pi/board.ts`: board types, state groups, terminal-text sanitization and width utilities.
- `/Users/oisin/dev/wrangle/pi/panel.ts`: `SelectList`, keyboard navigation, and action execution outside the panel interaction.
- Pi public APIs: `pi.events`, `ctx.ui.setWidget()`, `ctx.ui.custom()`, theme helpers, and terminal-width utilities.

## Steps

- [x] Set the replacement contract and presentation scope: existing Wrangle primitives; no todo editing; shared display lives in pi-wrangle.
- [x] **Unit 1 — working todo/work display contract.** Implement the pi-todo publisher/handoff and its real Wrangle strip consumer together as one contract unit. Own todo presentation/adapter paths and Wrangle `pi/index.ts`, `pi/board.ts`, `pi/work*.ts`, contract tests and docs. Prove both load orders, branch changes, single display, and standalone fallback. Acceptance: `moon run pi-todo:test pi-todo:lint` in oisinpi and `npm test && npm run typecheck` in Wrangle. Generated outputs: existing gitignored todo WASM bindings only; no new generated artifacts.
- [x] **Unit 2 — work panel.** After the contract lands, add the bounded tabbed overlay, read-only todo details, Wrangle read/logs/send/stop/cancel actions, recent job history, and failure states. Own Wrangle `pi/work*.ts`, integration tests and README. Acceptance: `npm test && npm run typecheck`, plus the isolated interactive walkthrough below. No generated outputs or execution-core changes.
- [x] **Unit 3 — caller guidance.** Independently update pi-guardrails messages and tests without weakening timeout/wait rules. Own the named guardrail policy files, adjacent tests, and its changeset. Acceptance: `moon run pi-guardrails:test pi-guardrails:lint pi-guardrails:build`. Generated output: existing gitignored `packages/pi-guardrails/dist/`. Delegate this independent unit while the lead handles the display contract; do not split writers across the same contract/files. Update Wrangle's job skill alongside its owning UI release.
- [x] **Unit 4 — released integration and cutover.** After approved releases exist, update the oisinpi dependency pin/lockfile and collection changeset; update dotfiles collection/guardrails/Wrangle pins together. Remove the background-task declaration/configuration with the supported removal path and verify instruction cleanup. Acceptance: affected oisinpi checks, managed dotfiles dry-run, then authorized `mise bootstrap --yes` and a fresh-session smoke check. Generated outputs: normal lockfiles, package contents, and managed settings only. No copied or patched dependencies.

## Verification

- Todo integration: current branch, resume/tree changes, nested tool calls, single active task, and unchanged completion reminders.
- Display ownership: startup/load order, reload, shutdown, missing providers, and standalone fallback; never show the old widgets alongside the shared strip.
- Rendering: exactly one summary row at narrow/wide widths; long labels, wide characters, empty/busy/error states, theme changes, and large lists.
- Panel: keyboard navigation, selected-row stability during updates, inspection/actions, Escape and focus return, and preservation of the editor draft. Test regular and fullscreen Pi modes.
- Jobs: existing Wrangle behavior for explicit local execution, remote placement when requested, logs after exit, failure/cancellation, unavailable hosts, and owner completion check-ins. Do not assert pi-background-tasks-specific guarantees.
- Migration: no lost running jobs or logs; no stale tools or instructions directing the agent to the removed package.
- Discover affected targets before running checks (`moon query tasks --affected`) and inspect dependency selection. Run fixers once when each unit is complete; run only relevant light checks and the final affected gate. Do not rerun proof from an unchanged worker revision. No tests or builds run during planning.

### Interactive acceptance

Load the two source extensions into an isolated Pi session using the installed Pi's `-e` mechanism and a temporary agent directory, without the installed duplicates. Keep all smoke jobs test-owned. Exercise regular/fullscreen modes and narrow/wide panes. Create a todo list through the actual tool, open `/work`, switch branches, restore a session, and confirm the list stays correct without any panel mutation. Open agent details and run one local success job, one failure, and one explicitly cancelled long job with Wrangle. Check output after exit and owner completion delivery. Stop only smoke-test jobs and close test UI resources at the end.

### Cutover gates

Before unloading the old extension, inspect its tracked jobs with its own tool. Do not silently kill, adopt, or restart processes. Allow finite jobs to finish; ask for an explicit stop/restart decision if any long-lived job prevents removal. Preserve logs the user needs before any supported uninstall cleanup.

`agent:pi-packages` reconciles declared installs but does not itself prove that a removed package's generated instructions disappeared. Use `pi remove` and the installed package's provided `append-system.mjs remove` lifecycle when necessary, without modifying that script. Verify the marked `pi-background-tasks` block is absent from `APPEND_SYSTEM.md`, the package is no longer declared/loaded, and `bg_task`/`bg_status` disappear in a fresh Pi session. If the supported path fails, report the blocker rather than hand-editing installed package state.

Keep Wrangle binary and Pi package versions aligned. Publishing/installing remains gated by the relevant approvals; this plan does not authorize changing services or terminating existing work. Existing instructions in this conversation cannot be retroactively removed, so cutover proof requires a new session.

## Coordination

`PLAN-work-sidebar.md` is an existing untracked plan from other work. It proposes a different surface. Leave it untouched; this plan implements the explicit option 1 selection in this conversation. Coordinate before concurrent changes to the shared todo/Wrangle files; do not combine sidebar work into this unit.

## Evidence

- Pi installed docs: `/Users/oisin/.local/share/mise/installs/pi/1.3.0/pi/docs/extensions.md` and `docs/tui.md`.
- Wrangle source: `/Users/oisin/dev/wrangle/pi/` and https://github.com/oisin-ee/wrangle/blob/v0.11.0/README.md.
- Herdr plugin surfaces: https://herdr.dev/docs/plugins/.
- Existing background-task contract: installed 2.2.0 package metadata and instructions, plus https://github.com/vanillagreencom/kendex/tree/main/pi-extensions/pi-background-tasks.
