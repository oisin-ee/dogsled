# Sidebar exits when its terminal is lost, and the CLI test reaps every sidebar it starts

**Outcome:** a `dogsled sidebar` whose terminal hangs up exits within 2 s, even when SIGHUP is ignored. The four-sidebar CLI test leaves no sidebar process behind.

**Cause (measured on the lead's Mac, 10 November 2026):** four orphaned `dogsled sidebar --owner w1:p1` processes each used about 93% CPU for two days.
- Their parent was launchd, and nothing else held their tty.
- Their cwd was a deleted `/tmp/ds3-*` rig, created by `tests/cli.rs` `Rig::shortened`.
- `sample` showed this stack: `sidebar::command` → `crossterm::event::poll` → `UnixInternalEventSource::try_read` → `read`.

The loop is in crossterm 0.29.0, `src/event/source/unix/mio.rs:68–115`. When the tty read returns `Ok(0)` (EOF) or an error other than WouldBlock or Interrupted (EIO after hangup), the inner `loop` reads again forever. So `event::poll(TICK)` at `src/sidebar/mod.rs:392` never returns, and the sidebar loop (lines 377–418) never reaches a failing `terminal.draw`. Never patch, fork, or vendor crossterm. The fix belongs to Dogsled's sidebar lifetime.

**Leak:** `tests/cli/observation_sidebar.rs`:
- `Sidebar::start` (lines 13–45) runs `script … sh -c "…; exec $DOGSLED_BIN sidebar"`.
- It owns only the `script` wrapper (`Owned::spawn`, and `Drop` at lines 67–72).
- `script` puts the sidebar in its own session and group, which `Owned::terminate` does not cover (`tests/cli.rs:284–287`).
- The test at line 151 starts four of them.
- The precedent for the fix is `Owned::adopt` with `read_pid` (`tests/cli/review.rs:25–29`, `tests/cli.rs:238`, `:542`).

**Edits**
1. `src/sidebar/mod.rs`: add a Dogsled-owned terminal-liveness guard that does not depend on `event::poll` returning.
   - For example, start a scoped thread that waits on the controlling terminal fd for hangup or error without consuming input. `nix` `poll` would need its `poll` feature enabled in `Cargo.toml`. Changing a feature on an existing dependency is allowed. Adding a new crate is not.
   - When the terminal is gone, the guard ends the process with a non-zero exit. Nothing is left to restore on a dead terminal. It must never fire on a live terminal, a resize, or a key press.
   - Keep the normal-exit and error paths unchanged.
2. `tests/cli/observation_sidebar.rs`:
   - (a) The run script records the sidebar pid (`echo $$ > "$RIG/sidebar-<n>.pid"` before `exec`). `Sidebar` adopts that pid with `Owned::adopt`, and `Drop` terminates both the sidebar and the wrapper.
   - (b) Add a regression test that starts a real sidebar under `script` with SIGHUP ignored (for example `trap '' HUP` before `exec`). The test kills only the `script` wrapper, then asserts that the sidebar pid exits within 2 s. Without edit 1, this test must fail.
   - (c) After the existing four-sidebar test, assert that each adopted pid has exited.

**Owned paths:** `src/sidebar/mod.rs`, `tests/cli/observation_sidebar.rs`, `Cargo.toml` (feature change only), `Cargo.lock`. Read everything else.

**Acceptance:** `cargo test --test cli observation::sidebar` passes with a non-zero test count, including (b). Before your edit to `src/sidebar/mod.rs`, test (b) fails. Show that failure in your reply.

**Checks (only these):** `cargo test --test cli observation::sidebar` (the file is registered as `mod sidebar` in `tests/cli/observation.rs:8–9`), `cargo test --bin dogsled sidebar`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`. `web/public` must exist. If it is missing, ask the lead and do not build it. Commit hooks run as configured. Do not bypass them.

**Publication:** use the commit message `fix(sidebar): exit when the terminal is lost and reap test sidebars`. Push your branch. Do not open a PR, merge, release, or install.

**Base:** `a479b9337e57a3bfa38a22fad6a2dcfe01eb1d34`.

**Reply:** give the branch, head SHA, changed paths, each command with its result, the failing-then-passing evidence for (b), and anything unverified. If the guard cannot be built without a new crate, stop and report.
