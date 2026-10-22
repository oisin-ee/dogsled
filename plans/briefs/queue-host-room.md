# Why queued tickets wait "for a host with room" on an eligible host

**Outcome.** A findings report that names, with file:line and live evidence, why pending tickets stay `pending` while `dogsled status` says their pinned host is eligible, plus a fix design a writer can implement. You edit nothing.

**Observed (2026-10-22 ~10:43Z, this machine, dogsled 0.22.0).**
- `~/.local/state/dogsled/queue.json` held 5 `pending` tickets, each with `retry.attempts == 0`, `next_ms == 0`, no `error`: four pinned to `local` (503–1063 s old) and `r-1792665512037-1708-5` pinned to `netcup` (316 s old).
- `dogsled status` at the same time: `local` eligible (busy 3.4/12), `netcup` eligible (busy 10.5/16). Only `momokaya-2` had a reason (memory).
- The row text "for a host with room" is the fallback in `src/board.rs:682` when no host has a headroom reason, so status itself does not know why the ticket waits.
- `queue.json` was last written at the newest ticket's creation; no ticket changed state after that.
- The latest `sweep.json` report took 41 s (`started_ms` 1792665619269, `finished_ms` 1792665660269).
- `unit_request::refresh` (`src/unit_request.rs:437`) takes ~22 s per call on this ledger, because it runs one sequential SSH call per remote unit. The service runs it in `inbox::tick` (`src/inbox.rs:965`) and `sweep::children` (`src/sweep.rs:210`). A separate writer is scoping it on `fix/unit-settlements-scope`.
- Later: the netcup ticket was admitted at ~10:59Z. Ticket `r-1792666081842-1708-11` (any host) first failed with `launch failed: ssh netcup: other`, then launched on retry. Another lead's brief (`plans/briefs/unit-refresh-ssh-fanout.md` on the Mac) reports netcup ssh failing with `kex_exchange_identification: read: Connection reset by peer` (sshd MaxStartups) and stuck `dogsled host unit-receipt` ssh sessions. Check whether the refresh SSH fan-out also starves or breaks admission SSH.
- The service is launchd `dev.mise.dogsled` (`mise x -- dogsled serve`), pid 33652 at the time.
- Four leaked test servers run from worktree debug builds with temp `HOME`s, 15–19 h old: pids 69612, 73631, 74347 (`.worktrees/fix-herdr-plugin-rust`, `stop-*` rigs) and 80636 (`.worktrees/busy-retry`, `sweep-worktree` rig). Earlier `local` reported 91% busy.

**Questions.**
1. For each pending ticket, which path keeps it pending: `admit` (`src/work.rs:805`) is not reached, `perform` returns `Ok(None)`, the `full` list skips it, or the ticket is never claimed?
2. If `perform` says no room, why does status say eligible? Compare the two headroom checks.
3. Does the serve loop run admission in the same thread or loop as the inbox tick and the sweep, so a 22–41 s refresh starves it? Find the loop (`grep -rn "fn serve" src`).
4. Do the leaked test servers use real CPU and skew `local` headroom? Which tests leak them, and why does their rig not stop them?
5. Is the cause the same as the slow `refresh`, so that the settlements fix also fixes this, or is it separate?

**Method.** Read the code above and the headroom code in `src/hosts.rs`. The Mac's live facts are recorded above; you may run on another host. On your host, use `sample`/`ps`, `dogsled status --json`, `dogsled doctor --host <label>`, and the ledger under `~/.local/state/dogsled/` (read only); netcup's own service and sshd logs are relevant to question 5. Do not kill processes, cancel tickets, or write to the ledger. If you need a reproduction, use a test rig with its own `HOME`.

**Reply.** Cause with file:line, the evidence for each question, a fix design (files, the change, a test), and whether it depends on `fix/unit-settlements-scope`.

**Base.** `origin/main` at `4e2be386b8926a40afd4e585918a15339cf6151d`.
