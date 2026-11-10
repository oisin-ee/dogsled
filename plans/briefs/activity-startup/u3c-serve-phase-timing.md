# U3c: serve reports where each observation pass spends its time

**Outcome:** a running `dogsled serve` writes one bounded summary line per minute to its log. The line gives, for that minute, the pass count, the wake domains that started the passes, and the count, total, and max time of each observation phase. Nothing else in serve's behaviour changes.

**Why.** U3a measured publication latencies of 3.9–13 s on the lead's Mac, at 0.19 publications/s with 9 MB frames. It could not attribute that time to functions: the v0.34.2 release has no symbols, and a rebuild did not reproduce the same `__TEXT,__text`. The leads are `holds::local` → `work::settled` doing per-owner whole-store reads, and frame serialization. Neither is measured. This unit adds the measurement. Plan: `plans/activity-startup-registration.md`.

**Edits**
1. New `src/observation/timing.rs`, declared in `src/observation.rs`.
   - Add `PassTimings` with `phase(&mut self, name: &'static str, elapsed: Duration)`, `pass(&mut self, domains: u8)`, and `flush(&mut self, now: Instant, write: impl FnMut(String))`.
   - Use only monotonic `Instant` values. Use a 60 s window, and emit at most one line per window.
   - Line format: `dogsled serve timing: passes=N wakes ledger=a local=b peers=c branches=d details=e scheduled=f phase name=count/total_ms/max_ms ...`. Use the domain bit constants in `src/observation/wake.rs:16–20`.
   - Never log paths, pane names, or request data.
2. `src/service.rs` `publish()` (lines ~302–365): time each phase separately.
   - Phases: `branches` (observe or gaps), `collect` (the `collect`/`refresh`/`publication` branch), `peers.compose`, `activity.refresh`, `details.refresh`, `activity.views`, `herdr.interests`, and `diagnostics.publish`. The last one includes `Publisher::replace` serialization. Time it as `publish.replace` by timing the `publisher.replace` call inside `Diagnostics::publish` (`src/observation/diagnostics.rs:16–25`), or by timing the whole call.
   - In `Observation::refresh` (lines ~424–455), also time `peers.retry`/`discover` and the activity sample.
   - In `run` (lines ~458+), call `pass(domains)` once per refresh. Count scheduled passes (`scheduled || baseline_due`) separately. Call `flush` after each pass.
3. `src/observation/collect.rs` `refresh` (lines ~199–289): time the sub-phases `collect.catalogue`, `collect.terminals`, `collect.snapshot` (the jobs/reservations or `tick_with_terminals` branch), `collect.host` (`host(...)`, which owns holds), `collect.status`, `collect.todos`, and `collect.publication`.
   - Pass the timings in as a `&mut PassTimings` parameter, or return them with `Collection`. Choose the smaller change.
   - Do not change what the collector computes, or its order.
4. Tests in `src/observation/timing.rs`:
   - The window emits nothing before 60 s and exactly one line after.
   - Counts, totals, and max values are aggregated correctly.
   - The wake domains are counted per bit.
   - The line contains no input strings except phase names.

**Owned paths:** `src/observation/timing.rs` (new), `src/observation.rs` (the `mod` line only), `src/service.rs`, `src/observation/collect.rs`, `src/observation/diagnostics.rs`. Read everything else. Do not change `src/observation/live_activity.rs` or `src/observation/ipc.rs`: other units own them. In `src/service.rs`, change only timing.

**Acceptance:** `cargo test --bin dogsled observation::timing` passes. `cargo test --bin dogsled service` and `cargo test --bin dogsled observation` also pass unchanged.

**Checks (only these):** the three `cargo test` filters above, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist. If it is missing, ask the lead and do not build it. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `feat(service): log per-phase observation timing once a minute`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`.

**Reply:** the branch, head SHA, changed paths, each command with its result, one example output line from a test, and anything left unverified.
