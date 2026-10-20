# Changelog

All notable changes to this project are documented in this file.

## Unreleased

- Queue order holds per host. A full host no longer holds back a ticket pinned to another host with room. Before, the first ticket that found no room ended the admission pass for every host.

## 0.14.0 - 2026-10-20

Cleanup uses one loss rule, and the Stop gate starts each cleanup pass.

- One rule decides if removal loses work. It applies to worktrees, detached HEADs, branches, `close`, orphan close, and board facts. The checkout must be clean, and the default branch or freshly fetched remote refs must hold every commit. Ignored files do not count.
- Each pass runs `git fetch --all --prune` first. If the fetch fails, the repository gets a warning, and only the default branch counts.
- Branches with a deleted upstream get no special rule. They go when the loss rule permits removal.
- Prune and the sweep share one list of holds: active panes or processes, the caller, an unreadable process table, open agent items, pending preparations and launches, and foreign locks. An idle shell, an idle agent's own processes, or a workspace alone no longer holds a checkout. The "untouched for a day" rule is removed.
- A missing or unreadable repository is a warning. The survey continues. Before, one missing repository stopped doctor, prune, and every sweep.
- An unheld dirty checkout whose work is in the default branch is a `landed_dirty` finding, with up to ten dirty paths.
- The 10-second sweep in `serve` is removed. The Stop hook writes `sweep-request.json` and wakes the service through `sweep.sock`. The service runs one pass for each request. A socket failure is a warning, and the request file stays the source of truth.
- Each pass is recorded in `sweep.json` and in `sweeps.jsonl`, which keeps seven days. `status` shows the latest pass. The gate reports failures, repository warnings, and landed-dirty findings once for each distinct set. Successful removals give no reminder.
- Breaking: `close` refuses local-only work, also for an explicit owner close.

## 0.13.1 - 2026-10-19

- Agent admission retries retain the selected host, prepared workspace, and original terminal identity. They do not prepare the branch again.
- Interrupted starts reconcile a matching agent or a saved success receipt. Unknown outcomes fail without sending the task again.
- Replaced terminals and foreign agents are refused. Interrupted jobs remain inspection-required instead of being restarted.
- Existing partial placements are preserved and are not adopted automatically.

## 0.13.0 - 2026-10-19

wrangle is now dogsled. The old name was too close to Cloudflare's wrangler. Nothing reads the old names, so update every host at once.

- The binary, crate, and repository are `dogsled` (`oisin-ee/dogsled`). The Pi package is `@oisin-ee/pi-dogsled`.
- The Claude and Codex plugin is `dogsled@dogsled`, the Herdr plugin is `dogsled`, and the MCP server is `dogsled` (`mcp__dogsled__*`).
- The environment variables are `DOGSLED_CONFIG`, `DOGSLED_STATE`, and `DOGSLED_BIN`. A repository's worktree file is `dogsled.toml`.
- Configuration and state live in `~/.config/dogsled` and `~/.local/state/dogsled`. Move the old directories while the service is stopped. The service units are `dev.dogsled.plist` and `dogsled.service`.

## 0.12.3 - 2026-10-19

- The Pi work widget uses the wrangle board design again: a header with counts and a load meter for each host, then the current task, then agents, jobs, and queued work grouped TO LAND, ATTENTION, RUNNING, and WAITING with aligned harness·model, host, activity, and age columns. This replaces the compact WORK strip from 0.12.1.
- Alt+W and `/work` still expand the widget in place. The expanded widget keeps the board's header, groups, and columns, and adds a view line, a selection marker, and key hints.
- The Todos view hosts the provider-owned pi-todo view inline.

## 0.12.2 - 2026-10-18

- The compact Pi widget shows the current task and named agents and jobs above the editor.
- Alt+W and `/work` expand or collapse the same widget. Work details stay inline. The editor draft stays unchanged.
- Modified shortcuts navigate work and run explicit actions without taking ordinary typing from the editor.
- Pre-commit checks select Rust and Pi verification from staged paths. Changes to shared check configuration still run both gates.

## 0.12.1 - 2026-10-18

The 0.12.0 release was cancelled before publication to synchronize the Cargo lockfile.
The shared work UI ships in 0.12.1.

- Pi shows one WORK strip for branch-backed todos and owner-pane agents and jobs. Compatible pi-todo providers retain standalone display when the shared presenter is absent.
- `/work` opens a bounded keyboard-operated overlay. Inspect read-only todos, agent output, and retained job tails; confirm job stops and queued cancellations, or send inbox messages. `/wrangle` retains administration.
- Job guidance uses non-blocking Wrangle commands instead of background-task tools. Execution and completion delivery retain their existing owners.

## 0.11.0 - 2026-10-18

Nothing types into an agent's terminal any more.

- `wrangle hook inbox --harness claude|codex --event Stop|UserPromptSubmit` reads the inbox for Claude Code and Codex. The plugin registers it on both events. At `Stop` records continue the turn (`decision: block`); at `UserPromptSubmit` they are `additionalContext` beside the user's text. An idle Claude or Codex agent sees a record at its next prompt or stop.
- Watchdog check-ins are `check-in` inbox records, not `herdr agent prompt`. An owner without an inbox reader gets none until it has one. A finished child's reply comes only from the sweep; its check-in is no longer immediate. `watchdog.json` drops `deliveries`.
- Admission notices go only through the inbox, to working owners too.
- The Pi extension is published to npm as `@oisin-ee/pi-wrangle` with each release, after the binaries; `pi install npm:@oisin-ee/pi-wrangle@<version>`. The package holds only `pi/` and the skill. At session start it warns when `wrangle --version` differs from its own version.

## 0.10.0 - 2026-10-18

Wrangle replaces Shepherdr for delegation: one board, one inbox, and children that close on events.

- `wrangle agent --kind pi|claude|codex` passes the task as a launch argument, or as `launches/<id>.task.md` when it has a newline, a leading `-` or `@`, or more than 4000 characters. `--message-file` reads it from a file or stdin. Nothing is typed into the new pane. A failed `herdr agent start` is retried with backoff, three tries in all, then the owner's inbox reports the failure.
- `--profile NAME` (Pi) reads `~/.config/wrangle/profiles/NAME/profile.json` on the child's host and passes `--model`, `--thinking`, `--append-system-prompt`, and `pi_args`. Shepherdr profiles load unchanged. `wrangle profiles` lists them.
- The inbox: `wrangle send TARGET [--host H] --message|--message-file`, `inbox claim|ack|list|path`. Records live per Herdr terminal on the target's host; a claim is a lease, offered again with backoff until acknowledged. A target without a reader fails at once and the sender hears so. The service gains an inbox lane.
- Event-driven close: when a child's turn completes and it is idle or done, the sweep captures its final reply, puts it in the owner's inbox, and closes a clean reader or a writer whose branch has landed. A writer with commits waits under TO LAND; blocked and working children never close; nothing closes on a timer. A blocked child sends one `blocked` record per episode.
- `wrangle board [--owner|--all] [--json]`: an owner's children, jobs, and tickets grouped TO LAND, ATTENTION, RUNNING, and WAITING, with harness, model, and activity. The prompt context's close commands become one `To land:` line.
- `wrangle read TARGET`: a child's or job's newest output and its captured reply.
- `wrangle mcp` serves `status`, `spawn`, `run`, `send`, `inbox`, `read`, `close`, `stop`, `cancel`, and `place` as MCP tools over stdio. The Pi extension, the Claude plugin (`.mcp.json`), and the Codex plugin (`codex-mcp.json`) register it. No tool waits.
- One backoff (10 s, doubling with jitter, 5 min cap) for inbox leases, follow-up, peer reads, and launch retries. Host reports gain `retry_ms`; `wrangle retry HOST` reads a host again now.
- `wrangle machine list|rename|enable|disable|reconnect|add` manages Herdr's saved machines; `reconnect` and `add` open a tab for SSH prompts.
- Pi: a board widget replaces the items widget; `/wrangle` opens a control panel (Status, Hosts, Profiles, Inbox); the extension reads its inbox, injects records as `wrangle` messages (steer while running, a new turn when idle), and acknowledges them once Pi has saved them. `@earendil-works/pi-tui` is a peer dependency.
- Breaking: the Pi extension no longer tracks or places Shepherdr `agents spawn` calls, and `pi/placement.ts` is gone. Use `wrangle agent` or the `spawn` tool.
- `close` removes only agent worktrees (locked `agent:worktree`). A child in any other checkout, such as its owner's, loses only its pane. Before, closing such a child removed the owner's worktree.
- Probe agents gain `terminal_id`, `completion_seq`, `cwd`, the session's model and activity, and worktree facts (branch, ahead, dirty, landed). Items gain `harness`, `profile`, `model`, `role`, and `reply`; old `items.json` still loads.

## 0.9.1 - 2026-10-16

- Admission counts every active agent (live and not `idle`) as declared demand for as long as it runs, not only for its 90 s reservation. A host now needs `cores − reservation_load × (active agents + reservations)` of at least `free_core_fraction_min × cores`, and total memory of at least `memory_reserve_bytes × (active agents + reservations + 1)`, as well as the measured CPU and free-memory rules. Headroom is the smaller of the measured and declared free cores. An agent waiting on its model used almost no CPU, so a host with many agents looked free between builds; netcup admitted 13 agents in 32 minutes and ran at busy p90 0.95. The refusal reason now names the active count.
- `place`, `run`, and `agent` queue a new request when a ticket is already pending, so the service admits tickets in order. Before, a new request could take a host slot ahead of waiting tickets.

## 0.9.0 - 2026-10-15

- `serve` sweeps on its own action worker, beside the watchdog and admission. Each pass builds the doctor plan in sweep scope and applies only entries whose removal loses nothing: ended job tabs and shells in deleted directories (and a workspace made only of them), clean integrated worktrees and clean detached worktrees whose HEAD is on a remote when no pane, workspace, or process holds them, and local branches with no worktree whose every commit is on a remote ref. It uses the same checks as `close` and `prune`, never touches the service's own pane, workspace, or directory, logs each removal as a `wrangle sweep:` line on stderr, and continues past a failed entry. Doctor and `prune` behave as before; everything else stays for `prune`.
- The sweep closes an orphaned child as `wrangle close` would when its owner pane no longer exists, its agent is idle or done, and its worktree is clean with every commit on a remote ref. A remote child closes with the new `host close --pushed`. A dirty, unpushed, working, or blocked orphan stays open: `status` shows `orphaned:` with the reason, the brief counts it, and every check-in on the host lists it. Status JSON gains `orphans`.
- Doctor reports a `targets` class: build output, a directory with a `CACHEDIR.TAG` cache-directory signature directly under a main checkout or linked worktree, such as Rust's `target/`. It is removable when no workspace shows the checkout and no pane or process works in it; only `prune targets` removes it, after it checks the tag again.
- Doctor also reports a branch as integrated when a clean `git merge-tree` result equals the default branch's tree. The detail names the merge-tree rule. Conflicts and changed trees do not satisfy this rule. Integration checks disable external diff tools and text conversion. The merge-tree check uses an isolated object store without user configuration or attributes.
- Close each job tab after `exec` saves the final record and reports completion. `wait` and `logs` still work after the tab closes.
- Log pane-close failures to stderr without changing the job result. Prune retains its ended-job-tab fallback.
- Create and remove worktrees natively, without `wt` or host `mise` tasks. An optional `wrangle.toml` in the repository's main checkout sets `[worktree]` `dir` (default `.worktrees`), `setup`, and `teardown`. Commands run with `sh -c` in the worktree and can use `{main}`, `{path}`, and `{branch}`, each quoted as one shell word. Unknown placeholders and unknown keys are errors. A missing file means the defaults.
- `place`, `agent --branch`, and `run --branch` run `git worktree add` (a new branch from `--base` or the default branch, or an existing branch), lock the worktree with the reason `agent:worktree`, run `setup`, and open it with `herdr worktree open --no-focus`. A failed `setup` removes the new worktree and the branch it made, and placement fails. When the main checkout had no Herdr workspace, wrangle records the anchor in `.git/agent-worktree-anchor`. Refusals match the old `agent:worktree` task: an invalid branch name, `--base` for an existing branch, a foreign lock, and a workspace that already shows the worktree.
- `close` and `prune` run `teardown` in the worktree first, so a failure keeps the worktree, its panes, and its lock. Then they close its panes and workspace, unlock, and run `git worktree remove` without force. The branch is deleted only when the integration check finds it integrated, so a squash-merged branch now goes too. After the last child, the anchor workspace closes when it is unfocused, is not the caller's, and has one pane with no agent.
- `close` also refuses a worktree outside the repository's worktree directory and a caller whose directory is inside the worktree. The JSON `worktree` result gains `anchor_closed` when the anchor closed.
- Doctor lists detached-HEAD worktrees. One is removable when it is clean, no pane holds it, and a remote branch holds its HEAD. `close` refuses a detached worktree and points at `prune`.
- Breaking: remove `hooks.prepare` and `hooks.remove` from the host config. A `[hooks]` table is now an unknown-key error. Move repository remove hooks, such as `.config/wt.toml` `[pre-remove]`, to `wrangle.toml` `teardown`.

## 0.8.2 - 2026-10-15

- Prune closes an ended job's tab at once when its shell is at an empty prompt. The job log keeps the output. A shell in a directory that no longer exists also closes at once. Other idle shells still need a day.

## 0.8.1 - 2026-10-15

- Doctor's process scan tolerates panes that close after the pane list. All process queries share the same closed-pane handling.

## 0.8.0 - 2026-10-15

- Doctor no longer lists a new worktree as merged when its base moves on. A worktree whose HEAD has not moved since Git made it, on a branch with no commits beyond the base, holds no work: it is ordinary work for its first day, then removable. Before, its branch counted as merged as soon as it was an ancestor of the base, so a worktree an agent had just made was removable.
- Doctor and prune no longer stop when a pane closes between `herdr pane list` and `herdr pane process-info`. A closed pane runs nothing.
- Admit on free cores and free memory instead of the one-minute load average. Free cores are `cores × (1 − busy) − reservation_load × reservations`, busy being the CPU busy fraction over the last probe interval (`/proc/stat` deltas on Linux, `iostat` on macOS). A host needs `free_core_fraction_min × cores` free (default 0.25), `memory_reserve_bytes` free per reservation plus one (default 3 GiB), and macOS memory pressure below critical. On the measured fleet the load rule refused the Mac while it was free 84% of the time; the research note in `docs/research/2026-10-15-admission-metric.md` records the evidence. Config keys `load_per_core_max` is gone; `free_core_fraction_min` and `memory_reserve_bytes` replace it (unknown keys are still errors).
- Probe `busy`, `cpu_ticks`, and `memory.pressure_level`; status lines show busy cores over cores. With a finite cgroup limit the probe reports whichever of the cgroup and `/proc/meminfo` has less free memory.
- Append every probe to `~/.local/state/wrangle/samples.jsonl` (seven-day window) so thresholds can be re-scored on real data.

||||||| 7178211
## 0.7.3 - 2026-10-15

- Find a stacked branch as integrated when each branch in the stack landed as its own squash. A branch whose commits split into consecutive runs, each applied by one commit on the default branch, is integrated; doctor names every applying commit.
- Doctor finds more cruft. New classes: `branches` (local branches without a worktree, removable when integrated or fully on a remote), `workspaces` (a workspace whose every pane is idle closes whole), and `repositories` (a configured path that is not a checkout).
- Doctor finds a shell pane idle for 24 hours. The service records when each shell last had no foreground command, because Herdr reports no activity time.
- Doctor removes a worktree with no commits a day after Git made it, and reports unused worktrees with unmerged work: commits ahead, commits on no remote, and the last commit's age.
- Doctor reads repositories from Herdr workspaces and the new `[cleanup] repositories` policy, so repositories without a live pane are checked.
- The integration search merges each path on its own and reads each commit's changes from one `git log`. On a host with 250 branches, doctor took 13 seconds instead of 7 minutes.

## 0.7.2 - 2026-10-14

- Let prune try every entry. A failed or refused entry no longer stops the rest; prune reports each failure and exits with status 2. `--json` returns `{results,failures}`.
- Drop a worktree's registration when `close` removes it, so doctor no longer lists it as a stale registration.
- Show the last lines of a failed remove hook's output, not only the task runner's summary line.

## 0.7.1 - 2026-10-14

- Never list a repository's main checkout as a cleanup candidate, even when it lags the remote default branch. List each linked worktree once when the repository is registered through a symlink.

## 0.7.0 - 2026-10-14

- Skill: spawn a placed agent with `placement=pane pane=<pane_id>`. Without `placement`, Shepherdr ignored the prepared pane and opened a tab in the caller's workspace.
- Pi: give every `agents spawn` without a `placement` its own worktree workspace. Use the prepared or open worktree pane, or place a new `agent/<label>` branch with `wrangle place`. A queued or failed placement blocks the spawn with its reason.
- Find integrated worktrees from Git alone: a deleted branch, an ancestor of the remote default branch, or a branch that one commit there applies (merge, squash, or cherry-pick). A branch on the base tip is not a candidate.
- Do not count a shell with no foreground command as holding a worktree. Prune closes such panes and the worktree's workspace before removal. Doctor reports held integrated worktrees with the reason.
- Add `hooks.remove` (`{branch}`, `{path}`), default `mise run -q agent:worktree-remove -- {branch}`. `close` and `prune` both remove through it; wrangle no longer runs `wt` or `git worktree remove` itself.
- Let `close` finish a child tab in the caller's own workspace. Refuse only the caller's own pane, a worktree the caller works in or shows, and busy panes in the worktree.
- Print the full `wrangle close PANE --host LABEL` command for each finished child in `status`, prompt context, and check-ins.
- Let prune drop a stale registration whose worktree directory is gone. Run remote `doctor` and `prune` under the setup time limit.

## 0.6.2 - 2026-10-14

- Accept a host checkout whose origin names the same repository in another spelling, such as `git@github.com:owner/repo.git` for `https://github.com/owner/repo.git`. Compare host and repository path, not the raw URL.

## 0.6.1 - 2026-10-14

- Remove the hourly automatic worktree cleanup from `serve`. Wrangle never removes anything on a timer.
- Prompt the owner as soon as its child finishes, with the exact `wrangle close` command. Idle and done count as one event.
- List close commands in prompt context only for the caller's own finished children. `status --brief` shows only the count.
- Stop prompting about an agent item after it closes.

## 0.6.0 - 2026-10-14

- Add `close PANE [--host H]`: close a finished child's Herdr workspace, remove its worktree with `wt remove` so the repository's remove hooks run, and end its item. Refuse active agents, dirty or foreign-locked worktrees, and the caller's own workspace before any change.
- Keep an idle or done agent item open until its pane closes. Check-ins name the exact `wrangle close` command, and the brief, status, and Pi widget show unclosed children.

## 0.5.0 - 2026-10-13

- Detect transcript stalls, publish stall timestamps, and set or clear sidebar marks. Include stall counts in prompt context.
- Add durable idle-owner check-ins, job-completion events, and top-level stall notifications, independent from admission and publication.
- Resolve exact Claude and Codex transcript session IDs without choosing an ambiguous file.
- Add read-only `doctor`, confirmed `prune`, and hourly safe worktree cleanup in registered repositories.
- Revalidate complete Git identities and remove only the approved path. Keep repository registrations after the last worktree removal.
- Preserve cleanup reports when registered paths are missing or unreadable. Never kill reported processes or force-remove worktrees.
- Stop observing closed items. Show sanitized transcript activity rather than agent footer lines in check-ins.

- Package wrangle hooks and one shared skill for Pi, Claude Code, and Codex. Add Claude and Codex marketplaces.
- Add `hook context --harness claude|codex|pi`, with event-specific JSON or plain text from the shared brief formatter.
- Bound prompt context to five seconds. Missing Herdr, snapshots, or binaries do not block prompts.
- Keep package versions aligned with Cargo and verify plugin paths and release service templates in tests.

- Keep job capture and exit status independent from a closed pane output.
- Enforce one command deadline across discovery, preparation, transport, and lock waits. Bound routine operations to 30 seconds and setup operations to 10 minutes.
- Run service actions independently from snapshot publication and record ticket timeouts without stopping the probe loop.
- Keep the Pi polling timer active after an initial refresh failure. Race concurrent store tests through barriers.

- Add jobs (`run`, `exec`, `ps`, `logs`, `stop`, `wait`) with output logs, process-group cancellation, deadlines, and sidebar lifecycle reports.
- Add harness-independent `agent`, owner-host `track` and `items`, and locked FIFO admission with `cancel` and idle-owner wake-ups.
- Add jobs to host snapshots and open items and tickets to status. Service admission and owner reconciliation use the peer cache.
- Clone missing repository checkouts from the caller's origin before prepare or launch. Reject origin conflicts without overwriting paths.
- Check mise trust after fresh clones and trust only the new checkout when mise refuses its configuration.
- Track Shepherdr spawns in Pi and show open jobs and tickets above the editor, refreshed every ten seconds and after tool results.
- Add `serve` with local-only probes, atomic `host.json`, memory used/total, and a configurable ten-second interval.
- Record each agent pane, harness, state, transcript path, and transcript file modification time in the host snapshot.
- Make `status` read snapshots, with parallel peer reads and a shared ten-second cache. Show stale and unavailable hosts instead of hiding them.
- Add `status --line`, `status --brief`, and `place`. Placement reserves capacity until its TTL (90 seconds by default) and can prepare a worktree.
- Run host-policy prepare hooks in the repository checkout. Restore `--base` and literal `{branch}`/`{base}` substitution. Reject unknown placeholders. Remove repository hook overrides.
- Report Linux working-set memory from the cgroup with the smallest finite ancestor limit.
- Bundle launchd and systemd service definitions in release archives. Dotfiles installs them later.
- Replace the Pi spawn tool with CLI placement and keep prompt context from `wrangle status --brief`.
- Remove leads, marks, the old spawn/ticket API, lifecycle reports, and Herdr release hooks. The new stores do not migrate old ticket state.
- Remove the sidebar snippet and obsolete lead/placement plan. Keep the Herdr status-board popup.
- Document the approved transcript-based stalls, owner check-ins, and cleanup decisions for later units.

This entry does not change package versions or publish a release.

## 0.4.0 - 2026-10-09

- Repository config: an optional `wrangle.toml` at the root of a repository sets
  `[hooks] prepare` for spawns from that repository. It replaces the host policy's hook. The lead
  reads it from the main checkout. Admission thresholds stay in `~/.config/wrangle/config.toml`.
- The `prepare` hook's `{repo}` is now valid on the admitted host. A directory resolves to its
  main checkout, so a lead inside a linked worktree works. On a remote host the checkout is
  relative to home, where ssh starts the hook. Hooks no longer need to rebuild the path from a
  fixed folder such as `~/dev/<name>`.

## 0.3.0 - 2026-10-09

- Lifecycle log: the ledger appends `queued`, `admitted`, `spawned`, `released`, and `cancelled`
  events to `events.jsonl` under its lock. It stores no prompts or messages, keeps 30 days, and
  skips a torn line instead of failing.
- New command `wrangle report [--since <duration>] [--json]`: per-ticket queue wait and run time,
  plus host refusal counts with reasons.
- `wrangle status` shows the refusal reason for every ineligible host.
- The Pi tool passes `base` only to the `prepare` hook, never to Shepherdr's spawn.

## 0.2.1 - 2026-10-09

- The Pi tool never blocks: every spawn goes to `agents spawn` with `blocking: false`, so the call
  returns once the child has its task and the lead keeps working. The child's completion arrives
  as a Shepherdr message. The `blocking` parameter is removed.

## 0.2.0 - 2026-10-08

- One lead per repository. The first spawn from a Herdr pane claims the lead for the pane's
  repository (`⌂ <repo>`, tokens `role=lead`, `repo=`). A second live lead is refused with
  `lead_exists` before anything is admitted; `take_over=true` / `--take-over` moves it.
  New command `wrangle lead`.
- Children carry `lead=lead:<repo>` instead of `owner=`. `wrangle status` lists every lead with
  its children before the hosts; older `owner=` children are grouped too.
- Layout is no longer a choice: the Pi tool drops `placement`, `workspace`, and `pane`, and the
  CLI drops `--workspace`. A child with a branch opens in its worktree; any other child opens as
  a tab in the lead's workspace (a new workspace on a remote host). `spawn --lead` is optional
  inside Herdr.
- Pi widget shows the session's lead, its children by status, the queue, and host headroom;
  refreshes on every call and every 60 s while work runs. Toasts on spawn and on `lead_exists`.
- `wrangle status --sidebar` prints the Herdr sidebar rows (`herdr-sidebar.toml`).

## 0.1.1 - 2026-10-08

- Herdr plugin: `pane.closed` and `pane.exited` release the pane's reservation; a `board`
  popup runs `wrangle status --watch`.

## 0.1.0 - 2026-10-08

- Skill `wrangle`: the orchestration procedure for Pi and for shell harnesses.
- README: install, configuration, CLI, Pi tool, sidebar rows, skill.

- Pi tool `wrangle`: `spawn` (default) admits on the host with the most headroom, runs the
  `prepare` hook when `branch` is set, calls Shepherdr `agents spawn` on that host, and marks
  the child pane; `status`, `cancel`, `help`. Queued calls return a ticket at once.
- Queue: tickets persist in the session (`wrangle-queue` entry), a poll timer (30 s → 60 s)
  re-admits the oldest ticket, admission sends a `wrangle` wake-up message, `wrangle ticket=…`
  reuses the stored arguments; queued tickets show in a widget above the editor, as a
  `queue` token on the lead's Herdr pane, and as a toast.
- CLI `spawn` for harnesses that are not Pi (Claude Code, Codex, …): admit or resume a ticket,
  place a pane (the `prepare` hook with `--branch`, else a new tab or workspace), `herdr agent
  start`, mark, `herdr agent prompt`. An agent blocked on a startup dialog keeps its pane and
  returns `status: blocked` with the next step.
- Engine: `prepare` command runs the policy's `[hooks] prepare` template on a host.
- Engine: `probe`, `admit`, `release`, `status`, `cancel`, `mark`, `queue`, `notify`, and the
  per-host `host` primitives. Balanced admission by headroom, TTL reservations under a
  per-host lock, queue tickets, remote hosts over `ssh -T` (`remote_command`, default
  `mise x -- wrangle`).
- Scaffold: Cargo crate, Pi package manifest, CI, cargo-dist release workflow.
