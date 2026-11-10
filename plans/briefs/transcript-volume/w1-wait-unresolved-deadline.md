# W1: `wait` says when its deadline expired before the target was resolved

**Outcome:**
- `dogsled wait <id> --max <d>` prints `still running` and exits 124 only when it has resolved `<id>` to a real job or agent that has not finished.
- If the deadline expires before `<id>` is resolved, it exits 124 and says so on stderr: `dogsled wait: deadline reached before <id> was resolved`. Stdout is empty.
- An unknown `<id>` that is resolved in time still exits 2, as today.
- `--json` output carries `"resolved": false` for the new case.

**Cause.**
- `src/main.rs:110–125`: `--max` becomes a `Budget` from process start. Any `Error::Deadline` from a waiting command becomes `println!("still running")` with exit 124. That includes a deadline hit while opening `state.db` (`ledger.budget.remaining("state.db")`) or reading the publication.
- `src/runtime.rs:980` (`Command::Wait` → `wait_job`) is the wait path. `src/runtime.rs:585–620` has the same mapping for `place --wait`. Change it there too only if `place` has the same unresolved state, and say so.
- The user approved this change on 10 November 2026, after `wait unknown --max 20ms` printed `still running` for an agent that does not exist.

**Design constraints:**
- Keep the budget semantics (`--max` bounds the whole command, as a hook budget does). Only the reporting changes.
- Add a typed distinction. For example, a resolution phase carried on the deadline error, or a `Resolved` marker the wait path sets once it knows the target. Never parse message text.
- Exit codes stay 124 (deadline) and 2 (unknown or invalid).
- Existing tests that assert `still running` for a target that is resolvable and still running must keep passing: `tests/cli/jobs.rs:1235` (`--max 0ms` on a durable local job) and `tests/cli/lifecycle.rs:54` (a remote item with silent ssh).
  - If `--max 0ms` can no longer resolve a local job before the deadline, the clean design and that test conflict. Stop and report; do not weaken the test.

**Tests:**
- A rig with a slow publication read, or an expired budget, before resolution. Assert stderr carries the new message, stdout is empty, the exit is 124, and JSON has `resolved: false`.
- An unknown name resolved in time. Assert exit 2.
- The existing resolved and still-running cases are unchanged.

**Owned paths:** `src/main.rs`, `src/runtime.rs` (wait and place-wait reporting only), the error or budget type that carries the distinction (`src/error.rs` or `src/budget.rs`), and wait tests in `tests/cli/jobs.rs`, `tests/cli/lifecycle.rs`, `tests/cli/review.rs` (wait cases only). Read everything else.

**Acceptance:** `cargo test --test cli wait` and `cargo test --test cli review` pass, including the new tests, with a non-zero test count.

**Checks (only these):** the filters above, `cargo test --bin dogsled budget`, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist; ask the lead if it does not. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `fix(wait): report a deadline reached before the target was resolved`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** given by the lead at dispatch. It is after T1 lands.

**Reply:** branch, head SHA, changed paths, each command with its result, the exact new stderr and JSON shapes, whether `place --wait` changed, and anything unverified.
