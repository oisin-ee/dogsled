# T1b: correct the transfer manifest, Git isolation, and ref cleanup

Follow-up to T1 on the same branch. This brief is complete without the plan.

## Outcome
A transfer manifest always records the source commit. Transfer Git calls use the isolation that `src/preservation.rs` owns. A failed capture or import leaves no transfer ref that it created.

## Cause, at `e30c58f51e010c4796c4c51d66e57abbc0b09ebd`
1. `src/transfer.rs:338-340` sets `Manifest.commit` to the snapshot commit when `include_worktree` is set. Line 134 requires both commits to be equal, and the test at line 801 asserts it. The source HEAD is then absent from the manifest.
2. `src/transfer.rs:152` (`git`) copies `preservation::git_words` (`src/preservation.rs:635`) but omits its removal of inherited `GIT_CONFIG_*` variables. An inherited `GIT_CONFIG_COUNT` can then change a transfer Git command.
3. `capture` creates the ref at `src/transfer.rs:354`, before the bundle. A later failure returns with the ref in place and no manifest. `import` verifies at the end, after it created the ref.

## Edits
- `src/transfer.rs`: `Manifest.commit` is always the resolved source commit. `Snapshot.commit` is the snapshot commit. Add `Manifest::pinned(&self) -> &str`: the snapshot commit, else `commit`.
- `src/transfer.rs`: use `pinned()` where the ref target is meant: `existing`, `pin`, the `for-each-ref --contains` call, and the ref check in `verify_objects`. In `validate`, replace the equality rule with a hex check of `snapshot.commit`.
- `src/transfer.rs` `verify_objects`: with a snapshot, also require that `<pinned>^1` equals `commit`. Read the two trees from `pinned()`.
- `src/preservation.rs`: make `git_words` `pub(crate)`. Change nothing else there.
- `src/transfer.rs` `git`: build the words with `crate::preservation::git_words(checkout, None, args)`. Map its error to `Refusal::UnsupportedSource`. Delete the copied variable list.
- `src/transfer.rs` `capture` and `import`: when a step fails after this call created the ref, delete it with `update-ref -d <ref> <pinned oid>`, then return the original refusal. Never delete a ref that existed before the call.

## Tests in `src/transfer.rs`
1. With `include_worktree`: `manifest.commit` equals HEAD of A, `snapshot.commit` differs from it, the ref resolves to `snapshot.commit` in A and in B, and `<snapshot.commit>^1` is HEAD. Replace the assertion at line 801.
2. Committed only: `manifest.commit` equals HEAD, and the ref resolves to it in A and in B.
3. A manifest whose `commit` is changed to another existing commit refuses `VerificationFailed` on import, and B has no transfer ref afterwards.
4. With the `Recording` shell (line 713): the words of one transfer Git call equal `preservation::git_words(checkout, None, args)`. Do not set process environment variables in a test.
5. A shell that fails `bundle create` for an unpushed commit: `capture` refuses, `for-each-ref refs/dogsled/transfer/` in A is empty, and `transfers/` holds no directory for it.

Keep every existing test. Change an existing assertion only where item 1 requires it.

## Paths
Owned: `src/transfer.rs`, `src/preservation.rs`. Generated: `web/public/` through `moon run dogsled:web-build`, never edited, staged, or committed.
If the change needs another file, stop and reply `blocked` with the evidence.

## Acceptance
These pass with nonzero tests: `cargo test --locked --bin dogsled transfer::` and `cargo test --locked --bin dogsled preservation::`. Report the tested SHA.

## Checks and authority
First run `git rev-parse HEAD`. If it is not `e30c58f51e010c4796c4c51d66e57abbc0b09ebd`, reply `blocked`.
Allowed: `moon run dogsled:web-build` when `web/public/index.html` is missing, `cargo fmt --all --check`, `cargo test --locked --bin dogsled --no-run`, and the acceptance commands.
Run each long command as a direct `bash` call with `timeout: 600`, never inside `codemode`. A call that ends at the limit can run again, up to three times. Builds are incremental.
If one command still cannot finish, start that one command with `mcp__dogsled__run` on your own host, in your worktree. Read it when its completion notice arrives.
The commit hook stays enabled. If it does not finish inside the limit, reply `blocked` with its last lines. Never bypass it.
Commit as `fix(transfer): record the source commit and reuse Git isolation`. Push `feat/transfer-capture` to origin with a normal push. No force, PR, merge, release, or install.
Reply: full head SHA, changed paths, commands and results, blockers, and unverified scope.

## Base
Branch `feat/transfer-capture` at `e30c58f51e010c4796c4c51d66e57abbc0b09ebd`.
