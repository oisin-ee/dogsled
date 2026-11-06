# U2: held remote agent and activity streams

Plan: `plans/git-files-sidebar.md`, unit U2. Base: `83c10cc` (`feat/detail-part-revisions`, U1 accepted).

## Outcome

A remote agent's `events` and `activity/events` streams come from one held SSH process per (host, pane, stream), shared by all viewers. Unrelated portal publications start no SSH call.

## Cause

`src/web/agents.rs:298-357` (`peer_events`) runs a one-shot SSH `host agent-detail` call (`peer_call`, lines 185-226) on every local portal publication (`portal.changed()`, line 337), for every viewer. It forwards each reply even when unchanged. Callers: `src/web/agents.rs:684` (`PeerOperation::Event`) and `src/web/activity.rs:173` (`PeerOperation::Activity`).

## Edits

1. `src/cli.rs:743` `AgentDetail`: add `#[arg(long)] follow: bool`. `src/main.rs:591`: with `follow`, call a new `web::agents::peer_follow(pane, input, ledger)`; it writes lines itself and returns exit success.
2. `src/web/agents.rs` `peer_follow` (remote side): accepts only `Event` or `Activity`, else an error.
   - `Event` holds `client.detail(&pane)`; `Activity` holds `client.activity_subscribe(&pane)`.
   - On each change, build the same `PeerReply` as `peer_read` builds for that operation. Write it as one JSON line to stdout and flush, only when it differs from the last line written.
   - Exit when the subscription disconnects, a stdout write fails, or stdin reaches EOF. Read stdin on its own thread. It never reads Git, transcripts, or foreign paths.
3. New `src/web/peer_stream.rs` (register in `src/web/mod.rs`), the portal-side relay:
   - `Followers` holds a `Mutex<HashMap<(host, pane, stream), watch::Sender<Option<Line>>>>`, stored in `AppState` (`src/web/mod.rs:72`). `Line` is `Ok(PeerReply)` or `Err(String)`.
   - Subscribing returns a `watch::Receiver`. If an entry exists and its sender is not closed, reuse it. Otherwise start a follower.
   - The follower builds argv with `fleet.remote_argv(target, ["agent-detail", pane, "--follow", "--input", op])`, using the same fleet lookup as `peer_call`. It spawns `crate::shell::command("ssh")` with piped stdin and stdout, null stderr, and `process_group(0)`, and reads lines on a blocking thread.
   - Cap each line at `crate::observation::details::CODEC.wire_limit()`. Over the cap, or on invalid JSON, send `Err` and stop.
   - When `sender.closed()` resolves (the last viewer left), when `state.stop` is cancelled, or when the process exits, call `crate::shell::kill_group(pid)` and remove the entry.
   - If the process exits before its first line, send `Err("host {label}: agent stream unavailable")`. Never fall back to one-shot polling.
4. `peer_events` (`src/web/agents.rs:298`): replace the initial `peer_response` call and the loop with a `Followers` subscription.
   - Wait for the first line, bounded by `crate::budget::CALL_LIMIT`. A first line that is `Err`, or has status ≥ 400, becomes the HTTP response, with the same status mapping as `peer_response`.
   - Otherwise start the SSE stream with that first line. Later `Ok` lines use the given event name. `Err` or status ≥ 400 sends `error` and closes the stream.
   - Keep the keepalive and `until_shutdown`. Delete the `portal.changed()` trigger.
5. Keep the `valid_for_ms` reduction in `peer_call` for one-shot reads. Follow lines are written immediately, so the relay does not reduce `valid_for_ms` again.

## Paths

Owns: `src/cli.rs`, `src/main.rs` (the `AgentDetail` arm only), `src/web/agents.rs`, new `src/web/peer_stream.rs`, `src/web/mod.rs`.
Reads: `src/hosts.rs`, `src/shell.rs`, `src/jobs.rs:979-1030` (spawn pattern), `src/observation/ipc.rs`, `src/web/activity.rs`. Do not change `web/`, `portal-types/`, or `src/web/activity.rs`.

## Acceptance

Add tests in `src/web/peer_stream.rs`. Inject the follower's program and argv so a local `sh -c` script stands in for SSH.
- Two subscribers to one key start one process. Its lines reach both.
- Dropping both receivers kills the process group. Check that the pid is gone.
- A script that exits before output sends one `Err` "agent stream unavailable". An oversized line and an invalid line each send `Err`.
- `remote_screen_activity_and_streams_forward_peer_publications` (`src/web/agents.rs:1200`) serves streams through the fake shell. Move its stream half to the injected launcher. Assert the follow argv, and that streams make no fake-shell `agent-detail` call.
- `peer_follow` writes one line per changed detail and none for an unchanged publication. Use the `details::fixture::Fixture` that U1's SSE test uses.

Run `cargo test --bin dogsled web::` and `cargo test --bin dogsled observation::tests::details`. All must pass.

## Checks

`moon run dogsled:fmt-check`, `cargo clippy --all-targets -- -D warnings`, and the tests above. `web/public` is prebuilt by the lead; do not run `web-build`.
