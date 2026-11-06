# U7: By Stage Files tab

## Outcome
Files is the third tab of the lead's right column, after Todos and Workers. It lists the open agent's checkout changes in the By Stage groups of design B. The list refetches only when the files revision changes. Rows are read-only.

## Cause
U6b serves the `FilesState` at `GET /api/agents/{host}/{pane}/files` and sends `revisions.files` in agent events, but nothing in the portal reads either one.

## Edits
1. **`web/src/app.rs`:** add `LeadPanel::Files` (`value()` is `"files"`).
   - The shell's `on_value_change` at `:481-482` maps `"files"` to it.
   - Like Todos and Workers, Files updates `detail`.
2. **`web/src/api.rs`:** add `fetch_files(address) -> Result<FilesState, String>`, built the same way as `fetch_screen`.
3. **`web/src/agent_view.rs`:** edit only the `AgentSession` data hooks and the lead tabs. Leave `Conversation`, `ToolRecords`, `LiveActivity` and `LiveEdge` unchanged.
   - **`ChangedParts`:** add `files`.
     - `changed_parts` sets `files` when `held.files != revisions.files`.
     - An event without revisions sets all three parts.
   - **Fetching:** a `files_trigger` signal and resource fetch through `fetch_files`, like `screen_trigger`. A session change clears the shown files, and a reconnect refetches once.
   - **Width ≥ 1400:** `lead-detail` Tabs (`:665-677`) gets a third trigger, `Files {paths}`, with a `TabContent` index of 2. `on_value_change` maps all three values.
   - **Width < 1400:** `lead-tabs` (`:600-606`) gets `Files {paths}` after Workers, with a matching `TabContent` of class `files-tab`.
   - **Keep the selection:** an explicit Files selection stays selected across a resize.
4. **`web/src/files.rs` (new), declared in `web/src/main.rs`:** a `Files { state: Option<FilesState> }` component that renders groups from `portal_types::files::by_stage`. Use the existing `Accordion`, `Item`, `ScrollArea`, `Skeleton` and `Status` components. Don't edit generated component sources.
   - **Groups:** in order Conflicts, Staged, Unstaged, Untracked. Each heading shows its count. Empty groups are hidden.
   - **Rows:** each row shows the path and its status code.
     - Paths wrap.
     - A rename shows "from <previous path>" underneath.
     - Rows have no buttons, links or handlers.
   - **States:** `Pending`/`None` shows a Skeleton. `Current` with no entries shows "Working tree clean". `Stale` shows the last list plus "Last read at <time>: <error>". `NotRepository` shows "Not a Git checkout". `TooMany` shows "Too many changes to list". `Unavailable(reason)` shows the reason.
   - **Tab count:** `Stages.paths`, or blank when no list is shown.
5. **`web/assets/console.css`:** `.files-group`, `.files-row` and the rename line, using only existing neutral tokens. No new colour values (design B).

## Paths
- **Owns:** `web/src/files.rs`, `web/src/main.rs` (module line), `web/src/app.rs`, `web/src/api.rs`, `web/src/agent_view.rs` (scope above), `web/assets/console.css`, `web/src/component_tests.rs`.
- **Reads:** `portal-types/src/files.rs`, `.impeccable/mocks/git-files-sidebar/` (design B: `desktop.png`, `compact.png`), `web/src/workers.rs` (sibling tab), `DESIGN.md`.

## Acceptance
`web/src/component_tests.rs` covers each of the six states, group order with empty groups hidden, a rename row, and the unique-path count when one path is both staged and unstaged.

In `agent_view.rs` tests:
- A files-only revision change sets only `files`.
- A missing `revisions` sets all three.

`cargo test -p web` and `cargo test -p portal-types` pass. The lead runs web-lint, then a browser check at 1600, 1400, 1200, 1000 and 800 px against design B.

## Checks
- Run `cargo fmt --all --check` and `cargo clippy -p web -- -D warnings`. Commit through the pre-commit hook, without bypassing it.
- The lead builds `web/public` and runs web-lint and the browser check. Don't run web-build.

## Base
The revision that contains U6b and U5, named in the spawn. This brief is complete; it depends on no plan file.
