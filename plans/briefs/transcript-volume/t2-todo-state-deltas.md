# T2: todo state is saved as deltas, so session files stop growing as changes × tasks

**Outcome:** a task change appends an entry whose size depends only on what changed. Today each change appends the whole list. Every reader still gets exactly the list, provenance, reminders and import receipts it gets today. Sessions that hold only full snapshots still read correctly.

**Cause (measured 10 November 2026 on local-mac):**
- `pi/todo` appends the whole `TodoList` as a `todo-state` custom entry on every change:
  - `pi/todo/src/tool.ts:51`
  - `pi/todo/src/provider.ts:155`, `:184`, `:201`, `:280`
  - the README "State" section.
- Of 1,897 Pi sessions, 767 are over 1 MB. The p90 is 16.6 MB and the largest is 890 MB. In that largest session, 783 `todo-state` entries make up 789 MB (about 1 MB each; the last holds 192 tasks at about 2 MB). All tool results together are 68 MB.
- These entries also make serve's transcript reader parse megabytes per append. A separate unit fixes that.

**Readers that must stay exact:**
- `latestSnapshot` (`pi/todo/src/state.ts:21`), used by `lifecycle.ts:49`, `:86` and `work.ts:135`.
- `branchSnapshot` with `Provenance` (`state.ts:47–103`):
  - a revision's `contractEntryId` is the entry that first introduced that revision;
  - a seal's `entryId` is the first sealing entry;
  - a status-only change must never move an origin.
- `reminder` (`reminder.ts:15–30`) builds one `snapshot` fact per state entry, in order.
- `TaskProvider.#importScope` (`provider.ts:240–255`) scans every state entry for import receipts.
- The tests `pi/unit-contract.test.ts:84` and `pi/work-contract.test.ts:16` read `todo-state` directly. Update their fixtures only if the format requires it, and keep their assertions.

**Edits**
1. **Entry format** (`pi/todo/src/state.ts`):
   - Add a delta entry. Prefer a new `customType` (for example `todo-delta`) that holds the changed root tasks or subtasks by id, removed ids, and order changes.
   - Keep `todo-state` for full snapshots.
   - Add one fold, `branchList(branch)`, that replays full snapshots and deltas in branch order, and also yields the list after each entry. Every reader above uses it, so all readers share one fold.
   - The fold must not be quadratic in branch length: cost is O(entries + changed tasks).
2. **Writers:** every `session.append(STATE_ENTRY, list)` site computes the delta between the branch's folded list and the new list, and appends it. If the delta would not be smaller than the full list, write a full snapshot instead.
3. **Provenance:** resolve origins from the entry in which a task's revision or seal first appears. For a delta entry, that is the entry that carries the changed task.
4. **Mixed versions:**
   - An older pi-dogsled that reads a branch containing deltas must never treat a delta as a full list.
   - Decide what it sees: either the last full snapshot, or a refusal. State that in your reply.
   - If you cannot make an older reader safe without writing full snapshots, stop and report. The lead will take the trade-off to the user.
5. **Tests** (`pi/todo/src/*.test.ts`):
   - For a scripted sequence of at least 50 changes over at least 100 tasks, the folded list, every reminder fact and every provenance value match what the current full-snapshot code produces for the same sequence. Keep the current code inside the test as a reference.
   - Delta entry size is bounded by the changed tasks, not by the list size.
   - A branch of full snapshots only, written by today's format, folds to the same result.
   - A mixed branch (full snapshots, then deltas) folds correctly.
   - `/tree` branch moves see the list of their own branch.

**Owned paths:** `pi/todo/src/` (source and tests), `pi/todo/README.md` (the State section), and the two fixtures named above if the format requires it. Do not change `crates/todo-core` or `crates/todo-wasm`, and do not regenerate wasm. If the fold needs the core, stop and report. Read everything else.

**Acceptance:** `node --test pi/todo/src/*.test.ts` passes, and so do `pnpm run typecheck` and `node --test pi/unit-contract.test.ts pi/work-contract.test.ts`.

**Checks (only these):** the three commands above and the repository's Pi lint, if `moon run dogsled:lint` covers TypeScript (report it). Commit hooks run as configured. Do not bypass them. `web/public` must exist for the hook. If it does not, ask the lead.

**Publication:** commit with `fix(todo): save task changes as deltas instead of full snapshots`. Push your branch. Do not open a PR, merge, release or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`. Run `git rev-parse HEAD` first. If it differs, stop and report.

**Reply:** branch, head SHA, changed paths, the entry format, the mixed-version behaviour, each command with its result, the measured entry sizes for one status change in a 192-task list (before and after), and anything you did not verify.
