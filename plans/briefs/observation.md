# One observer per host — writer brief

**Status:** Proposed design. No implementation authorization.

**Source revision:** `68352890fb043a2fee533cc8dc5a4753f99f62a8`. The lead confirmed that `269b1c4` adds only `plans/observation.md`.

**Cause:** Status readers own observation and reconciliation, while `Identity::eq` resolves paths against whichever host performs the comparison.

Evidence: `src/commands.rs:24`, `src/hosts.rs:658`, `src/node.rs:340`, `src/drop.rs:259`, `src/worktree.rs:35`.

## 1. Published snapshot contract

### Ownership

`dogsled serve` owns observation, derivation, and publication. Command handlers retain ownership of explicit mutations and destructive validation.

“One writer” means one **observation publisher**, not one process permitted to update application records.

Retain the service lifetime lock at `src/service.rs:193`. Keep observation separate from action lanes, as the current service already does at `src/service.rs:152` and `src/service.rs:234`.

Introduce a cohesive `observation` module with three boundaries:

| Boundary | Responsibility |
|---|---|
| Collector | Observe local resources and ingest peer publications. |
| Publisher | Derive complete views and replace the published value atomically. |
| Client | Obtain a published value or subscribe to replacements. No collector capabilities. |

Do not give the client a `Shell`, mutable store, or `Fleet`.

### Envelope

Publish one immutable `Published` value containing:

| Field | Contract |
|---|---|
| `version` | Observation protocol version, initially `1`. Independent of database and execution-protocol versions. |
| `epoch` | New identifier for each service instance. Prevents revision reuse after restart. |
| `revision` | Increasing publication number within the epoch. |
| `published_ms` | Publication time, not a replacement for individual observation times. |
| `local_revision` | Changes only when the host’s exportable facts change. Peer-only updates must not increment it. |
| `sources` | Local observation times and each peer’s epoch, local revision, receipt time, and availability. |
| `local` | This host’s complete observation, suitable for peer export. |
| `views` | Complete reader projections derived from the captured inputs. |
| `gaps` | Explicit unavailable, failed, unsupported, or stale domains. Never substitute empty success. |

The `local` section includes the existing snapshot, jobs, handoffs, holds, protocol capabilities, and service-lane diagnostics from `src/node.rs:122–153`.

Extend it with host-owned headroom, todo publications, agent metadata, cleanup facts, and retained-branch observations.

The `views` section contains:

- Full status, including items, queue, orphans, sweep results, holds, and stopped activity.
- Complete boards, including owner-specific projections, titles, cleanup actions, and unread state.
- Portal board, agent list, and inbox.
- Sidebar host classifications and owner findings.
- Owner-filtered job history and the host inventory used by that history.

These are projections, not new authorities for task completion or resource release.

Use host-qualified identifiers for panes, jobs, and imported facts. The existing `"local"` alias is relative to the receiving host (`src/hosts.rs:12–33`).

### Storage and transport

**Recommended design:** Keep the published value in service-owned memory. Keep authoritative records and existing observation inputs in `state.db`.

Expose a host-local Unix-stream endpoint under the state directory:

- `current`: Return the current immutable publication.
- `subscribe`: Send the current publication, then replacements.
- Peer export: Return only the `local` section with its epoch and `local_revision`.

A request holds one publication reference throughout its response. It must not combine sections from different revisions.

Serialize shared wire projections when publishing. Slow subscribers must not block collection or accumulate an unlimited backlog. A subscriber can skip to the newest complete revision.

Do not make readers call `store::read`. Its connection path can initialize or replace a schema and acquire `state-db` (`src/store.rs:64–98`, `src/store.rs:179–224`).

This recommendation needs no new persisted projection schema. That matters because a schema increase currently recreates older stores empty (`src/store.rs:9`, `src/store.rs:77–91`).

### Availability

A cold reader without a running publisher returns **observation unavailable**. It must not probe, start the service, or reconstruct status.

A connected reader that loses the publisher retains its last publication with a disconnected/stale indication. It must not continue presenting that publication as current.

Preserve the existing 30-second host freshness boundary unless separately approved (`src/node.rs:11`, `src/node.rs:155`). Peer reconnects must not renew the observation time of retained facts.

Readers can format text, select an existing projection, and show elapsed time. They cannot classify lifecycle, eligibility, ownership, cleanup, or unread state.

## 2. Observation and change sources

### Local collection

Keep observation rules with their existing owners. Move their **display-time execution** into the publisher:

| Current owner | Required change |
|---|---|
| `Node::tick`, `src/node.rs:359` | Produce local inputs for the publisher. Keep capacity sampling separate from expensive projection work. |
| `observed_probe`, `src/node.rs:696` | Remain the local Herdr, transcript, Git, launch, and handoff observation boundary. |
| `holds::local`, `src/holds.rs:622` | Run once per relevant local evidence change, not per reader. |
| `work::settled`, `src/work.rs:3275` | Keep settlement rules here. Invoke them through observer-owned hold derivation, not status requests. |
| `commands::status`, `src/commands.rs:24` | Move assembly, handoff merging, hold deduplication, and stopped-activity derivation into publication. |
| `board::collect_todos`, `src/board.rs:118` | Consume typed todo inputs during observation and include the result in the same publication. |

Freeze the input set before building a publication. Preserve source timestamps and errors when a collector fails.

Do not hold a database transaction across Git, Herdr, SSH, or transcript reads.

### Wake sources

| Source | Implementation direction |
|---|---|
| Herdr events | Own subscriptions in `serve`, using `Client::subscribe` at `src/herdr_socket.rs:184`. Existing source demonstrates per-pane status events and `pane.created` subscriptions at `src/herdr_socket.rs:664` and `:691`. |
| Ledger mutations | Emit a best-effort wake after a successful, state-changing commit at `src/store.rs:118` and `:224`. Never notify before commit. |
| Peer publications | Maintain one outstanding subscription or bounded long-poll per peer, owned by `serve`. Accept newer source revisions and wake composition. |
| Action completion | Wake affected domains after service-lane completion at `src/service.rs:288`. Keep action scheduling independent. |
| Fallback timer | Reuse `serve --interval`, currently ten seconds by default (`src/cli.rs:38`). Recover missed events and sample resources without event sources. |

Reuse the local wake pattern at `src/service.rs:101–149`; do not make each reader register its own polling timer.

Coalesce wakes by domain. A peer update must not rerun local Git observation. A ledger update must not automatically resample every terminal.

Publication itself must not trigger another observation. Separate producer writes from input invalidation, or consume their wakes without resampling unchanged domains.

Do not invent unsupported Herdr event names. Cwd, removal, transcript, and Git changes without verified event support remain covered by the service fallback.

### Peer facts and branch checks

`Fleet::cached_peer` currently locks, starts SSH, and persists results from the status path (`src/hosts.rs:818–880`). Move that responsibility into the observer.

Peers export only their own facts. Never retransmit an aggregated fleet view as local evidence.

Use `local_revision` for peer subscriptions. Otherwise, two hosts can repeatedly publish each other’s fleet-view changes.

Move `drop::reconcile` out of `Fleet::status` (`src/hosts.rs:676`). Its current implementation locks `branch-checks` and invokes remote branch observation (`src/drop.rs:259–316`).

The replacement has two responsibilities:

1. The execution-host observer measures retained branches and publishes correlated results.
2. The origin-host observer imports those results through the existing guarded update at `src/items.rs:419`.

For branches whose origin retains the only observation request, send a host-scoped observation interest to the execution observer. Carry the path unchanged until that host receives it.

`host refresh-branches` must no longer perform synchronous observation in a helper process (`src/main.rs:722`). Replace its observer use with registration and published results.

Deduplicate branch observation already performed by the handoff lane (`src/done.rs:1005–1034`). Keep destructive handoff checks separate.

Preserve existing failed-observation behavior: failure must not replace the last successful branch result or its measurement time.

## 3. Reader cut map

Each path below must terminate at a projection from one `Published` revision.

### Core and terminal readers

| Reader path and source | Required cut |
|---|---|
| `commands::status`, `src/commands.rs:24` | Return the published status. Remove independent item, queue, sweep, hold, and watchdog reads. |
| `Fleet::status`, `src/hosts.rs:658` | Remove host observation, peer refresh, headroom derivation, and branch reconciliation from this reader API. |
| `Node::snapshot`, `src/node.rs:340` | Return the published host export. Remove hold derivation and `ledger.locked("service")`. |
| Host snapshot and todos, `src/main.rs:1350–1351` | Select both from publication. No separate store reads. |
| CLI status/watch, `src/main.rs:351–377` | Remove `Fleet::discover` and sleep-based refresh. One-shot reads current publication; watch subscribes. |
| CLI board, `src/board.rs:340–371` | Read a published owner board. Preserve explicit open/toggle commands as mutations. |
| Board construction, `src/board.rs:780` | Invoke only during publication, including lifecycle grouping and resource rows. |
| Board enrichment, `src/board.rs:897–1005` | Move inbox reads and ancestry observation to the producer. Publish cleanup and unread results. |
| Sidebar, `src/sidebar/mod.rs:45–116` | Replace its fleet and two-second loop with a subscription. |
| Sidebar host classification, `src/sidebar/model.rs:135` | Publish threshold-derived levels. Keep terminal formatting local. |
| MCP status, `src/mcp.rs:347` | Read the published owner board. Bypass the generic wrapper that discovers a fleet at `src/mcp.rs:251–263`. |
| Hook context, `src/main.rs:514`, `src/context.rs:47` | Consume published status, owner board, and owner findings. |
| Gate board/findings, `src/main.rs:503`, `:564`; `src/gate.rs:31` | Publish the observation portion. Preserve explicit stop requests and reminder-delivery writes as command behavior. |
| Items and jobs, `src/runtime.rs:841`, `:853` | Select published inventory. Remove the extra local `jobs::list` read. |

`Fleet::discover` itself runs `herdr machine list` (`src/hosts.rs:293–300`). Removing downstream derivation without removing reader-side discovery is insufficient.

Service callers also need migration. Watchdog, admission, children, and inbox currently call fleet status independently:

`src/service.rs:182`, `src/work.rs:3303`, `src/sweep.rs:221`, `:232`, `:270`, and `src/inbox.rs:526`.

Give those callers the current publication. Their explicit action paths still revalidate before effects.

### Portal readers

| Reader path and source | Required cut |
|---|---|
| Board background reader, `src/web/mod.rs:278–330` | Replace discovery, status assembly, and enrichment with one observation subscription. |
| `GET /api/board`, `src/web/mod.rs:230` | Return the published board projection. |
| `/api/stream`, `src/web/mod.rs:242` | Forward publication revisions. Do not attach a second derivation loop. |
| Agent list, `src/web/agents.rs:81–121` | Return published agents. Remove Herdr listing, blocked-screen extraction, transcript titles, and Git calls. |
| Agent repository lookup, `src/web/agents.rs:125` | Reuse host-owned facts from `src/facts.rs:243`, not request-time Git. |
| Agent title cache, `src/web/agents.rs:322` | Move transcript inspection and its mutable cache to the observer. |
| Inbox GET, `src/web/inbox.rs:21–56` | Return the published inbox. Do not join a board revision with newer questions, reviews, or agents. |
| Inbox SSE, `src/web/inbox.rs:167` | Delete the per-subscriber timer and store reads. |
| Portal board grouping, `src/web/board.rs:12` | Build the shared wire projection during publication. |
| Browser startup, `web/src/app.rs:239` | Obtain one combined portal publication rather than separately fetched board and inbox generations. |
| Browser agent loop, `web/src/app.rs:265` | Remove independent polling. Update agents with the same published revision. |
| Browser stream decoding, `web/src/api.rs:188` | Apply board, agents, and inbox together from a versioned portal snapshot event. |

Add a shared `PortalSnapshot` wire type in `portal-types`. Existing board, agent, and inbox GETs can remain projection endpoints with revision metadata.

Use one combined stream event for the browser’s coherent update. Separate GETs are independent reads and must not imply transactional consistency between requests.

### Agent-detail reads

The current agents module also observes outside its list route:

| Path | Required strict-boundary treatment |
|---|---|
| Conversation, `src/web/agents.rs:145` | Read publisher-owned, session-qualified conversation data. |
| Screen, `src/web/agents.rs:167` | Read the last published screen and its native revision. |
| Agent events, `src/web/agents.rs:250` | Forward observer-owned events/publications. Remove per-request Herdr subscriptions. |
| Agent lookup, `src/web/agents.rs:308` | Select published agent identity, not another `agent.list` request. |

Use subscription interests to limit detailed collection to requested agents. Collection still belongs exclusively to `serve`.

Store detail content as immutable, session-qualified values referenced by the publication. Do not reread unchanged transcripts or screens for each subscriber.

Do not export conversation content to all peers as part of fleet status.

Prompt, keys, focus, question answers, and review mutations remain explicit commands. They are not snapshot reads.

### Pi readers

| Path | Required cut |
|---|---|
| Widget refresh, `pi/index.ts:242`, `:298` | Replace repeated `dogsled board --json` processes with a publication subscription. |
| Session shutdown, `pi/index.ts:309` | Close the subscription and reject late frames from the previous session generation. |
| Panel status, `pi/panel.ts:153–164` | Reuse the same publication client. Do not create a second observer. |
| Job history, `pi/work-history.ts:18–31` | Consume one published history projection instead of joining three command results. |
| Hook context invocation, `pi/index.ts:356` | Keep the command if needed, but its Rust path must be snapshot-only. |

Separate observation refresh from inbox delivery, unit settlement, and todo publication. The existing `update` function mixes these responsibilities (`pi/index.ts:229–249`).

Those command workflows must not accidentally become dependent on a board notification.

## 4. Identity and destructive safety

### Normalize at the owning host

Replace filesystem-dependent `PartialEq` with structural equality over:

```text
common
gitdir
generation
```

If `Hash` or `Ord` are introduced, derive them from the same stored fields.

The current defect is explicit: `Identity::eq` calls `resolved`, which walks existing ancestors (`src/worktree.rs:35`, `:305`).

Normalize administrative paths at local capture:

- `Guard::discover_identity`, `src/worktree.rs:96–137`.
- `capture`, `src/worktree.rs:534–633`.
- Host-local registration boundaries, including `cleanup::remember` at `src/cleanup.rs:259` and preparation recording at `src/preparation_store.rs:379`.

`capture` already canonicalizes administrative paths at `src/worktree.rs:602–617`. Preserve that behavior and make the constructor invariant explicit.

Keep display/command path spelling separate from administrative identity. Git’s checkout spelling currently remains in `Linked.path` (`src/worktree.rs:624–629`).

Do not canonicalize during deserialization, generic row decoding, equality, or peer ingestion. Row loaders such as `src/preparation_store.rs:329` must preserve recorded values.

Keep explicit host-local equivalence checks separate from wire validation. This distinction already exists between `validate_prepared` and `validate_prepared_host` (`src/prepare.rs:1020`, `:1045`).

### Preserve the symlink regression

Do not merely derive `Eq` and change expected assertions.

The regression at `src/loss_tests.rs:2226` deliberately records aliased administrative paths, then exercises prune and close. It will expose an incomplete change.

Route that local recording scenario through the explicit normalization boundary. Retain successful prune/close behavior and the original safety assertions.

Also retain:

| Regression | Source |
|---|---|
| Same-request alias handling | `tests/cli/launch_retry.rs:163` |
| Execution-host symlink equivalence | `tests/cli/launch_retry.rs:182` |
| Symlinked temporary-root placement | `tests/cli/launch_retry.rs:1917` |
| Same-path, same-HEAD replacement refusal | `src/loss_tests.rs:2874` |
| Missing checkout that reappears | `src/loss_tests.rs:2170` |

Never repair an old identity by adopting a replacement checkout’s generation. Unproven legacy evidence remains unresolved.

### Validate at the destructive act

A published observation is not permission to remove or release anything.

| Act | Existing boundary to preserve |
|---|---|
| Close | Capture before waiting, then revalidate owners, identity, pins, terminal, and current panes under repository protection: `src/close.rs:363–451`. |
| Cleanup | Keep the original requested tree and apply under lifecycle protection: `src/cleanup.rs:2246`, `:2336`. |
| Preparation disposition | Preserve preparation → repository → launch protection and matching evidence: `src/prepare/disposition.rs:584–645`. |
| Reference release | Revalidate certificates and retained host evidence under the launch lock: `src/launch.rs:541`, `:1128`. |
| Placement completion | Preserve preparation and host locking plus exact grant/provenance checks: `src/node.rs:426`. |

Do not replace these checks with cached `settled`, `landed`, `cleanup`, or `eligible` fields.

## 5. Ordered writer units

These are proposed dispatch boundaries, not one oversized assignment.

All units use this repository. Each unit starts from the exact integrated SHA of its prerequisites. The lead records that SHA before dispatch.

Execute the following order because several files overlap. Do not run overlapping writers concurrently.

### Foundation

| Unit | Deliverable and owned paths | Acceptance command |
|---|---|---|
| O1 — Normalize local identity | `src/worktree.rs`, local recording paths in `src/cleanup.rs`, `src/host_store.rs`, `src/preparation_store.rs`; associated identity regressions. Pure equality with preserved symlink behavior. | `cargo test --bin dogsled worktree:: && cargo test --bin dogsled loss_tests::symlinked_home_preserves_identity_through_prune_and_close` |
| O2 — Publish immutable observations | New `src/observation.rs`, `src/observation/ipc.rs`, publication tests; module and command registration in `src/main.rs`, `src/cli.rs`. Current/subscribe protocol, epochs, replacement, and shutdown. | `cargo test --bin dogsled observation::tests::publication` |
| O3 — Collect local state once | `src/observation/collect.rs`, `src/service.rs`, `src/node.rs`, producer portions of `src/commands.rs`, `src/holds.rs`, `src/work.rs`. One local evidence owner without reader fallback. | `cargo test --bin dogsled observation::tests::local` |
| O4 — Wake observation from changes | `src/store.rs`, `src/service.rs`, `src/herdr_socket.rs`, `src/observation/collect.rs`, `tests/cli/service_lanes.rs`. Commit wakes, Herdr events, coalescing, and fallback. | `cargo test --test cli service_lanes::` |
| O5 — Exchange host-owned facts | `src/hosts.rs`, `src/drop.rs`, `src/items.rs`, observer-related branch work in `src/done.rs`; peer adapters in `src/main.rs`, `src/cli.rs`; peer tests. | `cargo test --bin dogsled observation::tests::peer` |

O5 must include missing-mark branch observation, stale peers, unsupported versions, and prevention of publication feedback loops.

### Projection and reader migration

| Unit | Deliverable and owned paths | Acceptance command |
|---|---|---|
| O6 — Build shared projections | `src/observation/project.rs`, `src/board.rs`, `src/facts.rs`, `src/gate.rs`, `src/web/board.rs`; producer extraction from `src/web/agents.rs`, `src/web/inbox.rs`, `src/web/store.rs`, `src/web/reviews.rs`; shared portal types. | `cargo test --bin dogsled observation::tests::projection` |
| O7 — Cut CLI and MCP readers | `src/main.rs`, `src/cli.rs`, `src/mcp.rs`, reader portions of `src/commands.rs`, `src/hosts.rs`, `src/runtime.rs`, `src/board.rs`; new `tests/cli/observation.rs` and its registration. | `cargo test --test cli observation::readers` |
| O8 — Cut hooks and lane rederivation | `src/service.rs`, `src/work.rs`, `src/sweep.rs`, `src/inbox.rs`, `src/watchdog.rs`, `src/context.rs`, `src/gate.rs`, hook routing in `src/main.rs`; context/lane tests. | `cargo test --test cli context:: && cargo test --test cli service_lanes::` |
| O9 — Serve one portal revision | `src/web/mod.rs`, `src/web/agents.rs`, `src/web/inbox.rs`, `portal-types/src/`, `web/src/api.rs`, `web/src/app.rs`; portal transport tests. Remove browser agent polling and per-client inbox polling. | `cargo test --bin dogsled web:: && cargo test -p portal-types` |
| O10 — Centralize agent-detail observation | `src/observation/details.rs`, `src/observation.rs`, `src/web/agents.rs`; necessary extraction in `src/activity.rs`, `src/transcript.rs`; detail/session tests. | `cargo test --bin dogsled observation::tests::details` |

O6 does not redesign lifecycle or cleanup rules. O8 does not remove destructive revalidation.

O10 preserves current conversation and screen behavior. It must distinguish empty, unavailable, and failed reads without attributing old content to a new session.

### Remaining consumers and proof

| Unit | Deliverable and owned paths | Acceptance command |
|---|---|---|
| O11 — Subscribe the sidebar | `src/sidebar/mod.rs`, `src/sidebar/model.rs`, sidebar tests. No fleet discovery or threshold classification in the reader. | `cargo test --bin dogsled sidebar::` |
| O12 — Subscribe Pi consumers | New Pi observation client and tests; `pi/index.ts`, `pi/board.ts`, `pi/panel.ts`, `pi/work-history.ts`, associated tests. Preserve command workflows independently. | `pnpm run --silent typecheck && node --test pi/index.test.ts pi/board.test.ts pi/work-history.test.ts pi/observation.test.ts` |
| O13 — Prove the complete boundary | `tests/cli/observation.rs`, focused integration fixtures, and regression tests only. No new architecture or unrelated repairs. | `cargo test --test cli observation::` |

The proposed observation test filters must execute nonzero tests. A successful command that selects no tests is not acceptance.

### Writer limits

Writers may run formatting and their focused acceptance commands. Preserve required hooks from `.moon/workspace.yml:19–21`.

The lead owns the full `moon run dogsled:check` gate, defined at `moon.yml:137`, plus WASM portal checks.

Generated outputs: none hand-edited. The lead runs the repository-owned web and todo generators required by the existing check tasks.

Publication authority: local commits only unless separately authorized. No push, PR, merge, release, installation, or fleet rollout is granted by this brief.

### Integrated acceptance

Require these five regression groups:

1. **Reader isolation:** Hold observation-related lock files and make Git, Herdr, SSH, and database access unavailable to reader processes. Every listed reader still returns the supplied publication without those accesses.
2. **Publication consistency:** Concurrent updates never mix revisions. Reconnect, restart, dropped notifications, slow readers, and unsupported versions preserve explicit availability.
3. **Host ownership:** Foreign paths survive transport unchanged. Local filesystem changes cannot alter their equality or cause local path resolution.
4. **Safety:** A safe publication followed by dirty work, replacement identity, changed ownership, or changed release evidence cannot authorize a destructive action.
5. **Scaling:** Adding sidebars, browser streams, MCP readers, and Pi sessions does not increase observation, settlement, branch-check, Git, or Herdr call counts.

Retain the symlinked-TMPDIR regressions on Linux and macOS. Do not claim native macOS proof from a Linux-only run.

### Decisions before dispatch

| Decision | Recommendation |
|---|---|
| Cold reads while `serve` is absent | Return explicit unavailable. Do not add reader-side reconstruction. Durable offline display requires a separate storage design. |
| Agent conversation/screen scope | Include O10 to satisfy the strict observer boundary. Excluding it requires an explicit content-reader exception to the acceptance criterion. |
| Herdr event coverage | Verify supported event filters before O4. Keep the service fallback for unsupported changes without modifying dependencies. |

**Review evidence:** Source inspection only. No files or Git state changed. `git diff --check` passed. Tests were not run.

**Next action:** Resolve the approval gates, save this text as `plans/briefs/observation.md`, and dispatch O1 from a recorded base SHA.
