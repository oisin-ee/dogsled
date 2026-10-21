# Unit: dogsled right-side Herdr board

Deliverable: a draft PR on `feat/side-board` (base: origin/main after the quota PR, which adds `src/quota.rs`, `dogsled quota --json --cached`, and the `quota.json` snapshot).
The PR must deliver all of these:
1. A right-side Herdr board showing quotas, agents, jobs, and todos.
2. A todo bridge that publishes pi-todo's own rendered lines.
3. The dogsled Pi extension, which shows work in the Herdr split when Herdr is available, and keeps today's widget only when it is not.

Read first:
- `README.md` (board, status, service, Herdr plugin sections).
- `herdr-plugin.toml` and `plugin/board.sh`.
- `src/board.rs`, `src/quota.rs` (`cached()`, `Snapshot::render`), and `src/service.rs`.
- `pi/index.ts` (widget lines 140–250), `pi/board.ts`, `pi/work-todo.ts` (TodoSnapshot, `createView`, `render`), and `pi/work-panel.ts`.
- The Herdr skill at `~/.agents/skills/herdr/SKILL.md`, and `herdr plugin pane open --help`.

Plan of record (authoritative):
- The Herdr pane `board` changes to `placement = "split"`, direction right, and runs a compact `dogsled board --side`.
- Sections: quotas, from `quota::cached()`. Show its age, and show it as stale when it is older than 15 minutes.
- Then agents and jobs: today's board rows, compacted to the pane width.
- Then todos.
- Refresh: work state every 10 s. Quota is read from the cache only; the service lane refreshes it.
- **Todo bridge (same view, no rewrite):**
  - On each new TodoSnapshot revision, the dogsled Pi extension calls `display.createView(host)` and then `render(width)` at the side-panel width. The width comes from a value the board publishes, or a fixed default of 48 columns.
  - It publishes those lines with `{session, branch, revision, current, pane, width, lines}` to the dogsled service. Use the existing ledger/host transport, and pick the smallest native mechanism already in the codebase, for example a per-pane file in the ledger state dir that the service reads.
  - The board prints those lines unchanged. pi-todo stays the only renderer. Never re-render, parse, or copy task state.
  - The board shows one `current` line per agent, plus the full rendered view for the owner's session (`--owner <pane>`, defaulting to the Herdr pane that opened the board).
- **Herdr owns display when it is available:**
  - The Pi extension detects Herdr: `HERDR_ENV=1` and a reachable `HERDR_SOCKET_PATH`.
  - When Herdr is present, it opens the dogsled right split for that tab, or reuses it if open, and sets no above-editor widget.
  - Without Herdr, it keeps today's widget exactly.
  - The `/dogsled` control panel keeps working in both cases.
- No changes to pi-todo, Pi, or Herdr.

Owned paths:
- `src/board.rs`, `src/cli.rs`, `src/main.rs`, `src/service.rs` (only for the todo-lines intake)
- `herdr-plugin.toml`, `plugin/**`
- `pi/index.ts`, `pi/board.ts`, `pi/work-todo.ts`, `pi/*.test.ts` for these files
- `tests/plugins.rs`, `README.md`, `CHANGELOG.md`

Acceptance:
- Todo lines in the board are byte-identical to pi-todo's own `render(width)` output at the same width (a TS test with a real pi-todo fixture, see `pi/todo-fixture.test.ts`).
- The Herdr-present path sets no widget and requests a right split. The Herdr-absent path sets today's widget (tests with a fake `ctx.ui` and fake exec).
- `dogsled board --side` renders the quota rows with age and stale marking, from a fixture snapshot (Rust test).
- `tests/plugins.rs` accepts the split manifest.
- Light checks: `cargo test board`, `cargo test plugins`, and `npm test` limited to the pi files you changed, if the runner allows it. The lead runs `mise run check` once after integration.

Rules for this unit:
- Work only in your worktree. Stay inside the owned paths. Read the repository's README and conventions before you edit.
- Light checks only: the formatter or linter on the files you changed, the type check of the changed package, and the unit tests that cover your change. Do not run full suites, e2e, dependency installs, or CI watching.
- Commit hooks run on commit. If a hook fails, fix findings inside your scope; otherwise report blocked with the hook output. Never bypass hooks. A hook that runs the full suite counts as the hook; do not also run the suite yourself.
- Commit with `feat(board): …`, push the branch, and open a DRAFT PR. Do not merge it.
- Never read, print, or commit secrets. Do not fork, patch, or vendor third-party code.
- Final reply: branch, head SHA, PR URL, files changed, the light checks you ran with results, and anything blocked or unverified. Do not spawn agents.
