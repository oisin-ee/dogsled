# U3a (research): why is the `dogsled serve` observation loop saturated?

**Outcome:** a report that names the code paths that keep the serve main thread busy, with symbolized evidence. The report also explains how this delays activity commands.

**Question.** Activity registration and publish commands reach the authority only once per pass of the serve main loop (`src/service.rs` `run` → `Observation::refresh` → `publish()`, lines ~300–560; `live_activity::Commands::send` has a 5 s timeout). Lead measurements on the local Mac, `dogsled` v0.34.2, pid 73519, 10 November 2026:
- 8 registrations sent 0.7 s apart had round trips of 20 ms, 659 ms, 3.3 s, 4.0 s, 4.2 s, and three 5 s timeouts (EAGAIN).
- One reply said "native binding unavailable", so its sample was older than 10 s.
- `sample 73519 6` showed the main thread busy for all 6 s. The binary is stripped, and the hot leaf frames were `pread` and `stat`.
- Leads, not yet causes: `state.db` 25 MB with a 30 MB WAL; 21,408 `dogsled job refusal … ssh netcup: remote command failed` lines for two jobs (`r-1793906906222-97975-2`, `r-1793906971060-97975-6`) in the last 5 MB of `~/Library/Logs/dogsled.log` (emitted at `src/launch.rs:620`); peer collection timeouts.

**Evidence required**
1. Function-level hot paths with symbols, from a debug-symbol build of `v0.34.2` or `origin/main` reproducing the load, or another way of attributing time. Attribute time per publish phase: `collector.collect`/`refresh`, `branches.observe`, `peers.compose`, `activity.refresh`, `details.refresh`, `diagnostics.publish`, `workers.requests`, and the sweep.
2. Iteration rate and the duration of each pass. Why the loop never idles, and what wakes it (`wake::Dirty` marks, the Herdr event stream, the `job refusal` retry loop).
3. Whether the `job refusal` flood is a retry without backoff, and what owns it.
4. Whether the WAL size reflects a missing checkpoint or long readers, and its effect on `pread` time.
5. A proposed fix owner for each measured cause, kept separate from unmeasured leads. State what U3b (moving the activity authority to its own thread) must take into account.

**Constraints:** read-only.
- Do not edit repository files, commit, or push.
- Do not spawn agents.
- Do not stop, restart, or reconfigure `dogsled serve`/`web` or Herdr.
- Do not write to `~/.local/state/dogsled`. Read-only `sqlite3 -readonly`, `lsof`, `sample`, `log`, `dtrace` if permitted, and source reading are allowed.
- You may build in a temporary checkout outside the repository.
- If you are not on the local Mac, use `dogsled run --host local -- <read-only command>` for local measurements, and state which host each measurement came from.

**Paths you can read:** everything in the repository, `~/Library/Logs/dogsled.log`, and the state directory, read-only.

**Checks:** none.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`.

**Reply:** the findings with commands, hosts, timestamps, and evidence. Label each deduction as a deduction.
