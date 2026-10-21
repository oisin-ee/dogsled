# dogsled

Host status and placement for coding agents across [Herdr](https://herdr.dev) hosts.

Dogsled measures hosts, starts Pi, Claude, and Codex children and jobs on the host with room, queues work when every host is full, and carries every message between agents through an inbox. Nothing types into an agent's terminal.

## Install

Install the binary on each Herdr host. Use the same release tag in each command.

Binary:

```sh
mise use github:oisin-ee/dogsled@v0.16.4
```

Pi (extension, tools, and skill), from npm or as part of `@oisin-ee/oisinpi`:

```sh
pi install npm:@oisin-ee/pi-dogsled@0.16.4
```

At session start the extension compares its version with `dogsled --version` and warns when they differ.

Claude Code (hooks, tools, and skill):

```sh
claude plugin marketplace add oisin-ee/dogsled#v0.16.4
claude plugin install dogsled@dogsled
```

Codex (hooks, tools, and skill):

```sh
codex plugin marketplace add oisin-ee/dogsled --ref v0.16.4
codex plugin add dogsled@dogsled
```

Codex requires review and trust before it runs plugin hooks. See the [Codex hooks documentation](https://developers.openai.com/codex/hooks#plugin-hooks).
The [Codex CLI reference](https://developers.openai.com/codex/cli/reference#codex-plugin) describes plugin installation.

Herdr (right-side work board):

```sh
herdr plugin install oisin-ee/dogsled --ref v0.16.4 --yes
```

Harness plugins use the binary on `PATH`. They do not install it or start host services.

## Commands

```sh
dogsled serve                     # local measurements every 10 seconds
dogsled serve --interval 20        # change the interval, in seconds
dogsled serve --once               # publish and run each action lane once
dogsled status                    # hosts, agents, and reservations
dogsled status --json
dogsled status --line              # one line for the tab row
dogsled status --brief             # at most six lines for prompt hooks
dogsled status --watch             # refresh until interrupted
dogsled place --json               # reserve the host with the most headroom
dogsled place --host netcup --json # pin by host id or label
dogsled place --branch feat/unit-a --base main --repo /path/to/repo --json
```

Exit codes: 0 success, 1 queued without waiting, 2 error. Job waits return the job's exit code.
When every host is full, `place`, `agent`, and `run` print `{"queued":true,"ticket":"…"}`.
`--wait --max 30m` waits for admission. A wait that reaches its deadline prints `still running` and exits 124.

## Jobs, agents, and owners

```sh
dogsled run --wait -- sh -c 'echo hi; exit 3'
dogsled run --host netcup --cwd /tmp --timeout 1h --idle-timeout 10m -- make test
dogsled run --branch feat/job --base main --cwd /path/to/repo -- cargo test
dogsled ps --all
dogsled logs JOB --follow
dogsled stop JOB
dogsled wait JOB --max 30m
dogsled agent --kind codex --name reviewer --message 'Review the change' --branch feat/review --base main
dogsled agent --profile general --name api-dups --message-file task.md --branch feat/api-dups
dogsled board
dogsled read api-dups
dogsled send api-dups --message 'Also cover the empty case.'
dogsled track netcup w1:p2 --name child
dogsled items --owner "$HERDR_PANE_ID" --json
dogsled cancel TICKET
```

With `--branch`, an agent uses the pane of the worktree's new Herdr workspace.
Other work in a repository opens a new tab where that repository is shown: the workspace of its worktree, else the main checkout's workspace.
When no workspace shows the repository, dogsled opens a new workspace for it.
Work outside any repository uses a new tab in the caller's workspace. On a remote host, that is the workspace with the caller's workspace name, created when absent.
Without a branch, a remote non-repository `--cwd` must exist there.
Repository directories map to the same path relative to home. A linked-worktree caller needs `--branch` to preserve its branch remotely.

The pane runs `dogsled exec ID -- COMMAND`. Output goes to the pane and `jobs/ID.log` on that host.
If the pane output closes, capture continues to the log and preserves the child exit code.
The atomic `jobs/ID.json` contains the command, directory, host, owner, exit code, signal, reason, and millisecond timestamps.
After the job ends, `exec` saves this final record, reports completion, then closes its own pane and tab.
`wait` reads the final record, and `logs` reads the saved output after the tab closes.
If the close fails, `exec` logs the error to stderr without changing the result. Prune can close the ended job tab later.
`host.json` includes job records. `ps --all` includes completed jobs and reads remote records through the snapshot cache.
`logs`, `stop`, and `wait` use the configured SSH transport for remote jobs.
`stop` requests a process-group kill. Timeouts and wrapper termination also kill the process group.
Both timeout defaults are unlimited. Durations accept integer `ms`, `s`, `m`, and `h` suffixes.
The sidebar shows `working`, then `idle` for success or `blocked` for failure, with a `dogsled` exit or timeout token.

`--max` sets one deadline from command startup, including discovery, placement, admission, and job completion. It defaults to 30 minutes.
Operations use the remaining command budget, capped at 30 seconds for routine calls and locks, or 10 minutes for setup.
Setup includes worktree creation and removal, `setup` and `teardown` commands, repository clones, mise trust checks, and agent startup, locally and through SSH.
A non-waiting action reports the operation that exceeded its deadline.
`logs --follow` can continue indefinitely, but each remote read has a 30-second limit.
Run long waits through the harness's background-task facility.
A wait deadline does not cancel its job or ticket. `cancel` only cancels pending tickets.

### Agents and profiles

`dogsled agent` starts a child with `herdr agent start <name> --kind pi|claude|codex --pane <pane> -- <args>`. The task is a launch argument, so nothing is typed into the new pane.
A task with a newline, a leading `-` or `@`, or more than 4000 characters goes to `launches/<id>.task.md`. Pi reads it as `@<file>`; Claude and Codex get a one-line pointer to it.
`--message-file PATH` (or `-` for stdin) reads the task from a file.
A failed `herdr agent start` is retried with backoff, three tries in all. After the last, the ticket fails and the owner's inbox says so. A refusal or a deadline fails at once.
Herdr refuses a start before it types into the pane, for example `agent_pane_busy` or `agent_name_taken`. That refusal is final: dogsled records it as the launch result, closes the empty pane it created, and does not retry. Any other failure leaves the outcome unknown, so dogsled keeps the pane and reconciles it on the next try.

`--profile NAME` (Pi only) reads `~/.config/dogsled/profiles/NAME/profile.json` on the host that runs the child:

```json
{"description": "Ordinary units", "model": "alias/sol", "thinking": "medium", "prompt": "prompt.md", "role": "writer", "pi_args": []}
```

The child gets `--model`, `--thinking`, `--append-system-prompt <prompt>`, then `pi_args`. Unknown keys are ignored, so Shepherdr profiles load as they are.
`prompt` can also be a list, such as `["../_shared/lead.md", "prompt.md"]`. Each file becomes its own `--append-system-prompt`, in order, so profiles can share one prompt. A directory without `profile.json`, such as `_shared`, is not a profile. The profile panel edits the last file.
`role` is `reader` or `writer`. Without it, a child with `--branch` is a writer, and any other child a reader.
`dogsled profiles` lists them; a broken profile shows as `broken: <reason>`.

### Items and closing

The owner's host stores locked `items.json` and `queue.json` files.
An item records its host, pane, owner pane, creation time, closure time, and for an agent its harness, profile, model, role, and captured final reply.
`items --json` returns `{items,queue}`. `--owner` filters both collections.
The service closes a job item after job completion, and any item after pane removal.

Closing is driven by events. When a child's turn completes (Herdr's `completion_seq` goes up) and it is `idle` or `done`, the sweep:

1. reads its final reply from the transcript and keeps it in the item;
2. puts the reply in the owner's inbox (see [Inbox](#inbox));
3. if the child called `done` during that turn, closes it when it is a reader whose worktree is clean with no commits outside the default branch, or when its branch has landed.

Herdr's `idle` and `done` only mean that the agent is ready for input. A child that asks a question, or that waits for its own children, is idle too. So a child says it has finished with the `done` MCP tool or `dogsled done`, in the turn that completes its task. Dogsled tells each child this at launch: Pi and Claude through `--append-system-prompt`, Codex through `-c developer_instructions`. The mark lives in the host's `done.json`, keyed by the Herdr terminal. A later turn, or an inbox record delivered to the child, ends it. `done` is refused while the caller owns open items or queued tickets.
A child that ends a turn without `done` stays open, and its reply tells the owner it waits for input.

A child with commits to land stays open and shows under TO LAND on the board, and the prompt line says `To land: web-tests (+2)`. Merge or drop the branch; the next sweep closes it.
A `blocked` child stays open, and the owner's inbox gets one `blocked` record per episode. A `working` child, one that has never completed a turn, and one that has not called `done` are never closed automatically. Nothing closes on a timer.

`dogsled board` groups an owner's children, jobs, and queued tickets by what the owner must do: TO LAND, ATTENTION, RUNNING, and WAITING. Each agent row names the harness and model; each job row names its command. `--json` is what the Pi widget draws.
`dogsled read TARGET [--host H]` shows a child's or job's newest output and the captured final reply.

```sh
dogsled close w2:p1
dogsled close w9:p1 --host netcup --json
dogsled done                       # as a child: my whole task is complete
```

`close` ends a child now, for an owner who does not want to wait for the sweep. `--host` defaults to the host of the pane's open item, else local.

For an interrupted launch with no final receipt, explicit `close` can retire its exact idle shell under the request and pane locks. This closes **only the pane**, retaining the checkout, preparation holds, and unknown launch evidence. It does not report that the task never started. Competing launch records, a changed terminal, any agent, or incomplete foreground evidence refuse recovery. Automatic cleanup never uses this recovery path; normal cleanup can assess the retained checkout after its holds are resolved.
In an agent worktree (locked `agent:worktree`, as dogsled makes them), it runs the repository's `teardown` commands in the worktree (see [Repository configuration](#repository-configuration)).
Then it closes every pane that uses the worktree and the Herdr workspace that shows it, unlocks the worktree, and runs `git worktree remove` without force.
Close uses the same loss rule as cleanup: no unique checkout content and every commit held by the default branch or freshly fetched remote refs. It deletes the branch under that same rule. Local-only work refuses close, including an explicit owner close.
When `place` opened the main checkout's workspace as the anchor and no linked worktree is open in Herdr any more, close closes that anchor too. It does so only while the anchor is unfocused, is not the caller's, and has one pane with no agent.
A pane outside a worktree, or in an unlocked worktree (someone's own checkout), is closed alone; the checkout stays.
A child tab in the caller's own workspace is closed. Close refuses a missing pane, the caller's own pane, a worktree the caller works in or whose workspace is the caller's, a worktree outside the repository's worktree directory, a dirty or detached worktree, and any other lock.
It also refuses a busy target or a busy pane that uses the worktree: an agent that is not `idle` or `done`, or a shell with a foreground command.
Each refusal happens before a change. If a `teardown` command fails, the worktree, its panes, its lock, and the item stay, and the error shows the command's last lines of output.
After removal, close drops the worktree's registration, as prune does.
The JSON result is `{pane,worktree:{path,branch,workspace,branch_deleted,anchor_closed?}|null,pane_closed}`. A remote host runs the same steps over SSH.

The service admits tickets in FIFO order using the same placement policy.
The order holds per host. A ticket waits only behind earlier tickets that could take one of its hosts. A full host does not hold back a ticket that is pinned to another host with room.
It records an admission error rather than silently choosing another host after a clone, prepare, or launch failure.
A service interruption during admission leaves an explicit failed ticket for inspection, not an automatic duplicate launch.
For non-waiting callers, admission tells the owner through its inbox. An owner without an inbox reader hears once it has one; nothing is typed into its terminal.
`status` lists open items per owner and pending tickets. `--brief` includes their counts. Prompt context adds the caller's own `To land:` line.

## Inbox

Every message to an agent goes through one inbox on the agent's host: a child's final reply, a blocked report, an admitted ticket, a launch failure, and a message from another agent.

```sh
dogsled send api-dups --message 'Also cover the empty case.'
dogsled send w9:p1 --host netcup --kind task --message-file note.md
dogsled inbox list
```

`send` resolves a name or pane on any host and writes the record there, over SSH for a peer. A target with no inbox reader fails at once, and the sender hears so; a record never waits where nothing reads it.
Records live in `inbox/<terminal>.json`, keyed by Herdr's terminal id, so a record follows the terminal and not a reused pane id.
A reader claims records with `inbox claim`, which is a lease. A claimed record is offered again with backoff until `inbox ack` marks it delivered. When the terminal is gone, the record fails and the sender's own inbox says so.

Readers:

- **Pi:** the extension watches its inbox file, injects each batch as one `dogsled` message (`steer` while the agent runs, a new turn when it is idle), and acknowledges it when Pi has written the message to the session.
- **Claude Code and Codex:** `dogsled hook inbox --harness claude|codex --event Stop|UserPromptSubmit`, registered by the plugin. The first hook run registers the reader. At `Stop`, records continue the turn as `{"decision":"block","reason":…}`; at `UserPromptSubmit`, they go beside the user's text as `additionalContext`. Records printed are acknowledged. At `Stop`, the same response also includes a new reminder about finished children with work to land. It prints nothing when neither is pending.

A hook runs only when its harness acts, so an idle Claude Code or Codex agent sees a record at its next prompt or stop. Pi has no such delay.

`inbox list --json` gives `{pane,terminal,received,sent,lines}`; `lines` is the display form the Pi panel shows.

### Backoff

One schedule is used for inbox leases, the sender's follow-up, peer reads, and launch retries: 10 seconds, doubling with jitter, capped at 5 minutes.
A host that cannot be read shows `retry in 40s` on the board. `dogsled retry HOST` reads it again now.

## Host service and status

Run `dogsled serve` on every host. Dotfiles owns launchd and systemd user-service installation.
The measurement step uses no SSH. It reads CPU busy time, cores, memory, home disk space, Herdr agents, and Herdr panes.
When it has owned items or tickets, the service reads peers through the status cache and can launch admitted work through SSH.
Separate watchdog, admission, child-completion, inbox, and requested-cleanup workers keep slow actions independent from snapshot publication. Routine calls and lock waits have 30-second limits. Setup calls have a 10-minute limit.
An admission timeout records a failed ticket. The service continues publication and later actions.
It writes `~/.local/state/dogsled/host.json` through atomic replacement.
Each `probe.agent_panes` entry contains `pane`, `agent`, `state`, `name`, `transcript`, `transcript_modified`, `terminal_id`, `completion_seq`, `cwd`, the session's model and latest activity, and the worktree's branch, commits ahead, dirty state, and whether it landed.
Transcript paths come from Herdr path sessions. Exact Claude and Codex session IDs also resolve against their standard session directories.
Ambiguous, absent, or unreadable transcripts remain unstalled. Modification times use Unix milliseconds.
Each pane also has `stalled_since_ms` and `state_since_ms`. The latter records the start of the observed state.
The service stops with an error if a measurement fails. The last complete snapshot remains available.

On Linux, the smallest finite `memory.max` from the process cgroup through its ancestors governs memory.
Used memory is that scope's `memory.current` minus `inactive_file` from `memory.stat`.
Without a finite limit, memory comes from `/proc/meminfo`: total minus `MemAvailable`.
With a finite limit, the probe measures both and reports the scope with less free memory.
On macOS, memory comes from `vm_stat` and `sysctl hw.memsize`, and `pressure_level` from `sysctl kern.memorystatus_vm_pressure_level` (1 normal, 2 warn, 4 critical).

CPU busy is the fraction of the last interval that the CPUs were not idle.
On Linux the probe records cumulative `/proc/stat` ticks (`cpu_ticks`); the service derives `busy` from the previous snapshot, so the first tick after start has no sample and admits nothing.
On macOS `busy` comes from a one-second `iostat -c 2 -w 1` window.
`load1` stays in the probe for display. It no longer takes part in admission.

Every tick appends one line to `~/.local/state/dogsled/samples.jsonl`: time, cores, busy, load1, memory, pressure level, disk, reservations, live agents, free cores, and the admission verdict. Lines older than seven days are dropped on append.

`status` reads the local snapshot and each enabled Herdr peer's snapshot.
Peer reads run in parallel through SSH and the configured `remote_command`.
A shared, locked cache permits at most one read per peer per ten seconds, including concurrent callers.
Status never probes peers itself.

`service: up` means a process holds the host's service lock. A free lock or unreachable peer means `service: down`.
Status shows snapshot age separately, including when the service runs. Peers without the liveness field retain the 30-second age fallback during upgrades.
Admission still requires a snapshot no older than 30 seconds. Finished-child cleanup can use an older snapshot from a running service.
Close rechecks the live pane, the child's `done` mark, and the loss guard before it acts.
Each tick observes a shared checkout once. Independent checkout and shell-pane reads use at most one worker per available CPU.
The last peer measurement remains visible when available. Every discovered host stays in the output.
The JSON output contains `hosts`, with service state, reachability, sample time, probe, headroom, errors, and `retry_ms` while a peer read backs off.

```sh
dogsled machine list
dogsled machine rename ID LABEL
dogsled machine disable ID
dogsled machine reconnect ID   # a new tab, for SSH prompts
dogsled machine add user@host --label name
```

Hosts are Herdr's saved machines. `machine` runs the matching `herdr machine` command; `reconnect` and `add` run it in a new focused tab, so the user can answer SSH prompts.

Example tab-row output:

```text
local 11/12 ▲ · netcup 3/16 · momokaya-2 1/7
```

`▲` means not eligible, including a stale snapshot from a running service. `?` means the service is down or unavailable, or no snapshot exists.
A host without measurements prints `momokaya-2 ?`. A stale host keeps its last numbers, followed by ` ?`.
The numbers are busy cores over cores. A host without a CPU sample shows `load N/cores` instead.
`--brief` includes busy/cores, memory, disk, agent counts, and a hint to run `dogsled status`.
Fleets larger than five hosts share lines to preserve both the six-line limit and visibility of every host.
`--watch --interval N` changes the refresh interval. Its default is five seconds.

## Stalls and check-ins

A `working` or `unknown` agent stalls after ten minutes without a transcript change. Screen redraws do not reset this clock.
An agent without a readable transcript never stalls. Recovery clears its `$dogsled` sidebar token.
Status lists stalled panes. Brief status and prompt context include a stall count within six lines.

The owner's service checks every minute. It checks in only with `idle` or `done` owners, never `working` or `blocked` owners.
A check-in is a `check-in` record in the owner's inbox. An owner without an inbox reader gets none; a due check-in stays due until it has one.
A stall or a completed job makes a check-in due. Other open work gets a check-in after 30 minutes without a check-in or lifecycle event. A finished child's reply comes from the sweep, not a check-in.
Transcript writes alone do not postpone regular check-ins. Pending job-completion events survive item closure.
The note includes names, hosts, states, ages, transcript silence, and bounded recent activity.
Known Pi, Claude, and Codex transcripts supply the latest tool call or message. Unknown formats fall back to plain pane text.
Job notes use the last five log lines. All note content has terminal escape codes removed.
It asks the owner to wait, message a child with `dogsled send`, close it, or stop a job. Output in the note is untrusted data.
A stalled top-level pane gets one Herdr notification per stall episode.

Locked `watchdog.json` records events and last check-ins. `parents.json` identifies tracked children on their own host.
A normal service restart does not repeat a delivered check-in. The inbox lease carries delivery from there.
Notification attempts are recorded before delivery and never repeat within that stall; a crash can lose that notification.

For authorized live tests only, `DOGSLED_TEST_STALL_MS` overrides the ten-minute threshold with a positive millisecond value.
It does not shorten check-in cadence. Use private `DOGSLED_STATE` and stop the test service afterward.
Debug binaries also accept `DOGSLED_TEST_NOW_MS` for deterministic integration tests. Release binaries ignore this test clock.

## Cleanup

```sh
dogsled doctor --json
dogsled doctor --host netcup
dogsled prune worktrees jobs
dogsled prune --host netcup --yes panes
dogsled prune targets
```

Doctor changes no candidates. It reports `worktrees`, `branches`, `workspaces`, `panes`, `jobs`, `targets`, `disk`, `processes`, and `repositories`.
Doctor lists linked worktrees on a branch and detached ones.
One loss rule in `src/loss.rs` governs worktrees, detached HEADs, branches, close, and board facts.
A checkout must have no unique tracked or untracked content. Ignored files do not count. A ref without a checkout skips this condition.
Deleted tracked files alone (` D` or `D `) lose nothing when the tip is held. Other changes still prevent removal.
Before removal, dogsled restores only those deleted paths from the held HEAD, including staged deletions. It then removes the checkout without force.
Every commit must be held by the default branch or remote-tracking refs. Each cleanup pass runs `git fetch --all --prune` before remote refs count.
A failed fetch produces a repository warning. Only the default branch counts for that repository. Deleted upstream branches receive no special treatment.
A branch is integrated when it is deleted, when it is an ancestor of Git's remote default branch (local `main` is the fallback), or when commits on that branch apply it.
The branch's commits split into consecutive runs, and each run needs one commit that applies it. Each run uses its net diff, including paths and before/after objects, rather than the union of paths from its commits. Merges from main and reverted edits therefore do not enlarge the whole branch's required path set. A candidate must change every net path. The exact three-way merge into its parent must reproduce its content on those paths.
One run is a merge, squash, or cherry-pick of the whole branch. Several runs are a stack: the branch holds an earlier branch's commits, and each branch landed as its own squash. Git alone decides this.
Each path merges on its own. An addition, deletion, or mode change on a path the base also changed differently is a conflict, and renames are not followed, so a doubtful run is not applied.
A tip also counts as integrated when a local or remote-tracking ref contains it and that ref is integrated. This proof names the containing ref. The search considers only containing refs and checks each directly, without recursive alias loops.
Results use the remote-tracking branch, so `git fetch` keeps them current.
A local branch with no worktree is removable when it is integrated or every commit is on a remote branch. Other branches are report-only. The default branch is never a candidate. Prune deletes a branch with `git branch -D` after it rechecks the planned commit.
Prune and the sweep share the same holds: active panes or processes, the caller, an unreadable process table, and pending preparations or launches.
An unfinished launch holds its checkout before pane allocation completes, or while its attempted start has the matching agent. An empty shell from a final launch holds nothing.
An open agent item holds its checkout in prune and the sweep; `close` ends it and keeps the reply. Within `close`, a live owner holds the checkout until the default branch holds its work.
An idle or done agent that has not called `dogsled done` holds its checkout in prune, the sweep, and an automatic close; only an explicit `dogsled close` takes it. An idle shell, a done agent without an open item, and a workspace alone are not holds. The processes of an idle agent pane are not holds, but a background job that a shell started is. Foreign locks still prevent removal.
A dirty checkout whose commits are in the default branch gets a `landed_dirty` finding when no hold applies.
The finding includes the repository, checkout, branch or HEAD, dirty-path count, and up to ten paths. It is never removable.
A registered worktree whose directory is gone is a stale registration. Prune drops the record only.
Prune and the sweep use one pane rule. Both keep the caller's pane and focused panes.
An empty shell closes when its job ended, its directory disappeared, its launch is final, or its agent item closed.
A final launch was refused, never attempted, or has an unknown outcome with no agent in its terminal. Cleanup preserves launch receipts and checks terminal ownership under the launch locks.
A shell without a Dogsled record closes after 15 minutes continuously at an empty prompt, based on service observations and a fresh check. Herdr reports no activity time, so a command that starts and ends between samples does not count.
An agent pane still needs more than 24 hours continuously idle or done and a `done` mark. An agent with open children stays open.
A workspace whose every pane is idle is one `workspaces` entry, and prune closes it whole. The caller's workspace stays, and so does a workspace that a worktree removal closes.
Ended jobs expire after 24 hours, together with their logs. Disk usage above 90% lists the largest known dogsled-owned paths.
A `targets` entry is build output: a directory with a `CACHEDIR.TAG` that starts with the cache-directory signature, directly under a main checkout or a linked worktree, such as Rust's `target/`.
It is removable when no Herdr workspace shows the checkout and no pane or process works in it. Work in a linked worktree inside the main checkout uses that worktree, not the main checkout.
Prune removes it after it checks the tag again. A worktree that the same plan removes takes its build output with it, so that output is not listed.
Processes above 100% CPU outside Herdr pane ancestry are report-only. Prune never kills processes or frees arbitrary disk paths.

Prune displays a plan and asks before removal. `--yes` skips that question, not safety checks.
Each removal rechecks current state. Working or blocked panes, held worktrees, dirty worktrees, and local-only commits remain untouched.
Removal rechecks Git path, branch, HEAD, and repository against the approved plan. Any identity change stops removal.
Worktree removal is the same as `close`: `teardown`, the panes and workspace, unlock, `git worktree remove`, the branch when its commits are held, and the idle anchor. Dogsled checks that the worktree is gone afterwards.
Each entry is independent. A failed or refused entry does not stop the others; prune reports each failure, then exits with status 2.
`--json` returns `{results,failures}`. A result names the removed path and the closed panes. A failed `teardown` command's error includes its last lines of output.

Placement records repositories in `repositories.json`, independently of `worktrees.json`. Repository registrations survive the last worktree removal.
Doctor also discovers repositories from the caller, live pane directories, Herdr workspaces, and `cleanup.repositories`. It never scans the whole home directory.
A missing or unreadable repository produces a report-only `repositories` warning. The warning names its registration or configuration, and the survey continues.
Repository registrations stay unchanged.
### The sweep in `serve`

The sweep closes finished children (see [Items and closing](#items-and-closing)), and a job closes its own tab. That closed loop ends most work.
The sweep catches what a broken loop leaves behind: a crashed owner, a killed process, or a worktree that something else made.
The Stop gate writes `sweep-request.json` and wakes the local service through `sweep.sock`. It never runs the survey inside the hook.
The service checks the durable request at startup, on notification, and on each publication iteration. No request means no survey.
The socket is optional. A bind failure produces a warning, and pending requests still reach the worker on the publication interval.
A Stop request failure produces a warning without preventing existing gate or inbox messages from reaching the agent.
Each requested pass builds the doctor plan in sweep scope. It applies only entries whose removal loses nothing:

- `panes` and `workspaces`: the shared pane rule above. A workspace closes whole only when every pane qualifies. Final launch panes close separately under their launch locks.
- `worktrees` and `branches`: the shared loss rule and holds described above, identical to prune.
- Orphaned children: a child that called `done`, whose owner is gone, and whose checkout passes the shared loss rule. Close runs teardown and removes its workspace, checkout, and item.

The sweep runs the same checks as `close` and `prune`. It never closes the service's own pane or workspace, or a worktree that the service works in. It skips a worktree it cannot check, and a failure on one entry does not stop the others.
The service keeps seven days of completed passes in `sweeps.jsonl` and the latest pass in `sweep.json`.
Each history entry has a `ts_ms` timestamp; each new pass removes entries older than seven days.
The report includes start and finish times, removals, failures, repository warnings, and landed-dirty findings.
`dogsled status` and `status --json` show the latest pass. Prompt context adds one summary line when that pass has errors or warnings.
The service also writes removals, failures, and repository warnings to stderr.
Child completion and orphan checks remain independent of requested doctor cleanup.
An orphan that is dirty, unpushed, `working`, `blocked`, or on an unreachable host stays open. Status shows `orphaned:` and the reason on its item line, the brief counts it, and every check-in on the host lists it.
Prune also handles ended jobs, stale registrations, and `targets`. Disk, process, and repository findings are report-only.

## Placement

Headroom is free cores, the smaller of the measured and the declared value:

```text
measured = cores × (1 − busy) − reservation_load × reservations
declared = cores − reservation_load × (active agents + reservations)
```

An active agent is a live agent that is not `idle`.
It holds its demand while it waits on its model, because its next build starts within minutes.
The measured value counts work that dogsled does not know about.

A host is eligible when all of these hold:

- a fresh service snapshot with a CPU sample;
- at least `disk_free_min_percent` free on the home filesystem;
- on macOS, memory pressure below critical;
- free memory of at least `memory_reserve_bytes × (reservations + 1)`;
- total memory of at least `memory_reserve_bytes × (active agents + reservations + 1)`;
- free cores of at least `free_core_fraction_min × cores`.

The host with most free cores wins. Ties use fewer live agents, then host id.
The reasons for each refusal appear in `status --json` under `headroom.reason`.
The thresholds come from the fleet measurements in `docs/research/2026-10-15-admission-metric.md`.
The selected host rechecks reservations under its own lock before it accepts the placement.

`place` writes a reservation with a default TTL of 90 seconds.
It returns the host id and label. JSON also includes the reservation id.
With `--branch`, it creates or reuses the branch's worktree on that host and returns its `path` and `pane_id` (see [Repository configuration](#repository-configuration)).
`place` does not start an agent. `run` and `agent` perform placement and launch together.

A reservation ends only at its TTL, which covers the time until the new agent appears in `herdr agent list`.
Until the TTL ends, the new agent counts twice: as its reservation and as an active agent.
Agent discovery and worktree failures do not end it early.

## Configuration

The optional host policy lives in `~/.config/dogsled/config.toml`.
`--config PATH` or `$DOGSLED_CONFIG` selects another file.
Unknown keys are errors.

```toml
free_core_fraction_min = 0.25
memory_reserve_bytes = 3221225472
disk_free_min_percent = 15
reservation_load = 1.0
reservation_ttl_ms = 90000
remote_command = ["mise", "x", "--", "dogsled"]

[cleanup]
repositories = []  # for example "~/.config/mise"; `~/` is the home directory
```

Host policy has no worktree hooks. A `[hooks]` table is an unknown key, so it is an error. Worktree setup and teardown belong to each repository.

### Quota snapshots

```toml
[quota]
management_url = "https://cliproxy-admin.tail10e7b9.ts.net"
```

The URL is an HTTPS origin, without a path or credentials. No management key belongs in configuration.
Only macOS collects quota, and only when this Keychain lookup succeeds:

```sh
security find-generic-password -s dogsled -a cliproxy-management -w
```

This command prints the key. Dogsled captures it internally and never logs it.
Create the generic-password item through Keychain Access, with service `dogsled` and account `cliproxy-management`.
The collector passes the key to curl through stdin, never command arguments or a file.
Linux hosts never read the key. A missing URL or unreadable Keychain item makes quota commands use the local cache.

```sh
dogsled quota
dogsled quota --json
dogsled quota --json --cached
dogsled quota --refresh
```

`--cached` reads only `$DOGSLED_STATE/quota.json`, even when it is older than 15 minutes.
Without `--cached`, the collector refreshes snapshots older than five minutes. `--refresh` forces collection.
The service also refreshes every five minutes. A shared lock prevents concurrent collection, and failed collection backs off.
Each successful collection pushes the snapshot to enabled Herdr peers over SSH. Failed pushes retry with backoff.
Peers need a dogsled version that supports quota storage. No management credential leaves the collector.

The version-1 contract contains `version`, `fetched_ms`, `source`, and `pools`.
`source` identifies the collector by hostname. Each row contains these fields:

- `pool`: `claude`, `codex`, or `minimax`.
- `account` and `window`: the account label and provider window name.
- `used_pct`: a number from 0 to 100.
- `resets_ms`: Unix milliseconds, or `null` when unavailable.
- `error`: a safe error description, or `null`.

Claude windows are `five_hour`, `seven_day`, `seven_day_opus`, and `seven_day_sonnet`.
Codex windows are `primary` and `secondary`. MiniMax windows are `interval` and `weekly`.
MiniMax account labels use `minimax-N`, where N is the position in the configured API-key list.
A failed account has error rows with zero usage and no reset time. Consumers must treat these rows as unknown, not available quota.
A pool is exhausted when every account has a window at 100% with a reset time in the future.
Human output shows each window, reset time, and snapshot age. No snapshot produces an empty version-1 snapshot with `fetched_ms: 0`.

The hidden `quota --store` command accepts one validated snapshot on stdin and writes it atomically.
Older pushed snapshots cannot replace newer snapshots. The SSH transport uses the same writer through `host quota-store`.

### Repository configuration

An optional `dogsled.toml` at the root of the repository's main checkout configures its worktrees. Without the file, the defaults apply.
Unknown keys are errors.

```toml
[worktree]
dir = ".worktrees"  # relative to the main checkout
setup = []          # run in a new worktree after creation, in order
teardown = []       # run in a worktree before removal, in order
```

Each command runs with `sh -c` in the worktree. `{main}` is the main checkout, `{path}` the worktree, and `{branch}` its branch (empty for a detached worktree).
Dogsled quotes each value as one shell word. Any other `{…}` is an error, so the command text cannot use braces for anything else.
Dogsled reads `dogsled.toml` from the main checkout, also for a worktree on an older branch.

With `--branch`, `place`, `agent`, and `run` use the branch's worktree on the selected host:

1. Dogsled refuses an invalid branch name, `--base` for a branch that exists, a branch checked out in the main checkout, and a worktree that has another lock. It needs a reachable Herdr server.
2. A branch that has a worktree reuses it without `setup`. Otherwise `git worktree add` makes `<dir>/<branch>` with each `/` as `-`. A new branch starts from `--base`, else the remote default branch's local name (else `main`).
3. The worktree is locked with the reason `agent:worktree`. The lock marks it as agent-owned, and `git worktree remove` refuses it until close or prune unlocks it.
4. `setup` runs in a new worktree. If a command fails, dogsled removes the new worktree and the branch it made, and placement fails. The reservation stays.
5. `herdr worktree open --no-focus` opens the worktree as a Herdr workspace linked to the repository. When the main checkout had no open workspace, Herdr opens one as the anchor, and dogsled records its id in the Git common directory as `agent-worktree-anchor`. A workspace that already showed the worktree may belong to a running child, so that is an error.

Before repository work, dogsled verifies the selected host's checkout against the caller's `git remote get-url origin`.
If the host has no checkout of the repository, dogsled clones it from the caller's origin into the same path relative to home first.
An existing path with a different origin, or a non-repository path, is an error. Dogsled does not overwrite it.
Clone errors identify the host and origin. Dogsled does not silently move the work elsewhere.
After a fresh clone, dogsled checks mise configuration. If mise reports missing trust, dogsled trusts only that new checkout's configuration.
For peers, a checkout under the local home maps to the same relative path under the peer's home.
The peer reads its own host policy.
Checkouts outside the home keep their absolute path.
Hooks substitute `{branch}` and `{base}` literally, without interpreting placeholders inside their values.
When `--base` is absent, a word that is only `{base}` is omitted. Other placeholders produce an error that names them.
Prefer an argument list over shell interpolation.

The hook must print JSON with `pane_id`, `path`, or both. `workspace_id` is optional.
Progress text can precede a final JSON line.

State defaults to `$XDG_STATE_HOME/dogsled`, or `~/.local/state/dogsled`.
`$DOGSLED_STATE` selects a complete state-directory path.
Host snapshots use `host.json`. New placement reservations use `placements.json`.
Jobs use `jobs/`, owner items use `items.json`, and FIFO tickets use `queue.json`.
The removed lead ledger and event log are not read or migrated.

## Prompt context and integrations

`dogsled hook context --harness claude|codex|pi` leads with your own children and jobs, then shows the fleet totals from `status --brief`.
The owner summary separates work to land, attention, idle, done, running, and queued work.
The `To land:` line names finished children with unmerged commits or uncommitted changes.
Claude Code and Codex register `SessionStart` and `UserPromptSubmit` context hooks, and `UserPromptSubmit` and `Stop` inbox hooks (see [Inbox](#inbox)), in `plugins/dogsled`.
Each registration supplies `--event`. The default context event is `UserPromptSubmit`.
The CLI never reads stdin. Claude Code and Codex receive `hookSpecificOutput` JSON with `hookEventName` and `additionalContext`.
Pi receives plain text through its `before_agent_start` extension. All three use the same snapshot reader and formatter.

Before an orchestrator stops, the stop gate checks its own TO LAND children.
Pi calls `dogsled hook gate` from `agent_before_settle` on every completed stop, which also requests a cleanup pass. A reminder is added as a visible message that continues the run.
Claude Code and Codex include the reminder in their existing `Stop` inbox response.
The reminder requests a review, a commit from dirty children, integration, or an explicit blocker or deferral.
It never commits, merges, closes, or discards work itself. Clean readers and working children do not trigger it.

The gate reports failures, repository warnings, and landed-dirty findings from the latest completed pass, without waiting for the requested pass.
Successful removals remain in status and the journal; they do not continue an agent turn.
Repository warnings name the repository and state entries, and request delegation to a dogsled agent instead of manual state edits.
Landed-dirty findings reach only panes in the same main repository, including linked worktrees and symlink paths.
The message asks the owner to read and preserve wanted changes, or obtain user approval before discarding abandoned work.
It forbids forced worktree removal, `git clean`, and `rm -rf` on the checkout.
Cleanup reminders use separate, once-per-set histories in `gate.json`, so an unchanged warning cannot cause a continuation loop.

The gate records each set of child item IDs, commit counts, and dirty states in `gate.json`.
It continues once per distinct set, across process restarts. Row order and idle/done transitions do not cause another reminder.
New states can trigger another reminder. Closing all owner items clears the history.
Pi skips the gate on errors, aborts, and boundaries that already request continuation.
This is a bounded reminder, not a permanent prohibition on stopping.

Without `HERDR_ENV`, service snapshots, or a usable binary, the hooks add nothing.
A hook has a five-second command budget. Hook failures exit successfully without output.
Plugin commands end with `|| true`. Pi also leaves the prompt unchanged if the binary fails.

### Tools

`dogsled mcp` serves the dogsled commands as MCP tools over stdio: `status`, `spawn`, `run`, `send`, `inbox`, `read`, `close`, `done`, `stop`, `cancel`, and `place`.
Each tool calls the same code as its CLI command and returns the same JSON. No tool waits: `spawn` and `run` return the item or the queued ticket at once.
The caller is the harness's `HERDR_PANE_ID`. Pi registers the server from its extension (tools `mcp__dogsled__*`); Claude Code reads `plugins/dogsled/.mcp.json`; Codex reads `plugins/dogsled/codex-mcp.json`, which forwards the Herdr and `DOGSLED_*` variables.

### Pi

The extension adds host status before each prompt, registers the tools, and draws:

- **The right-side board** shows cached quotas, agents, jobs, and todos when `HERDR_ENV=1` and `HERDR_SOCKET_PATH` accepts a connection. Pi opens or reuses the tab’s split and sets no above-editor widget. Without Herdr, the existing widget shows host meters, the current task, and work groups. Agent/job data refreshes every ten seconds. Compatible pi-todo snapshots update immediately in the widget.
- **Alt+W or `/work`** opens or reuses the Herdr split. Without Herdr, it expands or collapses the widget in place, without a popup or editor replacement. Alt+J/K moves selection, Alt+] changes views, and Alt+I shows inline details. Typing stays in the editor. Alt+S stops jobs, Alt+C cancels queued work, and Alt+M sends agent messages. Stop and cancel require confirmation. Completed job history starts collapsed.
- **`/dogsled`**, the control panel, with four tabs: Status (hosts with their retry state, then your rows; Enter on a host reads it again now), Hosts (reconnect, rename, enable, disable, add), Profiles (Enter edits `prompt.md`), and Inbox. The widget steps aside while the panel is open.
- **The inbox reader** and a renderer that shows each record's kind, sender, and text, folded past twelve lines.

Without `HERDR_PANE_ID`, the extension starts no Dogsled polling or jobs. The shared display can still show todos and reports Dogsled unavailable. `$DOGSLED_BIN` selects the binary.

Todos follow the active Pi session branch. Agents and jobs follow the calling Herdr owner pane, including after a Pi session change. `/todos` and Alt+T open the Todos view when compatible pi-todo is active. `/todos list` remains a plain readout. The [display contract](docs/pi-work.md) keeps task state and reminders in pi-todo. Dogsled retains execution, logs, cancellation, and completion delivery.

The Jobs view joins retained records to the current owner's ledger by host and job ID. It does not show unrelated fleet jobs. Job inspection uses `dogsled read` for a bounded log tail and shows the `dogsled logs` command for full output. Expired records and unavailable hosts produce explicit errors. The panel never starts a new process manager, edits todos, or writes into an agent terminal.

The Herdr plugin opens the right-side work board:

```sh
dogsled board --open                     # open or reuse the caller’s tab split
dogsled board --side --owner "$HERDR_PANE_ID" # print once
herdr plugin pane open --plugin dogsled --entrypoint board --direction right --no-focus
```

The split runs `dogsled board --side --watch` and refreshes every ten seconds. `--owner` defaults to the opening pane from Herdr’s plugin context.
Quotas come only from `quota.json`; the board never calls a provider or reads credentials. The heading shows cache age and `STALE` after fifteen minutes.

Pi renders each new todo revision through pi-todo’s `display.createView(host).render(width)`. Dogsled never reads or copies task state.
The board publishes its width in the ledger; before that, Pi uses 48 columns. Resizing republishes the same provider view at the new width.
Pi atomically writes `{session, branch, revision, current, pane, width, lines}` to `todo-lines/<pane-hash>.json` in Dogsled’s state directory.
The service reads files for live agent panes into `todos.json`. Peers expose this presentation through `dogsled host todos --json`.
The board shows each agent’s current line and prints the owner’s full provider-rendered lines unchanged, including ANSI styles. Missing presentations show as unavailable.
Session shutdown and provider release remove the pane’s publication. `/dogsled` remains the interactive control panel in both display modes.

Herdr 0.9.5 creates a new pane on every plugin open. Dogsled keeps a small `boards/<tab-hash>.json` registry keyed by Herdr tab ID.
It stores the board pane and terminal IDs, verifies them against live panes before reuse, and removes a stale entry before opening another split.
All three harness packages share `plugins/dogsled/skills/dogsled/SKILL.md`. It describes delegation, jobs, messages, and the queue.

## Build and development

Build this branch with the repository's pinned Rust toolchain:

```sh
cargo build --release
npm ci
```

The executable is `target/release/dogsled`.
Release archives include the binary, `dogsled.service`, and `dev.dogsled.plist` from `plugin/services/`.
Harness installers read their manifests and skill from Git, not the binary archive. The skill move does not change the archive contents.
Both run `dogsled serve` through `~/.local/bin/mise`, restart on failure, and log to the dogsled state directory.
Dotfiles installs the systemd user unit or renders the launchd template with an absolute, XML-escaped `@HOME@` path.
For launchd, dotfiles creates `~/.local/state/dogsled` before loading the agent. Systemd creates it through `StateDirectory`.
If mise has a different installation path, dotfiles adjusts the executable path.
Install the same release on every host before enabling its service. This PR does not install or start host services.

The pre-commit hook runs `mise run check`: Rust format, Clippy, Rust tests, TypeScript typecheck, and Pi extension tests.
CLI behavior tests use private state and fake Herdr/SSH executables. Time-sensitive unit tests inject timestamps.
See [the design](docs/design.md) for watchdog delivery guarantees and cleanup scope.
