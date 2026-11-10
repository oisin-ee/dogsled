# T1: source capture and import on one host

Plan: `plans/repository-transfer.md`, approved 2026-11-10. This brief is complete without the plan.

## Outcome
`src/transfer.rs` captures a pinned source from one checkout into a verified Git bundle under the state directory. It imports that bundle into another checkout of the same repository.

## Cause
A launch request carries no source content (`Request` at `src/work.rs:65`). The only raw capture of staged and working trees is private to close recovery (`capture` at `src/preservation.rs:1193`). It pins `refs/dogsled/recovery/<id>`, which never expires.

## Edits
- `src/preservation.rs`: add `pub(crate) struct Trees { head: Head, index_tree: String, worktree_tree: String }` and `pub(crate) fn trees(shell, checkout) -> Result<Trees>`. It runs `preflight` (`:1044`), `observe` (`:763`), `index_tree` (`:794`), `worktree_tree` (`:893`), then `observe` again. A difference refuses `CheckoutChanged`. It pins nothing. Make `commit_tree` (`:1133`) `pub(crate)`. Do not change `capture`, `snapshot`, or an existing test.
- `src/main.rs`: add `mod transfer;` with `#[cfg_attr(not(test), expect(dead_code, reason = "wired by T2"))]`, as line 37 does.
- `src/transfer.rs` (new): `capture(shell, ledger, checkout, &Options { include_worktree, base }) -> Result<Manifest>`, `import(shell, ledger, checkout, id) -> Result<Manifest>`, and `load(ledger, id) -> Result<Manifest>`.

`Manifest` is serde with `deny_unknown_fields` and `version: 1`. Fields: `id`, `repository` (`checkout::identity(origin)`, else the raw origin), `object_format`, `commit`, `snapshot` (optional: `commit`, `index_tree`, `worktree_tree`), `ref_name`, `bundle` (optional: `sha256`, `bytes`), `created_ms`.

Capture:
1. The source commit is `base` through `rev-parse --verify --end-of-options <base>^{commit}`, else HEAD. An unborn HEAD refuses. `base` with `include_worktree` is invalid.
2. With `include_worktree`, call `preservation::trees`. Build the snapshot commit as `src/preservation.rs:1236-1252` does: tree `worktree_tree`, parents HEAD and an index commit.
3. Create `refs/dogsled/transfer/<id>` at the snapshot commit, else at the source commit: `git update-ref <ref> <oid> <zero-oid>`. Never change a branch, HEAD, the index, or a file of the source.
4. Run `git bundle create <tmp> <ref> --not --remotes=origin`. If an origin ref already contains the commit, Git refuses an empty bundle. Record `bundle: null` then.
5. Write `transfers/<id>/payload.bundle` and `transfers/<id>/manifest.json` under `ledger.dir`. Write a temporary name, then `fs::rename` (`src/launch.rs:2330`), inside `ledger.with_lock`. The `id` is 32 hex characters (`random_id` at `src/preservation.rs:606`). Check each id with the rule of `jobs::validate_id` before it enters a path.

Import:
1. Refuse a different origin identity or object format.
2. With a bundle: compare the SHA-256 of the file, then run `git bundle verify`. If verify fails, run `git fetch --no-tags origin` once and verify again. A second failure refuses. Then run `git fetch <bundle> <ref>:<ref>` for that one ref.
3. Without a bundle: if the commit is absent, run `git fetch --no-tags origin` once. If it is still absent, refuse. Then create the ref as in capture step 3.
4. An existing ref with the same OID succeeds without a change. Another OID refuses.
5. Confirm that the ref resolves to the manifest commit and that both trees exist. Import never changes a branch, HEAD, the index, or a file.

Use a typed `transfer::Refusal`. Never put Git stderr or a remote URL in an error message. Rely only on `git bundle create`, `git bundle verify`, and fetch from a bundle file. Report the oldest Git version that the code needs. Known: local 2.56.0, netcup 2.54.0. momokaya-2 is not read yet.

## Tests in `src/transfer.rs`
Use real Git through `System::default()`, as the `Repo` fixture at `src/preservation.rs:2134` does. Use a bare origin with clones A and B.
1. Committed only: an unpushed commit in A imports into B. Dirty files in A do not travel. Branch, HEAD, index, and files of B do not change.
2. `include_worktree`: staged, unstaged, binary, deleted, renamed, untracked, executable, and symlink entries arrive in B with equal `ls-tree -r` output and blob bytes. An ignored file is absent. HEAD, the index tree, and `git status --porcelain=v2 -z` of A are identical before and after.
3. HEAD already on origin gives `bundle: null`. B lacks the commit, fetches once, and succeeds.
4. One changed payload byte refuses and creates no ref.
5. B has older origin refs than A: import fetches once and succeeds. With an unreachable origin, import refuses and creates no ref.
6. Another repository identity refuses. A repeated import changes nothing. The same id with another OID refuses.
7. `base` with `include_worktree` is invalid. An unmerged index refuses.

## Paths
Owned: `src/transfer.rs`, `src/preservation.rs`, `src/main.rs`. Generated: `web/public/` through `moon run dogsled:web-build`, never committed. No CLI, MCP, peer transport, retention, or checkout materialization in this unit.
If the change needs another file, stop and reply `blocked` with the evidence.

## Acceptance
These pass with nonzero tests: `cargo test --locked --bin dogsled transfer::` and `cargo test --locked --bin dogsled preservation::`. Report the tested SHA.

## Checks and authority
Allowed: `moon run dogsled:web-build` once when `web/public/index.html` is missing (`build.rs` needs it), `cargo fmt --all --check`, and the acceptance commands. `web/public/` is generated and gitignored: never edit, stage, or commit it. The commit hook stays enabled. The lead runs `moon run dogsled:check`.
Commit as `feat(transfer): capture and import a pinned source`. Push only branch `feat/transfer-capture` to origin. No PR, merge, release, or install.
Reply: branch, full head SHA, changed paths, commands and results, blockers, and unverified scope.

## Base
`origin/main` at `f443a7de94011a4c9f8c38050955a6decc9a78d6`.
