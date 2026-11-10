# U4: refused-job release retries follow one backoff schedule

**Outcome:** a refused job whose ordinary release keeps failing is retried on the shared `crate::backoff` schedule: 10 s doubling to a 5 min cap, with jitter. Serve logs that failure only when it starts, when its error changes, and when it recovers. It never logs once per pass.

**Cause.**
- `src/launch.rs` `release_owned` (lines ~609–640) loops over every admission whose refused job has no result. It calls `release_refused_job` for each one, with no retry deadline. Every failure is logged with `eprintln!("dogsled job refusal {}: {error}", launch.id)` at line ~620.
- Three paths call it:
  - the Admission lane (`src/work.rs:3410`, `tick`);
  - the Retention lane (`src/service.rs:174–177`);
  - an explicit command (`src/commands.rs:61`).
- The result, measured on the lead's Mac: two jobs (`r-1793906906222-97975-2`, `r-1793906971060-97975-6`) logged `ssh netcup: remote command failed` about every 3 s for days. That is 21,408 lines in the last 5 MB of a 36 MB log.

**Constraint.** Do not add columns or tables to `state.db`. Any `store::VERSION` bump makes an older store start again empty (`src/store.rs`, test `an_older_store_starts_again_empty`). Keep the schedule in serve memory. A restart may retry once immediately.

**Edits**
1. `src/launch.rs`:
   - Add a `ReleaseRetries` owner: a `Mutex<BTreeMap<String, crate::backoff::Backoff>>` plus the last logged error text for each launch ID.
   - `release_owned` takes `retries: &ReleaseRetries` and `now: u64`. It skips a launch whose `Backoff` is not `ready(now)`.
   - On failure, call `fail(now, &launch.id)`. Log only if no failure was recorded before, or if the error text changed.
   - On success, remove the entry. If a failure had been logged, log one recovery line.
   - Drop entries whose admission no longer qualifies, so the map stays bounded.
   - Keep every certificate, intent, and acknowledgement path byte-for-byte unchanged (lines ~700–812). Only the scheduling changes.
2. `src/service.rs`: create one `Arc<ReleaseRetries>` in `run`, and pass it through `Workers::spawn` → `actions` to both the Admission lane (`work::tick`) and the Retention lane. One schedule then covers both lanes.
3. `src/work.rs` `tick` (line ~3409): accept and forward the shared handle.
4. `src/commands.rs:61`: pass a fresh `ReleaseRetries`, because an explicit user command retries now.
5. Tests (in `src/launch.rs` tests, or the existing release tests that use a fake fleet):
   - A failing release is attempted once, skipped until the backoff deadline, then attempted again.
   - The same error is logged once across several passes. A changed error logs again.
   - Success clears the entry and logs recovery once.
   - The Admission and Retention callers share one schedule.

**Owned paths:** `src/launch.rs`, `src/work.rs` (the `tick` signature and call only), `src/service.rs` (lane plumbing only), `src/commands.rs` (that call only), and any test file that calls `release_owned` or `work::tick` (signature updates only). Read everything else. Do not touch `src/observation/`.

**Acceptance:** `cargo test --bin dogsled launch` passes, including the new tests. `cargo test --bin dogsled work` also passes.

**Checks (only these):** the two filters above, `cargo test --test cli retention` if such tests exist (report the count), `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist. If it is missing, ask the lead and do not build it. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `fix(launch): back off refused-job release retries and log changes only`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`.

**Reply:** the branch, head SHA, changed paths, each command with its result, and anything left unverified. If the release paths need a durable schedule after all, stop and report that. Do not change the schema.
