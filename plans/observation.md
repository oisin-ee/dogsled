# One observer per host

Status: problem measured 2026-11-04 on v0.32.0. Writer brief O1–O13: `plans/briefs/observation.md`. Three decisions are open (see the brief).

## Problem

On local (12 cores), dogsled's own status reads keep the machine near 100% busy. Admission then refuses work because of that load.

Measured on local:

- `opendirectoryd` used ~230% CPU and `automountd` ~95%. They served ~2,000 directory queries a second, caused by lookups of `/home/...` paths that exist only on remote hosts.
- One `dogsled status --json` made 304 `realpath` calls (240 of them remote `/home` paths) and 1,114 `stat` calls on `state.db`.
- Pausing the 9 herdr sidebars halved the directory queries. Pausing web, serve, and the sidebars cut them from ~2,085/s to ~220–300/s.

## Cause

The bugs are not separate. They all come from one design: every reader of status derives the fleet state again, by itself, on a timer.

| Violation | Evidence (v0.32.0) |
|---|---|
| Many processes compute status, each on its own timer | The portal every 2 s (`src/web/mod.rs:43`, `:323`). Each herdr sidebar every 2 s (`src/sidebar/mod.rs:29`, `:47`), 9 are running. The pi board every 10 s in each pi session (`pi-dogsled/pi/index.ts:17`, `:242`). |
| A status read derives ownership | `commands::status` (`src/commands.rs:24`) → `Fleet::status` (`src/hosts.rs:664`) → `Node::snapshot` (`src/node.rs:323`) → `holds::local` (`src/holds.rs:622`) → `work::settled` for each owned request (`src/work.rs:3275`). Each `work::settled` makes several store reads and validates certificates. |
| A status read writes, under a lock | `Fleet::status` calls `drop::reconcile` (`src/drop.rs:259`). It takes the `branch-checks` ledger lock and persists branch checks. Ten readers serialize on that lock. |
| Equality does filesystem I/O | `PartialEq for Identity` (`src/worktree.rs:35`) calls `resolved()` on both sides. Added in `c759c4e` to fix symlinked-TMPDIR tests on macOS. |
| One host interprets another host's paths | Remote checkout identities (`/home/dev/...`) are resolved against the local disk. Each failed lookup walks every ancestor (`src/worktree.rs:305`). |
| Readers spawn processes | The portal runs `git remote get-url` for each agent on each agent-list request (`src/web/agents.rs:115`). |

## Principles

1. **One writer per host.** `dogsled serve` is the only process that observes the host (herdr, git, filesystem, peers) and derives state (holds, settlement, branch checks). It publishes one versioned snapshot.
2. **Readers only read.** The portal, sidebars, pi board, MCP, and CLI `status` read that snapshot. They derive nothing, take no lock, and write nothing.
3. **Push, not poll.** `serve` derives again when something changes: a herdr socket event, a ledger mutation, or a peer snapshot. Readers get the new revision pushed to them. A timer is only a fallback for missed events.
4. **Facts belong to their host.** Each host derives its own facts. Peers carry them as opaque values. No host resolves another host's paths.
5. **Normalize once, at ingestion.** A path is canonicalized once, on its own host, when it is recorded. After that it is data. `Eq`, `Hash`, and `Ord` are pure.
6. **Validate at the destructive act.** Close, cleanup, and release recheck evidence under their own lock, at the moment they act. Reads show the last derived result and its time.

## Next

An architect maps these principles onto the code. It names the snapshot contract, the change sources that `serve` subscribes to, the read paths to cut, and a writer unit split.
