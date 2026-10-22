# Executor calibration: Sol, Sonnet, Luna, MiniMax

Status: proposal. Measured on 2026-10-21. This is step 2 ("Calibration") of
[agent-workforce.md](agent-workforce.md).

## Question

- Which order should `tier/exec-gpt`, `tier/exec-claude`, and `tier/mech` use?
- Should MiniMax M3.1 Flash stay in `tier/mech` only, or move up?

Policy: expensive models think, cheap models type.

## Method

The method follows the `benchmark` skill: accepted behaviour first, then the same workload on each candidate, with failures counted.

### Units

Six real units that merged on 2026-10-21. Each is a single-session unit. Each has a base commit and the brief (or, where no brief exists, a brief written from the PR's defect statement).

| Unit | PR | Base | Kind | Brief source |
| --- | --- | --- | --- | --- |
| O24 | oisinpi #24, skip unlisted tier targets | `02575b9` | small, some judgment | PR description, rewritten as a defect brief |
| O26 | oisinpi #26, case-insensitive targets and pools | `a260f6b` | mechanical | PR description, rewritten as a defect brief |
| O28 | oisinpi #28, 5xx and `empty_stream` are transient | `5845ac3` | mechanical | PR description, rewritten as a defect brief |
| D102 | dogsled #102, generic CLIProxy `quota/fetch` | `4a28ccc` | large, fully specified | original dogsled launch brief |
| D103 | dogsled #103, reconcile a vanished planned worktree | `4a28ccc` | small diagnosis | original dogsled launch brief |
| D107 | dogsled #107, hold a worktree that Git pruned | `2c2ed17` | large diagnosis and design | original dogsled launch brief |

Changes from the candidate list in the task:
- dogsled #104 is replaced by #107. No brief for #104 exists on any host. The tova brief that the candidate list linked to #104 produced #107.
- dogsled #98 is not run. Its brief has two causes in one unit, and the quota did not allow a seventh unit (see "Quota").
- The cliproxy-quota stream fix (#9) merged during the run. It is a Go plugin in a third repository, so it was not added.

Every brief keeps its text. Only the delivery lines change (commit, push, PR, reply to the lead, `done`), because a replay must not publish. The writer prompt replaces them with one rule (see "Writer rules").

### Runs

- One run per unit and physical model: `cliproxyapi/gpt-6.1-sol`, `cliproxyapi/claude-sonnet-5-5`, `cliproxyapi/gpt-6-luna`, `cliproxyapi/MiniMax-M3.1-Flash`.
- MiniMax ran last, one unit at a time, after its stream fix (see "MiniMax availability"). At 18:14 UTC the lead cut its scope: MiniMax is mechanical-only by policy, so D102, D103, and D107 are "not run: out of MiniMax's role". O28 had finished and is kept. At 18:22 the lead stopped O26 as unfinished and skipped O24.
- No tier and no fallback. A run that fails stays failed.
- Thinking `medium` for every writer.
- Each run is one dogsled `run` job (`dogsled run --timeout 100m`), so dogsled chose the host with room. The harness is in the appendix.
- The writer gets the brief only. There is no plan-model prefix. Real `writer-*` work gets a plan and a first edit from `tier/plan-*` before prewalk, so these runs are harder than real executor work.
- No model ran twice at the same time. Different models ran in parallel.

### Isolation

- Each run gets a fresh checkout at the base commit: `git init`, then `git fetch <mirror> <base-sha>`. Only the base's ancestors exist in the checkout. The merged fix is not reachable, and the checkout has no remote.
- Hindsight memory is off for the writer and the reviewer (`HINDSIGHT_CONFIG` points to `{"disabled": true}`). Memory holds the merged fixes, and replays must not write to it.
- The writer prompt forbids reading GitHub PRs, issues, and other checkouts. The grader scans each writer's tool calls for `gh pr`, `origin/`, `~/dev/<repo>/`, and the installed `@oisin-ee` packages.
- Each run has its own `CARGO_TARGET_DIR`. The harness deletes the checkout, the target directory, and the output after it emits the artifacts.

### Writer rules

Every model gets the same appended system prompt: the shared `writer.md` light-check rules, plus this replay rule:

> This checkout is a local replay with no remote. Do not commit, push, open a pull request, call `dogsled`, or message anyone. Leave your finished change uncommitted in this checkout. Work only from this checkout. Do not read GitHub pull requests, issues, or other checkouts of this repository.

Global `AGENTS.md`, the rules, and all Pi extensions except Hindsight load as they do for a real writer.

### Grading

Each run is graded on four separate measures:

1. **Light checks** by the grader after the writer stops.
   - oisinpi: `oxfmt --check` on changed files, `oxlint --deny-warnings` and `tsc --noEmit` in `packages/pi-tiers`, `vitest run`.
   - dogsled: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`.
2. **Hidden acceptance tests** that the writer never saw.
   - O24, O26, O28: the merged PR's `router.test.ts`, `policy.test.ts`, and test harness `fixture.ts`, copied over the writer's tree. They test the router through Pi's public context.
   - D107: one CLI test written for this calibration (appendix). It uses the real binary and real Git: the pass must not fail, must not delete the directory, must name the directory in `status`, and must drop the registration after the owner deletes the directory. It fails at the base `2c2ed17` and passes on the merged fix `1876f27` (run locally before any model ran, and again as the D107 control).
   - D102, D103: the merged tests call private functions, so they do not test public behaviour. These units are graded on the brief's own acceptance list, by reading the diff.
3. **Blind review**: one `tier/review` pass per run. The reviewer gets the brief, the light-check exit codes, and the checkout. It does not get the model name. It ends with `VERDICT: accept | minor-rework | major-rework | reject`.
   - Control: the same review runs on each merged reference diff. A finding that the reviewer also raises on the reference is not counted against a model.
4. **Cost**: elapsed writer time, tokens, and Pi's cost estimate from the session file, plus any human intervention. There was one: the lead stopped O26-minimax.

A run is **accepted** when the light checks pass, the hidden tests pass (where they exist), and the grader finds no defect against the brief beyond those on the reference. Fast invalid output is not accepted work.

### Environment

| Item | Value |
| --- | --- |
| Date | 2026-10-21, 17:23–18:22 UTC |
| Hosts (chosen by dogsled) | netcup (`local`, 16 cores, 62 GiB): 19 runs. momokaya-2 (7 cores, 8 GiB): 7 runs. The Mac had no room and ran none. |
| Pi | 1.3.0, `@router-for-me/pi-cliproxyapi-provider` 1.4.22, `@oisin-ee/oisinpi` 0.9.2 then 0.9.3, `@oisin-ee/pi-dogsled` 0.20.0 |
| Gateway | CLIProxy at `cliproxy.momokaya.ee`; MiniMax through the cliproxy-quota plugin 0.1.2 |
| dogsled | 0.20.0 |
| Toolchains | rustc and cargo 1.99.0, node 24.21.0, pnpm 12.14.0, git 2.54.0, Linux 6.18 |
| Thinking | `medium` for writers and reviewers |
| Cost column | Pi's list-price estimate from the session file. Traffic is on subscriptions, so this is a relative measure, not a bill. |

### Quota rule

Quota is shared with live work. The grader read `dogsled quota` before each batch.
A pool's level is the lowest-used account's highest window, because CLIProxy rotates accounts.
An arm stops when its pool passes 90%.

| Time (UTC) | codex (Sol, Luna, and every review) | claude, admin@ (5 h / 7 d) | claude, gmail (5 h / 7 d) | minimax |
| --- | --- | --- | --- | --- |
| 17:24 | 87% | 65% / 84% | 39% / 43% | 1% |
| 17:39 | 87% | 68% / 85% | 43% / 44% | - |
| 17:48 | 88% | 69% / 86% | 44% / 45% | - |
| 17:54 | 88% | 76% / 87% | 50% / 46% | - |
| 17:57 | 88% | - | - | 1% |
| 18:08 | 88% | 2% / 88% (5 h window reset) | 53% / 47% | - |
| 18:14 | 89% | not reported | 59% / 48% | 2% |

No arm reached the stop level. Codex rose from 87% to 89%, close to the limit, so the matrix has six units and not seven.

## Results

One row per run. "Accepted" is the grader's decision under the rule in "Grading":

- **yes**: light checks pass, hidden tests pass, and no defect beyond the control's.
- **minor rework**: one small fix is needed before merge (for example a lint, or a fallback that must become an error).
- **major rework**: the change misses part of the brief or adds a real defect.
- **no**: the writer failed, did not finish, or broke behaviour.

"Reviewer" is the blind `tier/review` verdict. "Tokens" counts input, cached input, and output; output is in brackets. "Writer time" is wall time for the writer process only, without grading.

### O24

| Model | Accepted | Light checks | Hidden tests | Reviewer | Rework (grader) | Writer time | Tokens (out) | Cost | Tool calls | Diff |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| Sol | yes | pass | pass | accept | none | 3m27s | 777k (4k) | $0.20 | 23 | +101/-9 |
| Sonnet | yes | pass | pass | major-rework | same two findings as the control | 1m38s | 508k (7k) | $0.28 | 18 | +79/-9 |
| Luna | yes | pass | pass | minor-rework | one finding, also on the control | 5m17s | 1205k (7k) | $0.02 | 58 | +77/-11 |
| MiniMax | not run | - | - | - | not run: skipped by the lead after O26 stalled | - | - | - | - | - |
| merged fix (control) | - | pass | pass | minor-rework | control: reviewer asks for the gateway-path and exhaustion-error cases | - | - | - | - | +73/-9 |

### O26

| Model | Accepted | Light checks | Hidden tests | Reviewer | Rework (grader) | Writer time | Tokens (out) | Cost | Tool calls | Diff |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| Sol | yes | pass | pass | accept | none | 3m27s | 805k (3k) | $0.22 | 26 | +66/-9 |
| Sonnet | yes | pass | pass | accept | none | 1m25s | 360k (5k) | $0.18 | 11 | +50/-8 |
| Luna | yes | pass | pass | accept | none | 3m12s | 949k (4k) | $0.02 | 47 | +47/-11 |
| MiniMax | no | - | - | - | unfinished: stopped by the lead after 15 min (18:07–18:22 UTC) with no edit; 47 turns, 65 tool calls, 2.0M tokens (150k out). Read files outside the checkout: ~/.pi/agent/tiers.json, models.json, cliproxyapi-models.json | 14m59s | 1970k (149k) | $0.13 | 65 | none |
| merged fix (control) | - | pass | pass | accept | control: no findings | - | - | - | - | +43/-14 |

### O28

| Model | Accepted | Light checks | Hidden tests | Reviewer | Rework (grader) | Writer time | Tokens (out) | Cost | Tool calls | Diff |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| Sol | yes | pass | pass | accept | none | 2m12s | 521k (2k) | $0.15 | 18 | +59/-1 |
| Sonnet | yes | pass | pass | accept | none | 1m27s | 417k (4k) | $0.23 | 14 | +39/-1 |
| Luna | yes | pass | pass | accept | none | 3m20s | 762k (3k) | $0.01 | 32 | +31/-1 |
| MiniMax | yes | pass | pass | accept | none; proved the new tests fail without the fix | 8m51s | 2340k (76k) | $0.09 | 62 | +58/-4 |
| merged fix (control) | - | pass | pass | - | control (no review run) | - | - | - | - | +38/-0 |

### D102

| Model | Accepted | Light checks | Hidden tests | Reviewer | Rework (grader) | Writer time | Tokens (out) | Cost | Tool calls | Diff |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| Sol | yes | pass | - | minor-rework | 3 new tests, clippy clean. Reviewer edge case only: a management key that is a substring of `claude` or `codex` would redact the provider before routing | 7m17s | 1466k (8k) | $0.41 | 31 | +260/-148 |
| Sonnet | minor rework | pass | - | major-rework | Brief behaviour met, clippy clean. Invents `unknown` provider and `quota` window instead of an error row; no new named tests (extends existing ones). Window slugging shared with control | 8m24s | 1586k (26k) | $0.63 | 32 | +214/-148 |
| Luna | major rework | pass | - | major-rework | Rewrites the published 0.17.0 changelog entry instead of adding Unreleased. ASCII-only bucket-window check can abort the whole refresh. Clippy clean, 2 new tests | 15m52s | 4417k (17k) | $0.07 | 92 | +204/-150 |
| MiniMax | not run | - | - | - | not run: out of MiniMax's role (lead cut the scope at 18:14 UTC) | - | - | - | - | - |
| merged fix (control) | - | pass | - | major-rework | control: reviewer asks for any-provider pools and unslugged windows | - | - | - | - | +326/-152 |

### D103

| Model | Accepted | Light checks | Hidden tests | Reviewer | Rework (grader) | Writer time | Tokens (out) | Cost | Tool calls | Diff |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| Sol | minor rework | fail: clippy | - | minor-rework | clippy -D warnings fails (2 test lints: format_push_string, assert_is_empty); writer ran cargo check, not clippy. Fix itself is sound and adds a live-pane check | 6m31s | 1579k (6k) | $0.36 | 33 | +260/-18 |
| Sonnet | yes | pass | - | minor-rework | P3 only: the changelog overstates the lock recheck. Real-Git tests cover both cases | 7m34s | 2237k (28k) | $0.81 | 36 | +86/-0 |
| Luna | minor rework | fail: clippy | - | minor-rework | clippy -D warnings fails (3 test lints); writer ran targeted tests, not clippy. Fix sound, adds a live-pane check | 8m32s | 2820k (9k) | $0.04 | 53 | +90/-3 |
| MiniMax | not run | - | - | - | not run: out of MiniMax's role (lead cut the scope at 18:14 UTC) | - | - | - | - | - |
| merged fix (control) | - | pass | - | accept | control: no findings | - | - | - | - | +80/-10 |

### D107

| Model | Accepted | Light checks | Hidden tests | Reviewer | Rework (grader) | Writer time | Tokens (out) | Cost | Tool calls | Diff |
| --- | --- | --- | --- | --- | --- | ---: | ---: | ---: | ---: | --- |
| Sol | yes | pass | pass | minor-rework | Hidden test passes; 4 new tests incl. real Git; also covers both control findings (branch ownership, close path). Reviewer nit: README wording says archive, not move | 10m24s | 3052k (11k) | $0.59 | 50 | +294/-11 |
| Sonnet | yes | pass | pass | minor-rework | Hidden test passes; real-Git reproduction; hold reaches status and notices. Only finding (close path) is also on the control. Pi exit 1 at shutdown (extension error) | 6m28s | 2069k (24k) | $0.70 | 36 | +170/-4 |
| Luna | no | pass | fail | major-rework | Hidden test fails: `status` never names the directory, because sweep drops the new hold row. No real-Git reproduction (mocked list only). +27 lines | 6m05s | 1540k (4k) | $0.02 | 50 | +27/-0 |
| MiniMax | not run | - | - | - | not run: out of MiniMax's role (lead cut the scope at 18:14 UTC) | - | - | - | - | - |
| merged fix (control) | - | pass | pass | major-rework | control: reviewer asks for branch ownership and the close path | - | - | - | - | +309/-22 |

### Failed, stopped, and discarded runs

Every launched run is listed here or in the tables. Nothing was rerun to get a better result.

| Run | What happened | Counted as |
| --- | --- | --- |
| D107 control and base, first launch | The first acceptance test checked `worktrees.json` at the wrong time, and the grader's hidden step had no `pipefail`. Both runs were discarded and the test was fixed before any model ran D107. | not counted (grader fault) |
| D107-sol, first ticket | Launched while D102-sol was still running. Cancelled before admission, to keep one run per model at a time. | not run |
| D107-sonnet | The writer finished its reply, then Pi exited 1. The `pi-dogsled` extension threw "This extension ctx is stale after session replacement" during shutdown. The change was complete and was graded. | graded normally; exit 1 recorded |
| O26-minimax | Stopped by the lead at 18:22 UTC: 15 min, 47 turns, 65 tool calls, no edit. It read `~/.pi/agent/tiers.json`, `models.json`, and `cliproxyapi-models.json` outside the checkout, plus oxlint schemas and pnpm internals. The other three models finished O26 in 1.5 to 3.5 min. | unfinished (no) |
| O24-minimax | Skipped by the lead after O26-minimax stalled. | not run |
| D102, D103, D107 on MiniMax | Cut by the lead at 18:14 UTC: MiniMax is mechanical-only by policy. | not run: out of MiniMax's role |
| MiniMax probes, 17:15 and 17:31 | Failed with `empty_stream` (see "MiniMax availability"). | blocked, then run |

The grader's hidden step for the Sol, Sonnet, and Luna D107 runs had no `pipefail`, so its exit code was always 0. Those results are read from the `test result:` line instead (`ok` or `FAILED`).

Scratch files: several writers and one reviewer wrote logs to the host's `/tmp` (for example `/tmp/D103-check.log`, `/tmp/d102-review.diff`). The grader searched every later writer's tool calls for those paths and found no reads. All of them were deleted.

### MiniMax availability

| Time (UTC) | Event | Result |
| --- | --- | --- |
| 17:15 | Probe: `pi -p --model cliproxyapi/MiniMax-M3.1-Flash 'reply ok'` | exit 1: `empty_stream: upstream stream closed before first payload` |
| 17:24 | cliproxy-quota 0.1.2 released ("emit one SSE line per stream chunk"); the lead installed it live | - |
| 17:31 | Probe | exit 1: same error |
| 17:35 | Probe | exit 0: `OK` |
| 17:57 | O28 started, after every Sol and Sonnet run had started | accepted |
| 18:07 | O26 started | stopped at 18:22, unfinished |

On the run that finished, MiniMax was correct and careful: it showed that its new tests fail without the fix. It was also the slowest writer on O28 (8 min 51 s, against 1.5 to 3.5 min) and wrote 76k output tokens, against 2k to 4k for the others. No MiniMax turn returned a stream error after the plugin fix.

## Totals

All runs, per model. MiniMax ran two units only (see "Runs").

| Model | Runs | Accepted | Minor rework | Major rework or no | Writer time (total) | Tokens (total) | Output tokens | Cost (list) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Sol | 6 | 5 | 1 | 0 | 33m18s | 8.2M | 38k | $1.94 |
| Sonnet | 6 | 5 | 1 | 0 | 26m56s | 7.2M | 96k | $2.84 |
| Luna | 6 | 3 | 1 | 2 | 42m18s | 11.7M | 46k | $0.18 |
| MiniMax | 2 | 1 | 0 | 1 | 23m50s | 4.3M | 226k | $0.22 |

By unit kind:

| Model | oisinpi units (O24, O26, O28) accepted | dogsled units (D102, D103, D107) accepted |
| --- | --- | --- |
| Sol | 3 of 3 | 2 of 3 |
| Sonnet | 3 of 3 | 2 of 3 |
| Luna | 3 of 3 | 0 of 3 |
| MiniMax | 1 of 2 | not run |

## Uncertainty

- **One run per cell.** Each unit ran once per model. A single run cannot separate a model's skill from luck on that unit. A difference of one accepted run between two models is not a signal.
- **Six units.** Three are small oisinpi fixes that Sol, Sonnet, and Luna all passed. Only the three dogsled units separate those models, so the exec ranking rests on three units. MiniMax has two runs, so its result is a warning, not a measurement.
- **No plan prefix.** Real executors receive a plan and a first edit from `tier/plan-*`. These runs start from the brief alone, so they measure a harder task than real `tier/exec-*` work.
- **One reviewer family.** `tier/review` chose Sol for every review. Sol therefore reviewed its own family's work, blind but not cross-family. The reference controls show the reviewer is strict: it asked for rework on three of the five merged fixes it reviewed (O24, D102, D107). O28's control was not reviewed.
- **Live state leaks into two briefs.** D103 and D107 tell the writer to read `dogsled status`. On the replay hosts that command runs dogsled 0.20.0, which already contains the fixes. Behaviour is visible; source is not.
- **Contention.** Runs shared hosts with live work (netcup load average 18 to 45 on 16 cores). Elapsed time includes Rust builds under that load, so compare times only within a unit.
- **Cost column.** It is Pi's list-price estimate from the provider's table, per million tokens: Sol $2 in and $10 out, Luna $0.10 in and $0.50 out, MiniMax $0.15 in and $0.60 out. Sonnet's rates come from the same table, with cached input billed separately. Traffic runs on subscriptions, so the real limit is each pool's quota window, not money.
- **Grader judgment.** D102 and D103 have no public-behaviour hidden test. They are graded by reading the diff against the brief's acceptance list, which is one person's judgment.

## Recommendation

### Findings

1. **Sol, Sonnet, and Luna all type small, specified fixes well.** On O24, O26, and O28 every one of their runs passed the light checks and the hidden tests. There, the models differ only in time and cost.
2. **The models separate on judgment units.** D102, D103, and D107 need a design choice or a diagnosis.
   - Sol: 2 of 3 accepted. Its D103 miss was two Clippy lints: it ran `cargo check`, not Clippy. Its D107 was the most complete fix. It also closed both gaps that the reviewer found in the merged fix.
   - Sonnet: 2 of 3 accepted. Its D102 needs a small rework: it invents `unknown` and `quota` identities where the brief wants an error row.
   - Luna: 0 of 3 accepted. D103 failed Clippy. D102 rewrote a published changelog entry. D107 failed the hidden test, because its hold row never reaches `status`. Luna also used a mocked Git list where the brief asked for real Git.
   - MiniMax: not run, by policy.
3. **MiniMax is not reliable yet, even on mechanical units.** It finished 1 of 2 units. The finished one was correct but took 2.5 to 6 times longer than the other models. The other stalled for 15 minutes with no edit and read live configuration outside its checkout.
4. **Sonnet is the fastest writer on small units. Sol writes the fewest output tokens.** On the oisinpi units, Sonnet took 85 to 98 s, Sol 132 to 207 s, and Luna 192 to 317 s. On the dogsled units the three were close (6 to 10 min each). Sonnet wrote 1.5 to 5 times more output tokens than Sol on the same unit.
5. **Luna is about 6 to 30 times cheaper than Sol at list price, but needs more turns.** It made 1.5 to 2.5 times Sol's tool calls on the oisinpi units.
6. **Both Clippy failures came from writers that ran a narrower check.** The shared writer rule says "a formatter or linter on changed files". For Rust it should name `cargo clippy --all-targets -- -D warnings`. That is a profile change, outside this document's scope.

### Tier order

- **`tier/exec-gpt`: keep `[sol, sonnet]`.** Sol had the best judgment-unit work: D102 and D107 accepted, and D107 better than the merged fix. Its one miss is a lint, and the build lead's heavy checks catch lints. Luna must not enter an executor tier: it failed all three units that needed a design or a diagnosis.
- **`tier/exec-claude`: keep `[sonnet, sol]`.** Sonnet matched Sol on acceptance and was the fastest writer on small units.
- **`tier/mech`: change to `[luna, minimax, sol]`.** Luna finished all three mechanical units, and every one was accepted. MiniMax finished one of two. A stall is not an error, so the tier never falls back from a stalled MiniMax run: someone must notice it and stop it. Luna first follows the policy, because Luna is also a cheap model.

Luna draws on the codex pool, which Sol and `tier/review` also use. That pool stood at 89% at the end of this run. pi-tiers skips an exhausted pool (every account at 100% with a future reset), so `tier/mech` falls to MiniMax when codex runs out. If codex pressure becomes the main limit, swap Luna and MiniMax back. Measure MiniMax again first.

### MiniMax

MiniMax stays in `tier/mech` only. It moves from first to second. This follows the policy, and the data agrees: it stalled on a two-file mechanical unit. It is still useful as the non-codex fallback, and its own pool was at 1 to 2%.

Run it again before you move it back to first. Use the same units after a MiniMax or plugin update, and include a time limit in the brief.

### Proposed `tiers.json`

Only the three calibrated tiers are shown. The other tiers stay as they are. Do not apply this without approval; this PR changes only this document.

```json
{
  "exec-claude": [
    "cliproxyapi/claude-sonnet-5-5",
    "cliproxyapi/gpt-6.1-sol"
  ],
  "exec-gpt": [
    "cliproxyapi/gpt-6.1-sol",
    "cliproxyapi/claude-sonnet-5-5"
  ],
  "mech": [
    "cliproxyapi/gpt-6-luna",
    "cliproxyapi/minimax-m3.1-flash",
    "cliproxyapi/gpt-6.1-sol"
  ]
}
```

Only `mech` changes, and only its first two targets swap places.

### What would change this

- A second run per cell on D102, D103, and D107. One more pass or failure on those units can change the exec order. The oisinpi units cannot, because every Sol, Sonnet, and Luna run passed them.
- Runs with the plan-model prefix that real `writer-*` work gets. A plan may close Luna's design gap, which would make Luna a fair `tier/exec-*` fallback.
- A reviewer from another family. Sol reviewed every run, including its own family's.
- A full MiniMax arm on O24, O26, and O28 with a stall limit.

## Appendix A: harness

`launch.py` builds one spec per run and starts `dogsled run --json --timeout 100m --cwd /tmp -- bash -c <run.sh> calib <spec>`.
The job prints its artifacts as base64 between `===CALIB-BEGIN===` markers. `fetch.py` decodes them from `dogsled logs`.

### run.sh

This is the final version. The only later edit adds `set -o pipefail` to the hidden step, after the Sol, Sonnet, and Luna D107 runs; those runs are read from their `test result:` line.

```bash
# Calibration replay harness. Invoked as: bash -c "$(cat run.sh)" calib <spec-b64>
# The spec is a base64 shell fragment that sets RUN_ID REPO BASE REF MODEL KIND
# WRITER_TIMEOUT OVERLAY APPEND_PATH APPEND_B64 HIDDEN_CMD BRIEF_B64 WRITER_B64 REVIEW_B64.
set -u
eval "$(printf '%s' "$1" | base64 -d)"
ROOT="$HOME/.cache/calib"
CACHE="$ROOT/mirror/$REPO.git"
W="$ROOT/runs/$RUN_ID"
OUT="$ROOT/out/$RUN_ID"
TO=""
command -v timeout >/dev/null 2>&1 && TO=timeout
[ -z "$TO" ] && command -v gtimeout >/dev/null 2>&1 && TO=gtimeout
tmo() { local s=$1; shift; if [ -n "$TO" ]; then "$TO" --kill-after=30 "$s" "$@"; else "$@"; fi; }
log() { printf '[%s] %s\n' "$(date -u +%H:%M:%S)" "$*" | tee -a "$OUT/harness.log"; }
rm -rf "$W" "$OUT"
mkdir -p "$ROOT/mirror" "$OUT"
printf '%s' "$BRIEF_B64" | base64 -d > "$OUT/brief.md"
printf '%s' "$WRITER_B64" | base64 -d > "$OUT/writer.md"
printf '%s' "$REVIEW_B64" | base64 -d > "$OUT/review-prompt.md"
printf '{"disabled":true}\n' > "$ROOT/hindsight-off.json"
export HINDSIGHT_CONFIG="$ROOT/hindsight-off.json"
export CARGO_TARGET_DIR="$ROOT/target/$RUN_ID"
log "host=$(hostname) run=$RUN_ID model=$MODEL repo=$REPO base=$BASE"

# 1. Mirror and isolated checkout at BASE. Only BASE's ancestors are fetched, so later
#    commits (including the merged fix) are unreachable from the checkout.
url="https://github.com/oisin-ee/$REPO.git"
if [ ! -d "$CACHE" ]; then git clone -q --bare "$url" "$CACHE" 2>>"$OUT/harness.log"; fi
(cd "$CACHE" && git fetch -q "$url" '+refs/heads/*:refs/heads/*' '+refs/pull/*/head:refs/pull/*/head' 2>>"$OUT/harness.log")
git -C "$CACHE" config uploadpack.allowAnySHA1InWant true
git init -q "$W"
git -C "$W" fetch -q --no-tags "$CACHE" "$BASE" || { log "fetch base failed"; }
git -C "$W" checkout -q -b replay FETCH_HEAD
git -C "$W" config user.name calib; git -C "$W" config user.email calib@localhost
git -C "$W" config core.hooksPath /dev/null
cd "$W" || exit 1
log "checkout $(git rev-parse --short HEAD)"

# 2. Dependencies (repository setup, as dogsled.toml would run it).
if [ "$KIND" = ts ]; then
  { (command -v proto >/dev/null && proto trust .prototools && proto run pnpm -- install --frozen-lockfile) || pnpm install --frozen-lockfile; } > "$OUT/setup.log" 2>&1
  log "setup exit=$?"
fi

# 3. Writer.
start=$(date +%s)
if [ "$MODEL" = reference ]; then
  git -C "$CACHE" diff --no-ext-diff --binary "$BASE" "$REF" > "$OUT/ref.diff"
  git apply --whitespace=nowarn "$OUT/ref.diff"; wexit=$?
  : > "$OUT/writer.out"
elif [ "$MODEL" = base ]; then
  wexit=0; : > "$OUT/writer.out"
else
  tmo "$WRITER_TIMEOUT" pi -p --model "$MODEL" --thinking medium \
    --append-system-prompt "$OUT/writer.md" --session-dir "$OUT/wsession" \
    "$(cat "$OUT/brief.md")" > "$OUT/writer.out" 2>&1 < /dev/null
  wexit=$?
fi
end=$(date +%s)
log "writer exit=$wexit elapsed=$((end-start))s"
printf '%s %s\n' "$wexit" "$((end-start))" > "$OUT/writer.status"

# 4. Diff against BASE (includes any commits the writer made anyway).
git add -A >/dev/null 2>&1
git diff --cached --no-ext-diff --no-color --stat "$BASE" > "$OUT/diffstat.txt"
git diff --cached --no-ext-diff --no-color "$BASE" > "$OUT/patch.diff"
git rev-list --count "$BASE"..HEAD > "$OUT/commits.txt"

# 5. Grader light checks.
chk() { local name=$1; shift; log "check $name"; { echo "### $name: $*"; tmo 1500 bash -c "set -o pipefail; $*" 2>&1 | tail -n 60; echo "### $name exit=${PIPESTATUS[0]}"; } >> "$OUT/checks.log"; }
changed=$(git diff --cached --name-only --diff-filter=d "$BASE" | tr '\n' ' ')
if [ "$KIND" = ts ]; then
  chk oxfmt "pnpm exec oxfmt --check $changed"
  chk oxlint "cd packages/pi-tiers && pnpm exec oxlint --config ../../.oxlintrc.json --deny-warnings"
  chk tsc "pnpm exec tsc --noEmit -p packages/pi-tiers"
  chk vitest "cd packages/pi-tiers && pnpm exec vitest run"
else
  chk fmt "cargo fmt --check"
  chk clippy "cargo clippy --all-targets -- -D warnings"
  chk test "cargo test 2>&1 | grep -E '^test result|FAILED|panicked|^failures:|^    [a-z_:]+$' | head -n 80"
fi
grep -E '^### .* exit=' "$OUT/checks.log" > "$OUT/checks.summary"

# 6. Blind review on tier/review (skipped for the reference arm).
if [ "$MODEL" != base ]; then
  {
    echo "mode: code"
    echo "base: $BASE"
    echo
    echo "Review the uncommitted change in this checkout (git diff $BASE) against the brief below."
    echo "Do not run builds or tests. The grader already ran these light checks:"
    cat "$OUT/checks.summary"
    echo
    echo "Report defects against the brief and the repository rules, then the rework a maintainer would require before merge."
    echo "End with exactly two lines:"
    echo "VERDICT: accept | minor-rework | major-rework | reject"
    echo "REWORK: <one sentence>"
    echo
    echo "----- brief -----"
    cat "$OUT/brief.md"
  } > "$OUT/review-task.md"
  rstart=$(date +%s)
  tmo 1500 pi -p --model tier/review --thinking medium \
    --append-system-prompt "$OUT/review-prompt.md" --session-dir "$OUT/rsession" \
    "$(cat "$OUT/review-task.md")" > "$OUT/review.txt" 2>&1 < /dev/null
  log "review exit=$? elapsed=$(( $(date +%s) - rstart ))s"
fi

# 7. Hidden behavioural acceptance: overlay the original PR's public-behaviour tests.
if [ "$OVERLAY" != "-" ] || [ "$APPEND_PATH" != "-" ]; then
  if [ "$OVERLAY" != "-" ]; then
    for p in $(printf '%s' "$OVERLAY" | tr ',' ' '); do git -C "$CACHE" show "$REF:$p" > "$p"; done
  fi
  if [ "$APPEND_PATH" != "-" ]; then printf '%s' "$APPEND_B64" | base64 -d >> "$APPEND_PATH"; fi
  log "hidden tests"
  { echo "### hidden: $HIDDEN_CMD"; tmo 1500 bash -c "set -o pipefail; $HIDDEN_CMD" 2>&1 | tail -n 80; echo "### hidden exit=${PIPESTATUS[0]}"; } > "$OUT/hidden.log"
fi

# 8. Session summary.
node - "$OUT" > "$OUT/summary.json" 2>>"$OUT/harness.log" <<'NODE'
const fs = require("fs"), path = require("path"), out = process.argv[2];
function sessions(dir) {
  const files = [];
  const walk = (d) => { if (!fs.existsSync(d)) return; for (const e of fs.readdirSync(d, { withFileTypes: true })) { const p = path.join(d, e.name); if (e.isDirectory()) walk(p); else if (p.endsWith(".jsonl")) files.push(p); } };
  walk(dir);
  const s = { files: files.length, assistant: 0, toolCalls: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0, cost: 0, models: {}, errors: [], commands: [], final: "" };
  for (const f of files) for (const line of fs.readFileSync(f, "utf8").split("\n")) {
    if (!line) continue; let d; try { d = JSON.parse(line); } catch { continue; }
    const m = d.message; if (!m || m.role !== "assistant") continue;
    s.assistant++; const u = m.usage || {};
    s.input += u.input || 0; s.output += u.output || 0; s.cacheRead += u.cacheRead || 0; s.cacheWrite += u.cacheWrite || 0; s.total += u.totalTokens || 0; s.cost += (u.cost && u.cost.total) || 0;
    const key = `${m.provider}/${m.model}`; s.models[key] = (s.models[key] || 0) + 1;
    if (m.stopReason === "error" || m.errorMessage) s.errors.push(String(m.errorMessage || "error").slice(0, 200));
    let text = "";
    for (const c of m.content || []) {
      if (c.type === "toolCall") { s.toolCalls++; const a = c.arguments || {}; s.commands.push(`${c.name}: ${String(a.command ?? a.path ?? a.query ?? JSON.stringify(a)).replace(/\s+/g, " ").slice(0, 220)}`); }
      if (c.type === "text") text += c.text;
    }
    if (text.trim()) s.final = text;
  }
  s.final = s.final.slice(-3000);
  return s;
}
const [wexit, elapsed] = fs.readFileSync(path.join(out, "writer.status"), "utf8").trim().split(" ").map(Number);
const w = sessions(path.join(out, "wsession")), r = sessions(path.join(out, "rsession"));
fs.writeFileSync(path.join(out, "commands.txt"), w.commands.join("\n") + "\n");
delete w.commands; delete r.commands;
console.log(JSON.stringify({ writer: { exit: wexit, elapsed, ...w }, review: r }, null, 1));
NODE
log "summary written"
tail -c 4000 "$OUT/writer.out" > "$OUT/writer-tail.txt"

# 9. Emit artifacts through the job log, then remove the checkout, target and output.
cd "$ROOT"
head -c 300000 "$OUT/patch.diff" > "$OUT/patch.trunc"; mv "$OUT/patch.trunc" "$OUT/patch.diff"
echo "===CALIB-BEGIN $RUN_ID==="
tar czf - -C "$OUT" --exclude=wsession --exclude=rsession --exclude=writer.out . | base64 | fold -w 76
echo "===CALIB-END $RUN_ID==="
rm -rf "$W" "$OUT" "$CARGO_TARGET_DIR"
echo "calib done $RUN_ID"
```

### launch.py (unit table)

```python
#!/usr/bin/env python3
"""Launch one calibration replay as a dogsled job: launch.py <unit> <model> [--host H]."""
import base64, json, shlex, subprocess, sys, time, pathlib

H = pathlib.Path(__file__).parent
REVIEW = pathlib.Path.home() / "dev/dotfiles/conf.d/agent-ide/dogsled/profiles/reviewer/prompt.md"
TS_HIDDEN = "cd packages/pi-tiers && pnpm exec vitest run src/router.test.ts src/policy.test.ts"
UNITS = {
    "O24": dict(REPO="oisinpi", BASE="02575b9e948a07093551c858023f925dd97ca0d0", REF="bcba2d42673c2cf12c23b62079c2a96c0b098e30", KIND="ts",
                OVERLAY="packages/pi-tiers/src/router.test.ts,packages/pi-tiers/src/fixture.ts", HIDDEN_CMD=TS_HIDDEN),
    "O26": dict(REPO="oisinpi", BASE="a260f6bbdbfe6a21be3be2f94fedbd70d44f2bca", REF="3e750ace481cfcf55296da1df034473d86d74347", KIND="ts",
                OVERLAY="packages/pi-tiers/src/router.test.ts,packages/pi-tiers/src/policy.test.ts,packages/pi-tiers/src/fixture.ts",
                HIDDEN_CMD=TS_HIDDEN),
    "O28": dict(REPO="oisinpi", BASE="5845ac39489e3b73f92414a75f0b9fc42f550b64", REF="6f6e3092eaec85e4f47c52ec652db359d7ddd78f", KIND="ts",
                OVERLAY="packages/pi-tiers/src/router.test.ts,packages/pi-tiers/src/policy.test.ts", HIDDEN_CMD=TS_HIDDEN),
    "D102": dict(REPO="dogsled", BASE="4a28cccd0f47a94459a9eb9bb681d927ebfa18cd", REF="540b5b4e23d294466068f20664ab24fa3fe69661", KIND="rust",
                 OVERLAY="-", HIDDEN_CMD="-"),
    "D103": dict(REPO="dogsled", BASE="4a28cccd0f47a94459a9eb9bb681d927ebfa18cd", REF="2c2ed17382dbc5e638beda9f908c239c508176a7", KIND="rust",
                 OVERLAY="-", HIDDEN_CMD="-"),
    "D107": dict(REPO="dogsled", BASE="2c2ed17382dbc5e638beda9f908c239c508176a7", REF="1876f273ab179b59409b8211271e9fa257f51dec", KIND="rust",
                 OVERLAY="-", APPEND_PATH="tests/cli/stop_sweep.rs", APPEND_FILE="d107_accept.rs",
                 HIDDEN_CMD="cargo test --test cli calib_ 2>&1 | grep -E '^test |test result|panicked' "),
}
MODELS = {"sol": "cliproxyapi/gpt-6.1-sol", "sonnet": "cliproxyapi/claude-sonnet-5-5",
          "luna": "cliproxyapi/gpt-6-luna", "minimax": "cliproxyapi/MiniMax-M3.1-Flash", "ref": "reference", "base": "base"}


def b64(data: bytes) -> str:
    return base64.b64encode(data).decode()


def main() -> None:
    unit, model = sys.argv[1], sys.argv[2]
    host = sys.argv[sys.argv.index("--host") + 1] if "--host" in sys.argv else None
    u = dict(UNITS[unit])
    run_id = f"{unit}-{model}-{time.strftime('%H%M%S')}"
    spec = dict(RUN_ID=run_id, MODEL=MODELS[model], WRITER_TIMEOUT="2700",
                APPEND_PATH=u.pop("APPEND_PATH", "-"), APPEND_B64="", **{k: v for k, v in u.items() if k != "APPEND_FILE"})
    if "APPEND_FILE" in u:
        spec["APPEND_B64"] = b64((H / u["APPEND_FILE"]).read_bytes())
    spec["BRIEF_B64"] = b64((H / "briefs" / f"{unit}.md").read_bytes())
    spec["WRITER_B64"] = b64((H / "writer-replay.md").read_bytes())
    spec["REVIEW_B64"] = b64(REVIEW.read_bytes())
    frag = "".join(f"{k}={shlex.quote(str(v))}\n" for k, v in spec.items())
    cmd = ["dogsled", "run", "--json", "--timeout", "100m", "--cwd", "/tmp"]
    if host:
        cmd += ["--host", host]
    cmd += ["--", "bash", "-c", (H / "run.sh").read_text(), "calib", b64(frag.encode())]
    res = subprocess.run(cmd, capture_output=True, text=True)
    rec = {"run_id": run_id, "unit": unit, "model": model, "host_pin": host, "launched": time.time(),
           "stdout": res.stdout.strip(), "stderr": res.stderr.strip()[-500:], "rc": res.returncode}
    with open(H / "launches.jsonl", "a") as f:
        f.write(json.dumps(rec) + "\n")
    print(json.dumps(rec))


if __name__ == "__main__":
    main()
```

## Appendix B: D107 acceptance test

Appended to `tests/cli/stop_sweep.rs` after the writer and the review finish.
It fails at `2c2ed17` (status does not name the directory) and passes at `1876f27` (the merged fix).

```rust
// Calibration acceptance (public behaviour of the 107 brief): a registered worktree that Git
// pruned while its directory remains is reported, never failed on, and never deleted.
#[test]
fn calib_git_pruned_worktree_with_remaining_directory_is_held_not_failed() {
    let (rig, tree) = close_rig("calib-pruned-worktree");
    let repo = rig.dir.join("home/repo").canonicalize().unwrap();
    let head = git(&tree, &["rev-parse", "HEAD"]).trim().to_owned();
    rig.put("state/repositories.json", &json!([repo]));
    let registration = json!([{"repo":repo,"path":tree,"branch":"feat/x","head":head}]);
    rig.put("state/worktrees.json", &registration);
    fs::create_dir_all(tree.join("build")).unwrap();
    fs::write(tree.join("build/generated"), "output\n").unwrap();
    fs::remove_file(tree.join(".git")).unwrap();
    git(&repo, &["worktree", "unlock", tree.to_str().unwrap()]);
    git(&repo, &["worktree", "prune"]);
    assert!(!git(&repo, &["worktree", "list"]).contains(".worktrees/x"));

    gate(&rig, "w1:p1");
    for _ in 0..2 {
        let log = serve(&rig);
        assert!(!log.contains("failed:"), "{log}");
        assert!(!log.contains("is not a working tree"), "{log}");
        assert!(tree.join("build/generated").is_file());
    }
    let human = String::from_utf8(rig.run(&["status"]).stdout).unwrap();
    assert!(
        human.contains(&tree.display().to_string()),
        "status must name the remaining directory: {human}"
    );

    fs::remove_dir_all(&tree).unwrap();
    gate(&rig, "w1:p1");
    let log = serve(&rig);
    assert!(!log.contains("failed:"), "{log}");
    assert_eq!(rig.read("state/worktrees.json"), json!([]));
}
```

## Appendix C: writer prompt

```markdown
Research, write code, run codemods, and use Git within the assigned unit. Read ~/.agents/rules for the governing rules.

Run only these light checks:
- A formatter or linter on changed files.
- A type check of the changed package.
- Unit tests covering the changed files.
- Commit hooks.

Never run full suites, builds of other packages, e2e, dependency installs, CI watching, or gates.
If a hook fails, fix it in scope or report `blocked` with its output. Never bypass hooks.
Do not spawn agents.

Replay delivery. This rule replaces every delivery step in the task (commit, push, pull request, reply to a lead, `done`):
- This checkout is a local replay with no remote. Do not commit, push, open a pull request, call `dogsled`, or message anyone.
- Leave your finished change uncommitted in this checkout.
- Work only from this checkout. Do not read GitHub pull requests, issues, or other checkouts of this repository.
- End with one reply: the changed files, the checks you ran and their results, and any blockers.
```

## Appendix D: briefs

O24, O26 and O28 had no stored brief. These briefs restate each PR's defect and required behaviour.
D102, D103 and D107 are the stored dogsled launch briefs, with only the delivery lines changed.

### O24

````markdown
Repository: oisin-ee/oisinpi. Package `packages/pi-tiers` is the `@oisin-ee/pi-tiers` Pi extension: virtual `tier/*` models that route each turn to an ordered list of physical targets from `~/.pi/agent/tiers.json`.

## Defect (live)
`tiers.json` puts `cliproxyapi/minimax-m3.1-flash` first in `tier/mech` and second in `tier/scout`. CLIProxy does not serve that model yet, so Pi's model registry does not list it. When the router chooses it, the turn stops with `Unknown model`. So `tier/mech` and `tier/scout` fail instead of using their next target.

## Required behaviour
- `route()` and the boundary recovery (`agent_before_settle`) choose only targets that the model registry lists (`ctx.modelRegistry.getAvailable()`), in tier order.
- A persisted choice on the branch whose model is no longer listed moves to the next listed target.
- When no target in the tier is listed, the turn stops with the error `No available target in tier.`
- Quota, gateway, and cooldown behaviour do not change.

## Rules
- Owned files: `packages/pi-tiers/src/**` and one changeset in `.changeset/`.
- Add router tests with the existing test harness (`src/fixture.ts`) for the three cases above.
- Light checks: oxfmt on changed files, and oxlint and vitest in `packages/pi-tiers`.
````

### O26

````markdown
Repository: oisin-ee/oisinpi. Package `packages/pi-tiers` is the `@oisin-ee/pi-tiers` Pi extension: virtual `tier/*` models that route each turn to an ordered list of physical targets from `~/.pi/agent/tiers.json`. The router already skips targets that the model registry does not list.

## Defect (live)
`tiers.json` names `cliproxyapi/minimax-m3.1-flash`. CLIProxy's model list, and so Pi's registry, has `cliproxyapi/MiniMax-M3.1-Flash`. Exact matching leaves MiniMax unlisted forever, so `tier/mech` never uses it. Its quota would also go to the wrong pool.

## Required behaviour
- Tier targets match registry models without regard to case.
- Dispatch uses the registry's exact reference (for example `cliproxyapi/MiniMax-M3.1-Flash`), so later comparisons in dispatch and recovery stay exact.
- Pool prefixes (`claude`, `codex`/`gpt`, `minimax`) match without regard to case: `proxy/MiniMax-M3.1-Flash` is in pool `minimax`.
- A quota error on such a target cools the `minimax` pool, and the tier moves to its next target.

## Rules
- Owned files: `packages/pi-tiers/src/**` and one changeset in `.changeset/`.
- Add a policy test for pool mapping and a router test, with the existing harness (`src/fixture.ts`), for a registry id that differs only in case.
- Light checks: oxfmt on changed files, and oxlint and vitest in `packages/pi-tiers`.
````

### O28

````markdown
Repository: oisin-ee/oisinpi. Package `packages/pi-tiers` is the `@oisin-ee/pi-tiers` Pi extension: virtual `tier/*` models that route each turn to an ordered list of physical targets. `src/policy.ts` classifies a failed request as gateway, quota, transient, or request, and the router acts on that class.

## Defect (live)
MiniMax streaming through CLIProxy returns HTTP 500 with `empty_stream: upstream stream closed before first payload`. The classifier treats this as a request error, so `tier/mech` stops instead of moving to its next target (Luna).

## Required behaviour
- A provider-side failure of one target is `transient`, so the tier moves to the next target. This covers HTTP status 500 and 504, and messages that contain `HTTP 500`, `HTTP 504`, `internal_server_error`, `server_error`, or `empty_stream`.
- The gateway class (502 or 503 from CLIProxy itself, "no available server") and the quota class do not change. HTTP 400 and 401 stay `request`. Messages that only mention a number, such as "Timeout after processing 503 tokens", keep their current class.

## Rules
- Owned files: `packages/pi-tiers/src/**` and one changeset in `.changeset/`.
- Add classifier cases to `src/policy.test.ts`, and a router test with the existing harness (`src/fixture.ts`) that settles on `empty_stream` and continues on the next target.
- Light checks: oxfmt on changed files, and oxlint and vitest in `packages/pi-tiers`.
````

### D102

````markdown
Implement this brief in dogsled.

# Brief: generic CLIProxy quota for plugin providers

## Outcome
`dogsled quota` reports quota for every CLIProxy credential that CLIProxy marks `supports_quota`, through CLIProxy's own normalized quota route. When the planned MiniMax CLIProxy plugin is installed, MiniMax rows must appear on the board with no further dogsled change.

## Facts (CLIProxy v8.0.33, github.com/router-for-me/CLIProxyAPI tag v8.0.33)
- `GET /v0/management/auth-files` entries carry `provider`, `auth_index`, `disabled`, and, for plugin-served credentials, `supports_quota: true` and `quota_provider` (internal/api/handlers/management/auth_files.go near line 687).
- `POST /v0/management/quota/fetch` with body `{"auth_index":"<idx>"}` returns QuotaFetchResponse (sdk/pluginapi/types.go 1611-1750): `{groups:[{displayName, buckets:[{window, remainingFraction, resetTime (RFC3339), description}]}]}`. Snake-case aliases also exist: display_name, remaining_fraction, reset_time. Errors are 400/404/501/502 with `{error}`.
- No live credential has `supports_quota` today. Claude and codex are not plugin-served and keep their existing api-call paths.

## Change (owned: src/quota.rs, README.md, CHANGELOG.md under Unreleased)
1. In `collect`, for every enabled auth file with `supports_quota == true` whose provider is not claude or codex, POST `quota/fetch` with its `auth_index`. Map each bucket to a row:
   - pool = the auth file's provider, lowercased.
   - window = the bucket window. With more than one group, use `<group slug>-<window>`. Slugs are lowercase `[a-z0-9_-]`.
   - used_pct = (1 - remainingFraction) * 100, clamped to 0..100.
   - resets_ms from resetTime.
   - A failed fetch becomes an error row for that account, like the existing per-account errors.
   - Use the same account-label redaction as the claude and codex rows.
2. Remove the `claude-api-key` MiniMax discovery, the ApiKeys and ApiKey types, and the MiniMax token_plan/remains parser and its tests. They target a setup that will not exist, and the parser has known bugs.
3. The snapshot stays version 1, and the JSON for claude and codex rows is unchanged. `pool` must accept any provider string, which pi-tiers and the board read as a string. Keep the existing window validation for claude and codex. Check src/board.rs and pi/ for anything that assumes a closed pool set.

## Checks (light only)
- Run `cargo test quota`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
- Add tests using the existing fake transport:
  - a supports_quota credential maps groups and buckets to rows;
  - a failing fetch gives an error row;
  - a disabled credential is skipped;
  - claude and codex are unaffected;
  - no claude-api-key call is made;
  - no secret appears in output.

## Deliver
Report the test counts and any assumptions.
````

### D103

````markdown
Dogsled cleanup fails on the Mac with: `refuse /Users/oisin/dev/oisinpi/.worktrees/tiers-case: worktree identity disappeared`. The lead found that the path no longer exists and that `git worktree list` in /Users/oisin/dev/oisinpi does not list it, but Dogsled still holds a record for it, so every cleanup pass fails.

Your task:
1. Read `dogsled status` and the cleanup code in this repository. Find why a record whose worktree is already gone from git and disk stays and makes the pass fail, instead of being reconciled.
2. Fix the cause in Dogsled's source, with a test that reproduces the vanished-worktree case. Keep every safeguard: never remove a worktree that still exists, is dirty or is held by a live pane.
3. Do not hand-edit Dogsled state files, do not delete anything by hand, and do not bypass the cleanup checks. Do not touch other repositories. Do not start other agents.
4. Add a CHANGELOG.md `## Unreleased` bullet. Reply with the cause, and how the stale record gets cleared once the fix is installed.
````

### D107

````markdown
Fix a dogsled cleanup failure at its cause, in oisin-ee/dogsled. Work only in this worktree. Base: fix/cleanup-vanished-worktree (2c2ed17, not yet landed; it builds on origin/main 31b1f22, release 0.19.0).

Failure (local host, cleanup pass at 1792591510325 ms):
  git worktree remove kept /Users/oisin/dev/tova/.worktrees/feat-tova-shared-stack after its panes closed: fatal: '/Users/oisin/dev/tova/.worktrees/feat-tova-shared-stack' is not a working tree

How it arose: the directory had no .git file, so Git marked it prunable. Another agent checked every file and ran `git worktree prune -v`, which dropped `.git/worktrees/feat-tova-shared-stack`. The directory still exists. It holds only (a) 224 files identical to the branch tip d9a1cd3, (b) apps/android/gradlew.bat, which differs only by CRLF (the file has eol=crlf), and (c) gitignored generated output: Android build/, .gradle/, apps/ios/.build, apps/e2e/node_modules, generated apps/ios/Packages/TovaProtocol/Sources/, and apps/ios/Tova/Config/Local.xcconfig (written by apps/ios/scripts/configure-local.swift). Branch feat/tova-shared-stack still exists at d9a1cd3.

2c2ed17 treats "Git no longer lists it AND the path is absent" as already removed. It still refuses "unlisted but path exists". That safeguard is correct: never delete an unregistered directory blindly. What's needed: cleanup must not keep failing on every pass with a raw git error, and the owner needs a safe way to finish it.

Do this:
1. Read src/cleanup.rs and 2c2ed17. Reproduce with a test: register a worktree, delete its .git file, run `git worktree prune`, keep the directory, then run cleanup.
2. Choose the correct owner-level behaviour. Preferred direction (check it against the code and README before you commit to it): detect "registered in dogsled, unlisted by Git, path present" before calling `git worktree remove`. Do not run git worktree remove on that path, and do not delete it. Report it once as a hold/attention row with a clear reason, for example "worktree unregistered by git; directory remains: <path>", plus the action the owner must take. Retire the registration only through an explicit, safe path that already exists in dogsled (or one you add that refuses dirty or unknown content). If the design needs a decision you can't make, stop and report the options.
3. Never use rm -rf, git clean, --force, or any cleanup-safeguard bypass. Do not touch /Users/oisin/dev/tova or its .worktrees directory. Do not edit dogsled's live registry or state files by hand.
4. Run cargo fmt, clippy, and the cleanup tests. Add a CHANGELOG entry. Do not release or deploy.

Report: the root cause in cleanup.rs (file:line), the behaviour you chose and why, the test names, and the exact steps the owner should take for the tova path once the fix is live.
````
