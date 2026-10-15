# Replacing load1 in wrangle admission: evidence note v2

Date: 2026-10-15. Hosts: local Mac (Mac14,10, M2 Pro 8P+4E, 12 CPUs, 32 GiB, macOS 26.6.2, xnu-12377.121.6), netcup (NixOS, Linux 6.18.37, 16 CPUs, slice `user-1001.slice` cpu.max 16 cores, cpu.weight 30, memory.max 48 GiB), momokaya-2 (Linux 6.12.63, 7 CPUs, slice cpu.max 7, memory.max 8 GiB, zram swap 8 GiB).
Current rule: `src/plan.rs evaluate`: eligible when `1.5 × cores − load1 − 1.0 × reservations > 0`; disk gate 15%. No `config.toml` exists on any host.

Every number has its source in brackets: [K] = kernel/xnu source (child report "metric-kernel", permalinks at v6.18/v6.12/xnu-12377.121.6), [P] = prior art (child report "metric-priorart"), [S] = my sampler (`/tmp/wr-sample-linux.sh`, `/tmp/wr-sample-mac.sh`, 10 s / 8 s intervals, netcup 29 min, Mac 23 min as of writing, still running), [L] = pi session logs (`/tmp/wr_local.jsonl`, `/tmp/wr_netcup.jsonl`, `/tmp/wr_bg_*.jsonl`, `/tmp/wr_act_*.jsonl`; 2,478 sessions 07-19 → 10-14), [J] = kernel journal / JetsamEvent reports, [Q] = wrangle queue and launch ledgers.

## 1. What load1 measures, and how it fails here

Facts [K]:
- Linux load = EMA of `nr_running + nr_uninterruptible` (`kernel/sched/loadavg.c:18–27`); D-state tasks count unless `TASK_NOLOAD`/`TASK_FROZEN` (`core.c:6534–6537`). Sample every 5 s + 1 tick; EXP_1 = 1884/2048 → 63% of a step after 59.6 s, 95% after 179.5 s.
- xnu load counts `TH_BUCKET_RUN` threads only (`sched_average.c:288–306`); constants 1884/2014/2036 → same 60 s / 180 s response. No D-state contribution.
- `/proc/loadavg` field 4 is the instantaneous `nr_running()` sum over CPUs (`fs/proc/loadavg.c:20–25`).

Measured [S]:

| | netcup | Mac |
|---|---|---|
| corr(load1/cores, busy fraction now) | 0.58 | 0.14 |
| corr(load1/cores, trailing-mean busy 120–190 s) | 0.82 | 0.76 |
| samples where wrangle says full (load1/cores ≥ 1.5) | 72% | 82% |
| …of those, CPU busy < 80% | 21% | 55% |
| iowait fraction median / p90 | 0.8% / 6.6% | n/a |
| D-state processes seen | jbd2, occasional cargo | n/a |

Conclusion: the D-state/IO story (Gregg's post) is not what breaks load1 on this fleet; iowait is under 1% median. The failure is lag (load1 is a 2-minute trailing mean) plus runnable-thread bursts on the Mac: simulator boots (`qemu-system-aarch64`, `xpcproxy_sim`, `iOS` processes) pushed load to 50.7 on 12 cores while CPU busy was 44% (08:51:41), decaying over 2 minutes. GNU make 4.4 replaced the load average with the instantaneous runnable count on Linux for the same lag reason (`src/job.c` `load_too_high`, NEWS 4.4) [P].

Log evidence of the cost [L]: 105 local and 101 netcup `{"queued":true}` results; the most common refusal was `load 18.25 + 0 reserved ≥ 1.50 × 12 cores` (41 times). Ledger [Q]: only 2 of 12 admitted tickets have a launch record (0.3 and 0.5 min wait); the rest were admitted after cancel/retry, so queue wait cannot be measured from the ledger.

## 2. Who uses the CPU: agents vs builds

[S] per-process samples, trees rooted at pi/claude/codex:

| | netcup | Mac |
|---|---|---|
| agent process cores: median / p90 / max | 0.13 / 0.24 / 1.02 | 0.02 / 0.10 / 0.93 |
| agent process RSS MiB: median / p90 / max | 327 / 383 / 421 | 180 / 279 / 478 |
| active build tree (≥0.5 core) cores: median / p75 / p90 / max | 1.6 / 4.5 / 5.1 / 10.3 | 1.5 / 2.2 / 3.3 / 8.5 |
| build tree RSS MiB: median / p90 / max | 1228 / 3245 / 5894 | 1430 / 2513 / 5168 |
| concurrent build trees: median / p90 / max | 3 / 4 / 14 | 1 / 3 / 5 |
| CPU busy by concurrent trees (1 / 2 / 3 / 4+) | 0.52 / 0.74 / 0.87 / 0.95 | 0.68 / 0.83 / 0.95 / 1.00 |
| top CPU consumers (core-samples) | rustc 3.8, node 2.7, java 1.6, supabase 1.4, pi 1.2 | qemu 1.3, rustc 1.0, pi 0.6, node 0.4 |

Caveat: `ps %cpu` is lifetime-averaged; build trees are short-lived so the figure is near-instantaneous; the agent figure understates bursts but is an accurate mean.

[L] agent activity vs build concurrency (per minute, 09-20 → 10-14): mean concurrent builds ≈ 0.5 × active agents on netcup and 0.33 × on the Mac; build duty cycle (build-seconds per active agent-second) 0.585 netcup (357 build-h / 609 agent-h), 0.349 Mac (293 / 842). P(≥3 concurrent builds | 6 active agents) = 0.56 netcup, 0.34 Mac.

## 3. Concurrent builds hurt (corrected numbers)

[L] 15,245 builds ≥ 20 s (foreground tools + 1,092 `bg_task` builds; the latter were missing from the earlier 3,885 figure). Normalised duration = duration ÷ median duration of the same command with 0 concurrent builds at start.

| concurrency at start | 0 | 1 | 2 | 3 | 4 |
|---|---|---|---|---|---|
| netcup median ratio (n) | 1.00 (2496) | 1.20 (2110) | 1.46 (1416) | 1.68 (949) | 1.98 (547) |
| Mac median ratio (n) | 1.00 (1862) | 1.05 (1226) | 1.15 (908) | 1.18 (549) | 1.03 (341) |

Timeout rate (0 / 1–2 / 3+ concurrent): netcup 4.4% / 7.6% / 13.2%; Mac 6.2% / 7.4% / 9.5%.
Per class on netcup: JS/pnpm slope 0.32 per extra build, moon 0.12, Rust 0.10; Mac Rust 0.16, JS 0.08, moon ≈ 0. "Timeout" = text match; the main matches are tool deadlines (`[Hypa command timed out]` 436, `Command timed out after N seconds` 168, `Execution timed out` 113, `Test timed out` 96), i.e. real failed calls. No SIGKILL/OOM text occurred in any build result.

Since 10-09: 17 `wrangle run` calls vs 712 foreground + 1,211 `bg_task` builds. A slot limit on `wrangle run` would govern < 1% of builds.

## 4. Memory: where it failed and where it did not

- momokaya-2 [J]: 5 OOM kills 2026-10-09 01:00–01:03 in `user-1001.slice` (8 GiB): rustc 3.7 GB + nix 2.7 GB + docker/postgres/openfga; killed openfga, postgres ×3. Earlier kills (08-11 → 08-18) were CI runner tasks at 2–4 GiB limits. The slice today: 6.9 GiB used with 0 agents; `memory.peak` = the 8 GiB limit.
- netcup [J][S]: slice `memory.events` hierarchical `oom 19 oom_kill 12`, but `memory.events.local` `oom_kill 0` and no `Memory cgroup out of memory` in the journal since boot 09-17: the kills were inside child cgroups (containers), none attributable to an agent. Working set median 13.9 GiB, max 20.9 of 48; memory PSI `some avg10` max 0.18. Memory is not a limit on netcup today.
- Mac [S][J][K]: pressure WARN 51% of samples, swap 47.6 of 49 GiB, compressor 10.7–13.8 GiB, swap-ins median 2.9 MiB/min NORMAL vs 5.6 WARN (p90 31 vs 76). WARN = non-compressed available < 50% of available (`vm_compressor.c` divisors 20/25/35; `vm_pageout.c` predicates). Agents hold 3.6 of 29.6 GiB total RSS; the big consumers are OrbStack (1.6 GB), browsers, simulators. The one JetsamEvent (10-11) was a per-process-limit kill of `knowledgeconstructiond`; `kill_on_sustained_pressure_count` = 0. macOS has not killed an agent. `kern.memorystatus_level` is `available_memory × 100 / total_pages` (`vm_pageout.c:5277`), not a pressure enum; the earlier proposal confused the two.
- Wrangle's Mac "available" (`free + inactive + speculative`, `src/probe.rs parse_vm_stat`) was 7–10 GiB during NORMAL and 5–8.6 GiB during WARN: the ranges overlap, so it is not a usable gate alone.

## 5. Scoring candidate rules on the sample [S]

Ground truth per sample: contended = ≥3 active build trees or CPU busy ≥ 95%. Errors: admits-while-contended, refuses-while-free, total.

| rule | netcup | Mac |
|---|---|---|
| load1/cores < 1.5 (today) | 20% / 47% / 28% | 13% / 84% / 55% |
| free cores ≥ 2 (10 s busy delta) | 39% / 16% / 32% | 18% / 18% / 18% |
| free cores ≥ 3 | 25% / 28% / 26% | 15% / 31% / 24% |
| free cores ≥ 4 (= cores/4 on netcup) | 14% / 31% / 19% | 10% / 42% / 29% |
| runnable now ≤ cores (make 4.4) | 23% / 47% / 29% | n/a |
| PSI cpu some avg10 < 75 (Hydra default) | 100% / 0% / 71% | n/a |
| PSI cpu some avg10 < 50 | 68% / 3% / 49% | n/a |

Limits: 29 + 23 minutes, one weekday morning, netcup contended 71% of the time; "contended" is my label. PSI `some` on a 16-core box stays < 60 even with 14 cores busy; it measures waiting share, not spare capacity (`psi.c:243–266`).

## 6. Prior art that applies [P]

- Schedulers (Kubernetes `NodeResourcesFit`, SLURM `cons_tres`, HTCondor ≥ 23 partitionable slots, Bazel `ResourceManager`, Buck2 permits) admit on declared requests against capacity, not live load. Bazel admits a CPU request at 0.6× and records the full value; RAM budget 0.67 × host.
- Live-measure admission exists where jobs cannot declare: GNU make 4.4 (runnable count), Icecream (`1000 − idle‰`, credits own children, plus memory fill), Bazel `--experimental_cpu_load_scheduling` (utilisation + 5 s window of just-started actions at 0.6×), Hydra dynamic mode (CPU PSI some avg10 > 75 refuse; load1 > 8 fallback).
- Memory is gated by pressure/eviction, not admission: systemd-oomd 60% full-PSI for 30 s, kubelet `memory.available < 100Mi`, earlyoom 10% MemAvailable and swap, lmkd stall-ms. KEP-5394 excludes CPU because it is compressible and leaves thresholds to experiment.
- Build tools default to N or N+2 workers (cargo N, Ninja N+2, Gradle N, Vitest N−1, Playwright N/2); none coordinates across processes except via the jobserver.

## 7. Proposal, each value tied to a row above

1. CPU measure: busy fraction over the probe interval. Linux: governing-cgroup `cpu.stat usage_usec` delta ÷ quota cores (netcup slice: median 12.9 of 16 used, 0 throttled periods; quota-aware also excludes pods, which hold cpu.weight priority), fallback `/proc/stat` `1 − (idle+iowait)/total` [K §4]. macOS: `host_statistics(HOST_CPU_LOAD_INFO)` tick deltas, what `top`/`iostat` use [K §5]. `free_cores = cores × (1 − busy) − 1.0 × reservations`. Reservation = 1.0 because one agent's mean demand is 0.585 × 1.6 ≈ 0.9 cores on netcup, 0.35 × 1.5 ≈ 0.5 on the Mac (§2).
2. CPU gate: `free_cores ≥ 4` on netcup, `≥ 2` on the Mac: lowest total error in §5 per host (19% and 18%, vs 28% and 55% today). The single formula `cores/4` gives 4 and 3 (19% / 24%); take it only if one formula is worth 6 points on the Mac. Hosts are ordered by `free_cores` (Icecream/Bazel pattern). The 90 s reservation TTL no longer covers load lag; it covers spawn latency (0.3–0.5 min measured [Q]), so 90 s stays.
3. Memory gate: `free ≥ 3 GiB × (reservations + 1)`; 3 GiB = agent p90 (0.4) + build-tree p90 (2.5–3.2) (§2). Linux: `min(memory.max − working set, MemAvailable)`, working set = `memory.current − inactive_file` as `src/probe.rs` does now (kubelet convention [P]). momokaya-2 would admit at most one agent (6.9 of 8 GiB already used) and would have refused the 10-09 placement that OOM-killed postgres (§4). macOS: refuse while `kern.memorystatus_vm_pressure_level` ≠ 1; consequence: the Mac is ineligible ~50% of the time, caused by the user's apps, not agents (§4). The 1 GiB value in the earlier proposal is withdrawn: it measured idle pi panes only.
4. Per-host build limit: the harm is real (§3) but wrangle launches < 1% of builds. The limit belongs where builds start (the harness `bg_task`/exec path or a repo-level `agent:*` task wrapper); 3 on netcup and 2 on the Mac are where CPU busy crosses 0.87–0.95 (§2) and the duration ratio exceeds 1.5 / 1.15. Outside wrangle's scope; raise separately.
5. Resource log: the 10 s sampler produced the evidence this note rests on in under an hour. `wrangle serve` should append its probe (busy, free cores, working set, pressure level, build-tree count, reservations) to `~/.local/state/wrangle/samples.jsonl` with 7-day rotation, so §5 can be re-scored on weeks, not minutes.

## 8. Rejected

- PSI cpu `some` as a gate: 100% / 68% admit-while-contended at 75 / 50 (§5).
- load1 anywhere, including a macOS `cores − load1` fallback: corr with current busy is 0.14 on the Mac (§1).
- Runnable count (make 4.4): on netcup it refuses while free 47%, same as load1 (§5); the count includes threads of builds that finish within seconds.
- Memory PSI `full` gate on Linux: `mem some avg10` max 0.18 on netcup; nothing to threshold.
- `ru_maxrss` per job: reports the largest child only (`getrusage(2)`).

## 9. Unverified / remaining limits

- Sampler window is short and from one morning; re-score after the 4 h run: `node /tmp/wr_sample_analyze.js /tmp/wr-sample-linux.jsonl /tmp/wr-sample-mac.jsonl` (copy the netcup file first).
- Claude Code / Codex sessions (4 + 2 local, 29 + 35 netcup) and momokaya-2 sessions were not parsed.
- The 10-01 "9 idle pi panes 345–928 MiB" figure is consistent with today's 327 MiB median but was not re-measured on that host.
- Build-tree counts depend on a comm-name regex; unknown build tools would be missed.
- xnu tag xnu-12377.121.6 matches the JetsamEvent kernel string for macOS 26.5; the exact 26.6.2 tag was not confirmed [K].
