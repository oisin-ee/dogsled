# Plan: dogsled closes everything you asked it to close

Status: ready for review. All decisions are answered.

## Context

Since 2026-10-19 you have asked for one result: when work is finished, dogsled removes what is left over. That means panes, worktrees, branches, and workspaces, on local, netcup, and momokaya-2. Many fixes merged, but the hosts still fill up. The ledger below lists each request and its true state today. Each claim was checked against the code at `d79af2a` (0.16.4) and against live state.

### Requirement ledger

| # | What you asked (session) | State today | Evidence |
|---|---|---|---|
| R1 | At the end of a turn, prune empty panes, merged worktrees, and cruft (10-19: "1, 2, and all of 3") | **Broken on every Pi agent.** The Pi gate never runs. | `pi/index.ts:254` skips when `canContinue` is false. Pi 1.3.0 sets it false on every normal stop. Local had 7 passes, all on Oct 20. netcup and momokaya-2 have none. |
| R1-3c | Close a plain shell at an empty prompt in an agent worktree whose child is closed | **Never built.** | `cleanup.rs:928 idle_panes`: in `Scope::Sweep` it closes only shells whose job ended or whose cwd is gone. |
| R2 | Delete local branches whose remote branch is gone, when nothing is lost | Done (d8818c5). It also depends on R1 to run. | `loss.rs` |
| R3 | One definition of "lost" for worktrees, branches, and close | Done (d8818c5). **Gap:** prune and sweep use different *pane* rules. Prune has an "idle for a day" rule, but the sweep does not. | `cleanup.rs:969` returns early for Sweep |
| R4 | Warn agents in the same repo about landed worktrees that are dirty, with cleanup steps | Done (`gate.rs`). It also depends on R1. | |
| R5 | No 10 s cleanup loop. The Stop gate starts the sweep, surfaces errors, and treats missing repos as warnings that do not stop the pass | Done in Rust (`service.rs`, `gate.rs`). **Pi never triggers it** (see R1). | |
| R6 | "What is this 30 second deadline?" `git worktree remove` is killed halfway and leaves half-deleted checkouts | **Fix written but never committed.** It is staged in `.worktrees/fix-remove-setup-limit`. Today's pass failed on 3 rondo removals for this reason. 7 half-deleted checkouts (≈1715 deleted files each) are now stuck as "landed + dirty". | `sweep.json` failures, `src/worktree.rs:316` |
| R7 | Show the age instead of "down" | Done (#65). **Gap:** `finish()` and `settle()` still skip any host marked down. Local ticks take up to 54 s against a 30 s threshold. Status shows local "service: down" right now while the service runs. | `sweep.rs:269,429`, `node.rs:11` |
| R8 | Admission defect: "fix it" | Done (#63) | |
| R9 | Close only children that have FINISHED, never after a question or while orchestrating | Done (#66, the `done` signal). **Must stay true** in every change below. | `done.rs` |
| R10 | A Herdr refusal is final, close the pane dogsled created, and a gone pane holds nothing (handoff "1&2") | **Partial.** `refused()` closes only panes dogsled allocated, not prepared ones. Records that were never attempted keep their worktree held while the shell pane exists, so the hold keeps the pane and the pane keeps the hold. | `launch.rs:117-123, 56-85` |
| R11 | Work in a repo goes in that repo's workspace | Done (#68) | |
| R12 | Do not stop until cleanup is really finished (10-20) | This plan ends with live counts on all 3 hosts, not with a release. | |
| R13 | Prune local "and the others" (10-21) | One manual pass ran on local only (11:20). netcup and momokaya-2 have never had a pass. | `sweep.json` |
| R14 | Empty panes still open on all 3 hosts (today) | Caused by R1 + R1-3c + R10 + R7 | Agentless panes: local 21, netcup 47, momokaya-2 13 |
| R15 | Squash merges: "nothing ever gets killed" in rondo | **Real bug.** `integration::runs()` throws away the squash commit because of the paths that merges *from* main touched. Those paths are not branch work. | `integration.rs:366-373`. 4 rondo branches checked: the merge-tree of the squash parent equals the squash tree. |
| R15b | typed-docs | Two things. (1) Its child never called `done` and its owner is alive, so R9 keeps it, as intended. (2) It was squashed inside #1836 as an ancestor of `typed-region-a`. No rule sees that, so the board keeps showing "+79 to land". | |

Out of scope (done elsewhere, or not dogsled): sccache service (dotfiles), the pay-respects zsh storm (dotfiles), the rondo `.structure-probe` test (removed in rondo `50dc3ecaea`), and the `.agent-logs` deletion (done).

## Approach

Fix each owning cause in the module that owns it. No new flags. No weaker checks. R9 stays: a pane that runs an *agent* closes only after `done` or an explicit `dogsled close`. Every new rule below is about panes that run **no agent**, or about branches whose content is already on main.

1. **Pi gate (R1, R5, R13).** Request the sweep and run `hook gate` on every completed stop. Use `canContinue` only to decide whether to return `continue: true` with the reminder. When the turn cannot continue, still run the gate so the sweep gets requested and the notice is saved, but do not ask Pi to continue.
2. **One pane rule for prune and sweep (R1-3c, R3, R10).** Remove the `Scope::Sweep` early return from `idle_panes`, so both scopes use one rule. An agentless pane is idle when its shell sits at an empty prompt (`probe::foreground == None`) and one of these is true:
   - its job ended;
   - its cwd is gone;
   - its launch record is final: refused, never attempted, or outcome unknown with no agent in the pane (see D2);
   - the agent item for that pane or checkout is closed;
   - for a shell with no dogsled record (D1): it is not focused, and it has sat at an empty prompt for longer than `IDLE_SHELL` = 15 minutes (`watchdog::observe` already tracks `idle_since_ms`). The same constant replaces `DAY` in the prune path, so prune and sweep use one rule.
   The caller's own pane and the focused pane stay excluded. Closing the pane changes no receipt. The worktree then goes through the existing loss rule.
3. **Launch finality (R10).**
   - `refused()` closes the refused pane whether dogsled allocated it or prepared it. The terminal-identity and agent checks stay as they are.
   - `pending_paths` holds a path only while a launch is really in flight. "The pane exists" is no longer a reason. A record is in flight only when no receipt exists and either no pane has been made yet, or the start was attempted and the pane runs a matching agent. The existing `recovery::close_unfinished` logic becomes the shared check, so the explicit close and the sweep use one rule.
4. **Worktree removal (R6, D3).**
   - Commit the staged branch: `git worktree remove` gets `SETUP_LIMIT`, the same limit as teardown.
   - In `loss.rs`, a dirty path that is only a deleted tracked file does not count as lost work when HEAD is held. Git still has the content at HEAD.
   - Any modified, added, or untracked path still counts.
   - The 7 half-deleted rondo checkouts then go through the normal sweep, and an interrupted removal in the future finishes on the next pass.
5. **Freshness (R7).**
   - Split "service down" from "snapshot old". On the local host, "down" means that no process holds the `service` lock. On a peer, it means the host is unreachable or its service is not running.
   - Snapshot age stays a separate fact, shown as an age.
   - `finish()` and `settle()` act on the latest snapshot of a running service. They can do this safely because `close` checks the live pane, the `done` mark, and the guard again before it acts.
   - Find out why a tick takes 28–68 s, and fix the slow step in `facts::observe` or the probe. Do not raise `STALE_MS`.
6. **Squash detection (R15, R15b).** In `integration::check_ref`, find the branch's own work as the net diff from the fork to the tip, not as the union of paths in each commit. Merges from main are then not counted as branch work. A candidate on main must change every path in that net diff, and `applies()` keeps its exact content check. If any path is in doubt, the branch is kept. D5 adds a rule for branches that were squashed inside a later branch's merge.

## Files to modify

- `pi/index.ts`, `pi/index.test.ts`: U1
- `src/cleanup.rs` (`idle_panes`, `held`), `src/launch.rs` (`refused`, `pending_paths`), `src/launch/recovery.rs`, plus tests in `src/launch/pane_tests.rs`, `src/launch/recovery_tests.rs`, `src/loss_tests.rs`: U2
- `src/worktree.rs`, `src/prepare.rs`, `src/shell.rs` (the staged branch), `src/loss.rs` (D3): U3
- `src/node.rs`, `src/hosts.rs`, `src/sweep.rs`, `src/facts.rs`/`src/probe.rs` (slow step): U4
- `src/integration.rs` (+ tests): U5
- In every unit: README "Items and closing" and cleanup sections, `docs/design.md`, CHANGELOG

## Reuse

- `crate::probe::foreground`, `probe::agent_list`, `launch::terminal`/`idle` (identity and empty-prompt checks)
- `launch::recovery::{candidates, unfinished, close_unfinished}`: becomes the shared "launch is final" check
- `done::finished`: the R9 boundary, unchanged
- `loss::Evidence::assess` / `Assessment::landed`: the one loss rule
- `integration::{changes, applies, merge_tree_unchanged}`: the content checks stay; only candidate selection changes
- `budget::SETUP_LIMIT`, `shell::run_setup`: already used by teardown

## Units (delegation)

| Unit | Owns | Depends on | Acceptance |
|---|---|---|---|
| U1 Pi gate on every stop | `pi/` | none | `pi` test: a stop with `canContinue:false` calls `hook gate` and returns no `continue`; a stop with `canContinue:true` continues as before |
| U2 Pane rule + launch finality | `src/cleanup.rs` panes, `src/launch*`, `src/watchdog.rs` constant | none | Tests: a prepared pane with a refusal closes; a record never attempted releases its hold, and its shell closes in the sweep; an unfocused shell with no record closes after 15 min but not at 14; a focused shell is kept; prune and sweep list the same panes; an idle agent without `done` is still kept |
| U3 Removal limit + half-deleted checkouts | `src/worktree.rs`, `src/prepare.rs`, `src/shell.rs`, (`src/loss.rs`) | none | Existing staged test + a D3 test |
| U4 Freshness | `src/node.rs`, `src/hosts.rs`, `src/sweep.rs` | none | Tests: an old snapshot from a running service still lets `finish()` close a child that is done; a host whose service is gone is "down"; the tick time is measured before and after |
| U5 Squash detection | `src/integration.rs` | none | Fixture: a branch that merges main into itself and is then squash-merged is `Applied`; a fixture with one changed line not on main is kept |

All five are independent and run in parallel. Then one release, rolled out to 3 hosts.

## Steps

- [x] D1–D5 answered
- [ ] Spawn U1–U5 as writer children (U1 is small; I do it myself)
- [ ] Review each PR. Merge each one when CI is green.
- [ ] Release, then install the service + Pi/Claude/Codex plugins on local, netcup, momokaya-2
- [ ] Run one sweep on each host. Discard the `.structure-probe-*` folders (D4) in `demo-ready` and `fix-e2e-dev-dock` by hand. These are untracked, so they still count as dirty under D3. Then let the sweep remove those checkouts.
- [ ] Verify (below). Report the counts per host.

## Verification

- Unit/CLI tests per unit. At the end, one full `check:rust` + `pi` test at the final revision.
- Live, on each host after rollout:
  - one Pi agent stop writes `sweep-request.json`, and `sweeps.jsonl` gets a pass;
  - the classifier (`/tmp/classify-empty.sh`, outside the repo) shows 0 panes in the classes that should close;
  - `sweep.json` shows no `deadline reached` failures;
  - `dogsled doctor` in rondo marks `feat/event-tasks-ui`, `chore/adopt-dev-stack`, `feat/home-hero-tasks`, and `test/native-player-maestro-flows` as integrated;
  - the local status shows the service "up" while it runs.
- R9 regression check: an orchestrator waiting for children, and a child that asked a question, both stay open.

## Decisions (2026-10-21)

- **D1** Close agentless shells with no record quickly: "I do NOT want random cruft in my workspace." The threshold is 15 minutes unfocused at an empty prompt. It applies in both prune and the sweep.
- **D2** Yes: the sweep closes the empty pane of a launch that was never attempted, or whose outcome is unknown and whose pane has no agent. The receipt stays.
- **D3** Yes: deleting a tracked file loses nothing when HEAD is held.
- **D4** Yes: discard the `.structure-probe-*` leftovers.
- **D5** Yes: a tip that is an ancestor of an integrated ref counts as landed. The candidate refs are local branches and remote-tracking refs. `integration::check_ref` already proves the integration.
- **Execution:** run the 5 units in parallel, cut one release, roll out to all 3 hosts, and verify live.
