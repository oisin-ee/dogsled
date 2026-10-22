# Unit refresh: one ssh per host

**Branch.** Your own branch `fix/unit-refresh-batch`, based on `bfe37cd` (= `origin/main` 4e2be38 plus briefs only). Another writer is adding `refresh(fleet, Scope)` and a `settlement_ack` skip on `fix/unit-settlements-scope` in parallel. Do not add those; keep today's `refresh(fleet)` signature and filter. The lead merges the two branches. Keep your change to the loop body so the merge stays small.

**Outcome.** Within one `refresh` call, all remote units still in scope on the same host are read with one ssh call. A refresh makes at most 1 ssh per host. Baseline on a ledger copy with 0.22.0: 31 ssh per `settlements` call, 31-40 s wall, 2-5 netcup resets (`kex_exchange_identification: Connection reset by peer`).

**Cause.** `refresh` (`src/unit_request.rs`, the loop at ~437-512 on `main`) runs `fleet.remote(host, ["unit-receipt", ...])` once per remote unit, one after another. Scoping reduces the count, but the service's all-owner refresh (`sweep::children`, `inbox::tick`) still fans out one ssh per open remote unit.

**Edits.**
1. `src/launch.rs`: add `pub fn unit_receipts(ledger, &[ExecutionRef]) -> Vec<ReceiptEntry>`. It calls `unit_receipt` for each execution; one bad entry never fails its neighbours. `ReceiptEntry { request_id, evidence: Option<HostEvidence>, invalid: Option<String>, failed: Option<String> }`, serde camelCase, `deny_unknown_fields`.
2. `src/cli.rs` (`HostCommand::UnitReceipts { input }`) and `src/main.rs`: wire it through `child()` like `UnitReceipt`. Keep `unit-receipt`.
3. `src/unit_request.rs`, `refresh`: collect in-scope remote units, group them by host, and make one `unit-receipts` call per host. Local units keep the current direct `launch::unit_receipt` path. Each entry then goes through exactly today's import, rejection and logging rules (`Ok(None)`, `Invalid`, import errors). A whole-host `Ssh | Parse | Deadline` error logs one line and skips that host's units; other hosts continue. The ledger is untouched for that host.
4. Do **not** import a saved `terminal_receipt` in place of asking the host. `invalid_host_evidence_never_falls_back_to_cached_return_projection` (`tests/cli/unit_seeded.rs:396`) must keep passing unedited. Run it first.
5. No fallback to per-unit `unit-receipt`. An old peer answers `unit-receipts` with an error; that is an `Ssh`/`Parse` error for that host, logged, ledger untouched. Put the deploy order in the PR body: upgrade netcup and momokaya-2 before the Mac.

**Owned paths.** `src/unit_request.rs`, `src/launch.rs`, `src/cli.rs`, `src/main.rs`, `tests/cli/unit_seeded.rs`. Do not edit `CHANGELOG.md`; put a Fixed line in the PR body.

**Tests (add, using the `ssh` script and `ssh.log` pattern at `tests/cli/unit_seeded.rs:1025`).**
- Two open units on one remote host: one ssh invocation per refresh.
- Units on two remote hosts: one ssh per host. When host A's ssh fails, host B's units still import, and A's admissions are unchanged.
- `unit_receipts` returns one entry per execution, and one invalid execution does not fail the others.
- An old peer (ssh stub prints clap's `unrecognized subcommand` and exits 2): refresh logs it and leaves the ledger unchanged.

**Acceptance.** `cargo test --locked --test cli unit_seeded` and `cargo test --locked --bin dogsled launch` pass at your head. The new tests fail on `bfe37cd` and pass on yours.

**Pace.** Read only the files named here. Edit within your first few turns. If anything blocks you for 10 minutes, reply `blocked: <reason>`.

**Checks.** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`. The pre-commit hook runs `mise run check:rust`; do not bypass it. GitHub Actions cannot start: put the local gate command and its result at the head into the PR body.

**Publish.** Commit `fix(unit): read every remote unit on a host with one ssh`, push `fix/unit-refresh-batch`, open a draft PR. No merge, release or install. The pane-closed settlement is not in this unit. Reply with the PR URL, head SHA, test names and check results.
