# Activity producer startup registration

Status: proposed on 10 November 2026 against `origin/main` `a479b93`. No `pi/activity.ts`, `src/observation/live_activity.rs`, `src/observation/wake.rs`, `src/service.rs`, or `src/web/activity.rs` change exists between `v0.34.2` and `a479b93`.

## Symptom

Every new Pi session prints a line like this into the TUI:

```text
[dogsled activity] pane=w136%3Ap2M method=POST status=404 code=agent_missing decision=retry
```

## Causes

Four defects combine. Each one is verified as described.

### C1. Diagnostics write raw stderr under the TUI

`pi/activity.ts` `diagnostic()` (lines 437–441) calls `console.error`. PR #317 (`66c5774`) added it. It prints at start, at the first retry of each code, at the acknowledgement, and at each stop and dispose. In TUI mode Pi does not redirect stderr (`output-guard.ts` in Pi v1.5.0 redirects stdout only, and only for print and JSON modes). The lines therefore paint over the TUI. No other `pi/` code writes to the console except `portal-client.ts` `openInBrowser`.

### C2. A new session registers before serve can verify it

- Pi v1.5.0 `SessionManager._persist` writes the session file only after the first user or assistant message (`session-manager.ts` lines 1173–1190). Evidence: after a probe session with no prompt, `~/.pi/agent/sessions/--private-tmp-actprobe--/` stayed empty.
- The extension `message_end` handler runs before that append (`agent-session.ts` lines 1160–1181). The file exists from the next event on.
- Serve verifies producer evidence by reading the session header (`verified_evidence` → `transcript::pi_session_id`, `live_activity.rs` lines 540–551). The registration of a new session therefore cannot succeed until the first prompt.
- The publisher posts at `session_start` anyway. It then retries every 0.5–5 s until the first prompt. Each attempt also renews `registration_interest`, which keeps serve sampling `herdr agent list` every 5 s for an idle session.

### C3. Serve answers registration from a sample taken before the request

- `Authority::register` resolves the pane only from `self.bindings`, the last `herdr agent list` sample (`binding()`, lines 287–314). The code never takes a sample after the request arrives. The comment at line 325 says: "Receipt never moves the sampling deadline".
- `binding()` rejects a cached `evidence: None` with `session_unverified` before `register()` re-reads the header (line 330). A file that appeared after the last sample therefore still fails.
- The Herdr integration (`~/.pi/agent/extensions/herdr-agent-state.ts`, auto-discovered, user scope) awaits `pane.report_agent_session` in its `session_start` handler. Pi loads auto-discovered user extensions before package extensions (`package-manager.ts` `resourcePrecedenceRank`), and `ExtensionRunner.emit` awaits handlers in order (`runner.ts` line 1103). Herdr therefore knows the session before the dogsled POST is sent. This is deduced from source; I did not observe the ordering at runtime. `agent_missing` therefore means that serve has not sampled since Herdr learned the session. It does not mean that Herdr lacks the pane.

### C4. Authority latency follows the busy observation loop

The authority applies commands only inside `publish()` (`service.rs` lines 302–365), once per loop iteration. Eight probe registrations for an absent pane, sent 0.7 s apart on 10 November 2026, gave these results:

| Result | Count | Round trip |
|---|---|---|
| `agent_missing` 404 | 4 | 20 ms, 659 ms, 3.3 s, 4.0 s |
| `upstream_unavailable` "native binding unavailable" (sample older than 10 s) | 1 | 4.2 s |
| `upstream_unavailable` EAGAIN (IPC 5 s timeout) | 3 | 5.0–5.1 s |

`sample(1)` showed that the serve main thread was busy for all of a 6 s window. Its leaf frames were mostly `pread` and `stat`. Other observations, which are not attributed causes: `state.db` 25 MB with a 30 MB WAL, and 21,408 `dogsled job refusal … ssh netcup` lines for two jobs in the last 5 MB of `~/Library/Logs/dogsled.log`. Heartbeat PUTs use the same path, so the same delay threatens the 15 s lease.

## Decisions this plan changes

- PR #317: "receipt never moves the sampling deadline". Changed only for registrations: a registration that the current sample cannot verify causes one binding sample after receipt. Heartbeats and viewers still never cause native reads. This keeps the brief's rules: no native reads in HTTP handlers, and no extra reads from viewers or heartbeats (`plans/briefs/chat-conversation/activity-protocol.md`).
- PR #317: "start and stop messages remain" in diagnostics. Replaced. The client writes nothing to the console. Serve logs refusals.
- Kept: missing or unsaved evidence means that detail is unavailable. Kept: no filename-based identity. A fresh session reports `no_producer` until its first prompt.

## Design

### U1. Serve verifies registration after receipt (`src/observation/live_activity.rs`, `src/service.rs`, tests)

1. Re-verify the evidence when the registration is applied. Do not reject on the cached `binding.evidence`. Keep the session-key and evidence-equality checks.
2. If a pending registration names a pane that is absent from the bindings, sample `herdr agent list` once before applying it. The same applies when the binding was measured before the registration arrived. Do this in the authority path, not in the HTTP handler. Take at most one forced sample per loop iteration, shared by all pending registrations. Publishes, heartbeats, and viewers never force a sample.
3. Return `agent_missing` or `session_unverified` only from a binding measured after receipt.
4. Log refusals in the serve log, once per change of pane and code. Do not log evidence paths or bodies.

Tests: a pane that Herdr lists after the last sample registers in one apply. A session file created after the last sample registers. An absent pane after the post-receipt sample gives `agent_missing`. Publish and viewer traffic adds no native reads. Existing CAS and no-steal tests remain.

### U2. The Pi publisher registers when verifiable and stays silent (`pi/activity.ts`, `pi/activity.test.ts`, `pi/index.ts`)

1. Register at `session_start` only if the session file exists (resume, fork, reload with a conversation). Otherwise, check for the file on boundary events (`message_start`, `message_end`, `turn_end`, `agent_settled`) until it exists. Never check on streaming deltas. The first user `message_end` handler runs before Pi writes the file, so the assistant `message_start` is normally the first event that finds it. That registers before the first reasoning streams. Do not use a timer before the file exists.
2. Delete `diagnostic()` and every `console.error`. Inject a reporter from `registerActivity`, as `TodoPublisher` does in `pi/index.ts`. Use `ctx.ui.notify(…, "warning")` once per publisher, only for actionable terminal failures: `unauthorized`, `unsupported_version`, `invalid_request`, `sequence_conflict`, and `invalid_response`. Keep expected states silent.
3. Keep the PR #317 bounded backoff and exact-request replay for `agent_missing`, `session_unverified`, `upstream_unavailable`, transport, and 5xx outcomes, but silently. After U1 these outcomes should be rare. A boundary-only retry would leave a resumed idle session without a producer if Herdr's session report ever lagged, because I deduced the load order from source and did not observe it.

Tests: a new session sends no activity request before the file exists. It registers at the first boundary event after the file exists. No failure code writes to `console.error` or `process.stderr`. A terminal failure gives exactly one notification. Rewrite the PR #317 test "POST startup agent_missing recovers through registration" to cover boundary-event retry.

### U3. Decouple authority latency from the observation loop

Decision on 10 November 2026: do both, U3a first and then U3b. Write the U3b brief from U3a's findings.

U1 and U2 remove the symptom, but commands still wait for a full loop iteration. Two options:

- **U3a, diagnose the saturated loop first.** This is recommended because the cause affects all of serve. Symbolize a profile of v0.34.2 or reproduce with a debug build, find the work that keeps the main thread busy, and fix that owner.
- **U3b, isolate the authority.** Give it its own thread, its own cheap binding sampler (`herdr agent list` takes about 25 ms), and its own activity watch channel, separate from `Frame`. Commands then finish in milliseconds whatever the publication load. This changes `Publisher` and the IPC activity readers.

## Briefs

`plans/briefs/activity-startup/u1-serve-post-receipt-sample.md`, `u2-pi-deferred-silent-producer.md`, `u3a-serve-loop-saturation.md`.

## Order and acceptance

U1 and U2 are independent. U2 alone removes the console output. U1 makes the first registration succeed. Ship them together, because the release is lockstep. Mixed versions degrade safely: an old extension against a new server retries as today, and a new extension against an old server retries at boundary events.

Final acceptance needs all three:

- Install the released version on all three hosts.
- Open a new Pi session: it prints nothing.
- After the first prompt, the portal shows live detail within one boundary event. A resumed session shows live detail at startup.

## Main frame decision (from U3c timing, v0.35.0)

Decided 2026-11-11 from the U3c `dogsled serve timing` lines. The source is the lead's Mac (`~/Library/Logs/dogsled.log`), with serve on v0.35.0 after the restart at 03:15. Each line covers one 60 s window. The values are `count/total ms/max ms`.

| Phase | Mac, per 60 s window | Share of the window |
|---|---|---|
| `collect.host` | 8 passes, 46,472–48,035 ms total, max 6,237–7,503 ms | about 78% |
| `collect.snapshot` | 11,042–13,838 ms total, max 1,956–3,733 ms | about 20% |
| `collect.terminals` | 1,922–3,526 ms total | about 4% |
| `peers.compose` | about 2,400 ms total | about 4% |
| `publish.replace` (includes `serde_json::to_vec` of the whole frame) | 168–193 ms total, max 23–35 ms | about 0.3% |
| `collect.publication` | 103–157 ms total | about 0.2% |

The `collect` total is 61–64 s per 60 s window, so the loop is fully busy. That matches U3a, which found 0.19 publications per second.

On netcup, `collect.host` is 27.5 s of each 60 s window. `publish.replace` is 2.1–2.4 s of each window over 48 passes, with a maximum of 93 ms.

**Decision 1: latency.** Do not split or slim the main frame to fix publication latency. Serializing and replacing the whole frame costs about 25 ms for each pass, which is 0.3% of the loop. The latency belongs to `collect.host`, which is `observation::collect::host` (`src/observation/collect.rs:19`). That phase does these whole-store reads on every pass:

- `holds::local`;
- `done::list`;
- `store::read(failing_lanes)`.

The second cost is `collect.snapshot`. The next serve-latency unit fixes those owners. U3b already removed the activity authority from the frame path.

**Decision 2: size.** Frame size is a separate correctness risk, and it needs a fix. The Mac frame is now 15,621,607 bytes. `ipc::MAX_FRAME_BYTES` is 16 MiB (16,777,216 bytes), so the frame is at 93% of the cap. When a frame is over the cap, `Publisher::replace` (`src/observation.rs`) returns `observation publication too large`, and no publication is made. The frame was about 9 MB in U3a.

The measured contents:

- `views` is 12.5 MB, and `local` is 3.1 MB.
- The host reports appear three times:
  - `views.hosts`, 1.6 MB;
  - `views.status.data.hosts`, 1.6 MB;
  - `local.status`, 2.4 MB.
- `inventory` and `inventories` are 1.7 MB each.
- Retained records grow without a bound. `status.queue` holds 438 tickets in 1.27 MB, `status.items` holds 586 items, and `status.holds` holds 609 holds.

The owning fix is to remove the duplicate projections, so each fact is in the frame once, and to bound the retained queue, items and holds in the published views. Raising the cap is not a fix.
