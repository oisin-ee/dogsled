# J1 repair 5: kill before joining in Background::drop

## Outcome
When a `Background` guard drops, it kills and reaps the owned command and its process group first. Only then does it join the output reader. A test whose release never arrives then fails within its bound instead of waiting on open streams. Tests only.

## Revisions
Repository: `/Users/oisin/dev/dogsled`. Continue branch `fix/lifecycle-job-claims-r2` **without a base argument**. The retained head is `c76711cd8e0b0750e92008fbb967fcc1f9d0b540`. Preserve all commits.

## Finding (lead verified at `c76711c`)
`impl Drop for Background` (`tests/cli.rs:206–211`) joins `reader` in its body. Rust runs a `Drop::drop` body before it drops the struct's fields, so the `owned` guard kills the command only **after** the join returns. If the command is still running, its streams stay open and the join blocks. The comment "The owned command is killed and reaped by the guard" hides this order.

## Required
1. In `Background::drop`, terminate and reap the owned command and its process group **before** joining the reader. Use the existing `Owned` kill/reap path; add a small method on `Owned` if needed. The later field drop must stay safe: no double kill of a reused PID.
2. Make the comment state the real order.
3. Add a regression: a `Background` whose command never ends is dropped and returns within the bound, with no surviving process. Prove it fails (hangs past the bound or times out) at `c76711c` and passes after.

## Owned paths
`tests/cli.rs` only. Do not edit `src/` or any other test file. The blocking read at `tests/cli/review.rs:32` is a separate later unit.

## Acceptance
```sh
cargo test --test cli jobs:: && cargo test --test cli lifecycle:: && cargo test --test cli launch_retry:: && cargo test --test cli stop_sweep:: && cargo test --test cli review:: && cargo test --bin dogsled work_tests::
```
Red before and green after for the new regression. No stray test processes afterwards. The pre-commit hook (`mise run check:rust`) passes once at the final commit; never bypass it. Report the exit codes and exact SHA.

## Limits
Local commits only; no push or PR. Start at once with the edit. Write code into the file with edit tools, one small piece per edit; never draft code in chat. If you make 20 tool calls without an edit, stop and report. Use the seeded unit and native done.
