# U3b: the activity authority runs on its own thread and publishes on its own channel

**Outcome:** activity commands (`register`, `publish`) and activity reads and streams no longer wait for, or travel with, the main observation publication.

- A `register` sent while the service main loop is busy collecting gets its acknowledgement in milliseconds, not after the next `publish()` pass. Today that wait is up to 13 s, against a 5 s IPC timeout.
- An activity stream gets each new view without a new 9 MB `Frame`.

**Why (U3a, measured on the lead's Mac, v0.34.2):**
- Publications arrive 3.5–12.6 s apart, and each frame is about 9 MB.
- `Authority` handles commands only inside `service::publish` → `activity.refresh` (`src/service.rs` ~354). It publishes only through `Publisher::activity` → `Frame.activity` (`src/observation.rs` ~120, ~198–216).
- `ipc::activity_connection` reads activity from the main frame watch (`src/observation/ipc.rs` ~526–598).
- Both the command path and the publication path are coupled to the slow loop. U3a showed that both must be separated.
- Plan: `plans/activity-startup-registration.md`, U3.

**Invariants to keep (each must still be covered by a test):**
- Bounded admission: `CAPACITY` and the `Capacity` code.
- Replay, CAS, generation, and sequence fencing in `apply`.
- Session identity and `verified_evidence`, unchanged.
- The 15 s producer `LEASE`, `NATIVE_TTL_MS` freshness of binding samples, and the `SAMPLE` cadence. Sample only while there is interest.
- The U1 rule: an unverifiable `register` forces at most one `herdr agent list` sample per handling pass. Publishes and viewers never force a sample.
- Refusal logging: once per change of (pane, code), bounded by interest.
- `agent_missing` is returned only when a fresh binding sample (< `NATIVE_TTL_MS`) shows the pane absent. Otherwise return `upstream_unavailable`.
- `valid_for_ms` is reduced by retention time before each send.
- Within one serve epoch, activity `revision` increases monotonically per pane, because `web/src/activity.rs:100–110` rejects lower revisions. Activity can have its own counter, but it must use the serve epoch.
- Independent shutdown: stopping serve stops the authority thread, and in-flight commands get `upstream_unavailable`, not a hang.

**Edits**
1. `src/observation/live_activity.rs`:
   - Move `Authority` behind a spawned owner, for example `Authority::spawn(epoch, shell) -> (Commands, ActivityFeed, Handle)`. Its thread blocks on the command receiver with a deadline equal to the next sample, lease, or expiry time, and never polls.
   - The thread applies commands immediately. It uses `blocking_recv` with the timing it already owns, or a current-thread runtime. Choose one, and spawn no threads per command.
   - It accepts terminal observations from the collector through a non-blocking channel (latest value wins). It also samples `herdr agent list` itself on its existing rules.
   - It publishes to its own `tokio::sync::watch` an `ActivityFrame { views: BTreeMap<String, Arc<[u8]>>, measured: Instant, bindings_measured_ms: Option<u64>, bound: BTreeSet<String> }`. It republishes only when the content changes, using the existing `same_activity` / `activity_content` semantics moved here.
   - Remove the `dirty.mark(DETAILS)` wake from `Commands::send`.
   - The shell must be owned and `Send`. Production uses `crate::shell::System::default()`, as the lane workers do (`src/service.rs` `actions`). Tests inject `crate::shell::Fake`.
2. `src/observation.rs`: remove `activity` and `activity_measured` from `Frame`, and `activity` handling from `Publisher`, including `same_activity` in the change check (~146).
3. `src/service.rs`:
   - Spawn the authority once, in `Observation::new`, next to `server.activity(...)` (~384). Hand `ActivityFeed` to the IPC server.
   - After each collection, forward the terminals measurement to the authority without blocking.
   - Delete `activity.refresh`, `activity.views`, and `publisher.activity` from `publish()`. Delete the `sample_due` / `sample` and `next_wake` uses from `Observation::refresh` and `due`.
   - The `Exchange.activity` field becomes the authority handle, which joins on drop.
4. `src/observation/ipc.rs`: `activity_connection` reads and waits on `ActivityFeed`, not the main frame watch. It answers `agent_missing` from `bound` and `bindings_measured_ms`, not from the portal view. `activity_command` is unchanged apart from removing the wake.
5. Tests:
   - `src/observation/tests/live_activity.rs`: move the existing authority tests onto the threaded owner, keeping their assertions. Add:
     - (a) a `register` gets its acknowledgement within 200 ms while nothing calls the service loop;
     - (b) a `publish` reaches an `ActivityFeed` subscriber without any `Publisher::replace`;
     - (c) dropping the handle makes pending and new commands return `upstream_unavailable` within 1 s;
     - (d) per-pane revision is monotonic across republishes.
   - `src/observation/ipc/tests.rs` and `src/observation/tests.rs`: replace `Frame.activity` fixtures with `ActivityFeed` fixtures, keeping each assertion's intent. Do not weaken or delete an assertion. If one cannot be kept, stop and report it.

**Owned paths:** `src/observation/live_activity.rs`, `src/observation.rs`, `src/service.rs` (activity wiring only), `src/observation/ipc.rs` (activity paths only), `src/observation/tests/live_activity.rs`, `src/observation/ipc/tests.rs`, and `src/observation/tests.rs`. Read everything else. Do not change `web/`, `pi/`, `crates/portal-types`, or the wire format.

**Acceptance:** `cargo test --bin dogsled live_activity` and `cargo test --bin dogsled observation` pass, including (a)–(d). `cargo test --bin dogsled web::activity` passes unchanged.

**Checks (only these):** the three filters above, `cargo test --test cli activity` (report the count), `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist. If it is missing, ask the lead and do not build it. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `refactor(activity): run the activity authority on its own thread and channel`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** `origin/main` after the U1 + U2 merge. The lead gives the exact SHA at dispatch. This unit edits the same functions as U1 (`refresh_at`, `sample`, `registration_refusals`).

**Reply:** the branch, head SHA, changed paths, each command with its result, how each invariant is covered (test name), and anything left unverified.
