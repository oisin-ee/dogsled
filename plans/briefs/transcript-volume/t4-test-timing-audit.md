# T4: test deadlines bound only the behaviour under test

**Outcome:** the full Rust test suite passes on a loaded host. No test fails because a wall-clock deadline also had to cover process startup, compilation, store opening, parsing, or scheduler delay. Every deadline that a test exists to prove keeps its value.

**Why.** On 10 November 2026, two existing tests failed precommit hooks for unrelated changes on netcup (load average 17–28 on 16 cores). Both pass when run alone.

1. `observation::tests::details::conversation_over_main_frame_limit_loads_only_through_detail_route`. T1 fixes it with kilobyte payloads.
2. `review::deadline_includes_discovery_and_placement_ssh`, `wait-discovery` case (`tests/cli/review.rs:74`). The `--max 20ms` budget starts at process start (`src/main.rs:110–117`), so it covered startup and the state read. The lead raises only that case to 1500 ms on T1's branch.

The user asked that everything works with proper timing. This unit audits the rest.

**Rule to apply.** For each wall-clock deadline in tests, decide what it bounds:
- **Liveness wait** ("eventually happens", "process exits", "tick completes", "frame arrives"). Use the shared generous deadline. `tests/cli.rs:93` `PROCESS_DEADLINE` (120 s) is the precedent for CLI tests. Add one equivalent named constant for unit tests if none exists. A liveness wait returns as soon as its event happens, so a generous value costs nothing when the test passes.
- **Behaviour under test** (a `--max` budget, a lease, a TTL, "returns within N"). Keep the value. Make sure only the measured operation runs inside it: move setup out of the timed region, or start the clock after setup. If the product's own budget necessarily includes startup (as `--max` does), the test's value must leave room for startup. Prove the property with a margin, as the lead's 1500 ms vs 2 s elapsed check does.
- **Negative check** ("does not happen within N"). Keep it short. It must not be able to pass vacuously because the process was slow.

**Starting inventory (not exhaustive):**
- `recv_timeout(Duration::from_secs(3|4|6))` in `src/observation/details.rs:552,575` and `src/observation/details_fixture.rs:97,117,267`.
- `Fixture::wait` 3 s, and the `Duration::from_millis` and `from_secs` literals across `src/**/tests*.rs`, `src/**/*_tests.rs`, `tests/cli/*.rs`.
- The `--max 20ms|50ms` cases at `tests/cli/review.rs:77,138,172` and `tests/cli/lifecycle.rs:54`. Classify each one, and only change it if the rule requires.

Use `rg 'from_millis\(|from_secs\(|recv_timeout|--max' src tests`.

**Out of scope:** the `wait` command's behaviour when its deadline expires before the target is resolved. Unit W1 owns that, including `tests/cli/jobs.rs:1235` and `tests/cli/lifecycle.rs:54` if W1 changes their expected output. Do not change product timing constants (`LEASE`, `NATIVE_TTL_MS`, `SAMPLE`, `DETAIL_TICK`, the backoff schedule). If a failure needs a product change, stop and report it.

**Edits:** test code only, under `src/**` test modules, `src/**/*_fixture.rs`, `src/**/*_tests.rs`, and `tests/`. Every changed deadline gets a short comment saying which class it is, if the class is not obvious from a named constant.

**Acceptance.** On netcup, run the suite under synthetic load twice in a row:

```
nproc_x2=$(( $(nproc) * 2 )); for i in $(seq $nproc_x2); do (timeout 3600 sh -c 'while :; do :; done' &) ; done
moon run dogsled:test
```

Then kill the burners. Both runs must pass. Report each run's result and the count of tests changed by class.

**Checks (only these):** the loaded runs above, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist; ask the lead if it does not. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `test: bound only the behaviour under test with tight deadlines`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** given by the lead at dispatch. It is after T1 lands.

**Reply:** branch, head SHA, changed paths, a table of every deadline you changed (file:line, old, new, class), both loaded-run results, and any deadline you left unchanged on purpose with the reason.
