# Metrics

Status: approved, 2026-11-17. Owner decisions from the 2026-11-17 forms are marked (owner).

## Goal

Dogsled shows how often each tool runs, how long it takes, and how often it fails. It also shows how long agents wait for the owner, and how long each stage of an agent's life takes.

The first slice has three metric groups (owner):

1. Tool calls: count, error rate, p50 and p95 duration per tool. Group by tool, repository, model, host, lead, or harness.
2. Owner wait: the time from a question, a plan review, an `ask`, or a `blocked` signal to the owner's answer.
3. Agent lifecycle: queue wait, spawn latency, time to done, and outcome (done, ask, blocked, closed, dropped). Group by profile, model, or harness.

Out of this slice: tokens and cost, CI wait per workflow. Both read the same transcripts later.

## Evidence

A 7-day probe of local pi transcripts (41 sessions, 2026-11-10 to 11-17) found these rows:

| Tool | Calls | Error % | p50 | p95 |
| --- | --- | --- | --- | --- |
| `bash` | 13,095 | 4.5 | 0.8 s | 12.7 s |
| `ci_watch` | 101 | 5.0 | 342 s | 1,814 s |
| `interview` | 109 | 1.8 | 0 s | 2,188 s |
| `mcp__dogsled__close` | 195 | 73.3 | 34 s | 60 s |
| `mcp__dogsled__spawn` | 247 | 19.8 | 0.5 s | 5.6 s |
| `todo` | 906 | 9.5 | 0.1 s | 3.9 s |

Duration is the result entry's timestamp minus the call entry's timestamp. The probe did not read netcup, Claude, or Codex transcripts.

## Decisions

### D1. Transcripts are the one source of tool timing (owner)

`dogsled serve` reads the transcripts of every harness on its host. It does not take live tool events from the Pi extension. One source gives one definition of a duration, covers Pi, Claude, and Codex, and backfills history.

### D2. A separate `metrics.db`, not `state.db` (owner)

Metrics live in `<state dir>/metrics.db`, owned by `src/metrics/store.rs`. The interview said "a table in state.db"; this plan changes that for three reasons:

- Every committed `state.db` write calls `service::wake::notify` (`src/store.rs`, `write`). An indexer that writes each pass would wake the observation loop each pass.
- Metrics are derived data. A metrics schema change can drop and rebuild `metrics.db` from transcripts. A `state.db` schema change empties the work state on every host (`src/store.rs`, `open`).
- Tool calls are the largest row set on a host: about 3,000 rows a day in the probe.

`metrics.db` uses the `state.db` connection settings: WAL, `synchronous=NORMAL` (derived data), foreign keys, and a busy timeout from the `Budget`. Its `user_version` starts at 1. An older version drops every table and recreates the schema; the next pass rebuilds from transcripts. A newer version refuses, as `state.db` does.

### D3. Each host indexes its own transcripts (owner)

The portal merges peer reports the same way it reads peer agents: `ssh <peer> dogsled metrics --json …` through `peer_call` (`src/web/agents.rs`). Durations travel as fixed-bucket histograms, so a merge is exact for counts and gives the same percentile on every host.

### D4. Keep rows 30 days (owner)

Rows older than `jobs::RECORD_KEEP_MS` (30 days) are deleted each pass. The indexer reads only transcript files changed in the last 30 days.

### D5. Plan reviews record their decision time (owner)

`web_reviews` has no decision time. Plan review wait needs it, and `submit_plan` returns at once (`pi/plan-mode.ts`), so its tool duration is not the wait. Add `decided_ms INTEGER CHECK (decided_ms >= 0)` to `web_reviews`, set where the decision is stored (`src/web_store/reviews.rs`), with `CHECK ((decision IS NULL) = (decided_ms IS NULL))`. This bumps `state.db` to version 6. A host with an older store starts empty, as before: the changelog says to let queued work finish before the upgrade.

## Data model (`metrics.db` v1)

```sql
CREATE TABLE transcripts (
  path TEXT PRIMARY KEY NOT NULL,
  harness TEXT NOT NULL CHECK (harness IN ('pi','claude','codex')),
  file_id TEXT NOT NULL,            -- device and inode; a change means a new file
  offset INTEGER NOT NULL CHECK (offset >= 0),  -- end of the last complete line read
  session TEXT,                     -- the harness session ID, from the file
  cwd TEXT,                         -- from the session header or first entry
  repository TEXT,                  -- owner/name from the cwd's origin remote, else NULL
  pane TEXT,                        -- set when observation binds this transcript to a pane
  item TEXT,                        -- the open item on (host, pane), when there is one
  lead TEXT,                        -- see "Attribution"
  indexed_ms INTEGER NOT NULL
) STRICT;
CREATE TABLE tool_calls (
  path TEXT NOT NULL REFERENCES transcripts(path) ON DELETE CASCADE,
  call_id TEXT NOT NULL,
  tool TEXT NOT NULL,               -- the raw tool name; no renaming
  model TEXT,                       -- the model of the entry that made the call
  started_ms INTEGER NOT NULL,
  ended_ms INTEGER,                 -- NULL while the result is not yet written
  error INTEGER CHECK (error IN (0,1)),
  PRIMARY KEY (path, call_id)
) STRICT;
CREATE INDEX tool_calls_started ON tool_calls(started_ms);
CREATE TABLE input_entries (        -- every input the agent received; see "Input entries"
  path TEXT NOT NULL REFERENCES transcripts(path) ON DELETE CASCADE,
  at_ms INTEGER NOT NULL CHECK (at_ms >= 0),
  PRIMARY KEY (path, at_ms)
) STRICT, WITHOUT ROWID;
CREATE TABLE owner_waits (
  id TEXT PRIMARY KEY NOT NULL,     -- question id, review id, or item id + signal event id
  kind TEXT NOT NULL CHECK (kind IN ('question','plan_review','ask','blocked')),
  pane TEXT, repository TEXT, lead TEXT,
  started_ms INTEGER NOT NULL,
  ended_ms INTEGER                  -- NULL while the owner has not answered
) STRICT;
CREATE TABLE runs (
  item TEXT PRIMARY KEY NOT NULL,
  host TEXT NOT NULL, harness TEXT, profile TEXT, model TEXT, role TEXT, lead TEXT, repository TEXT,
  queued_ms INTEGER,                -- ticket created_ms; NULL for a direct launch
  created_ms INTEGER NOT NULL,      -- item created_ms
  confirmed_ms INTEGER,             -- item launch_confirmed_ms
  done_ms INTEGER,                  -- reply captured with reply_done
  closed_ms INTEGER,
  outcome TEXT NOT NULL CHECK (outcome IN ('running','done','ask','blocked','closed','dropped'))
) STRICT;
```

## Indexer (`src/metrics/index.rs`)

One pass:

1. List transcript files under each harness root, with the roots that `transcript::resolve` uses. Skip files not modified in the last 30 days.
2. For each file, compare `file_id` and length with its `transcripts` row. A new `file_id` or a shorter file deletes the row (its calls cascade) and reads from 0. Otherwise read from `offset` to the last complete line.
3. Decode each line with the per-harness entry rules that `src/activity.rs` uses for conversations. Move those rules into one decoder that both callers use; do not write a second parser.
4. A call entry inserts a row with `ended_ms` NULL. A result entry with the same call ID sets `ended_ms` and `error`. `error` follows the `ToolStatus` rule in `src/activity.rs`. That rule marks every Codex `function_call_output` as completed, so a Codex call has `error` NULL. The error rate divides by calls with a known status only.
5. Write the rows and the new `offset` in one `metrics.db` transaction per file.

Call time per harness:

| Harness | `started_ms` | `ended_ms` |
| --- | --- | --- |
| Pi | timestamp of the assistant `message` entry with the `toolCall` block | timestamp of the `toolResult` message with that `toolCallId` |
| Claude | timestamp of the assistant line with the `tool_use` block | timestamp of the user line with the matching `tool_result` |
| Codex | timestamp of the `function_call` line | timestamp of the `function_call_output` line with that `call_id` |

Calls in one assistant entry share a start time. They run in parallel, so their durations overlap. The report sums durations per tool and never presents a sum as wall time.

A call with no result after its transcript stops changing for 30 days is dropped by retention. A call with no result in a live transcript is "running" and is not counted in durations.

### Input entries

The indexer stores the timestamp of every input entry in `input_entries`, so the harvest can find the first input after any signal, however many arrive in one pass. An input entry is:

| Harness | Input entry |
| --- | --- |
| Pi | a `message` entry with role `user`, or role `custom` with `customType` `dogsled` (an inbox delivery, `pi/inbox.ts` `MESSAGE_TYPE`) |
| Claude | a `user` line with at least one content block that is not `tool_result` |
| Codex | a `response_item` whose payload is a `message` with role `user` |

Equal timestamps in one transcript are one row (`INSERT OR IGNORE`).

### Attribution

- `repository`: the cwd's `origin` remote as `owner/name`, read once per transcript. A cwd under `.worktrees/` resolves to its main checkout's remote.
- `pane` and `item`: each pass, the lane copies the pane-to-transcript bindings from the current observation (`observation::details::Session`). It sets `item` to the open item on that host and pane.
- `lead`: the item's `owner_pane`, followed up the owner chain to an item with no owner item, named by its lead record (`src/lead_store.rs`) when it has one. A transcript of a lead is its own lead. A transcript with no binding has `lead` NULL and shows as "unattributed".

## Harvest (`src/metrics/harvest.rs`)

Each pass reads `state.db` (read transaction only) and upserts into `metrics.db`:

- `runs` from `items` with kind `agent`, joined to `tickets` on `tickets.request_id = items.id` for `queued_ms` (`src/work.rs`, `item`, sets the item ID from the request ID). Outcome order: `dropped_ms` set is `dropped`; reply done is `done`; open with `signal_kind` is that kind; `closed_ms` set is `closed`; else `running`. A row stays after retention deletes its item.
- `owner_waits` of kind `question` from `web_questions`: `created_ms` to `answered_ms` (answered or rejected). Withdrawn questions are deleted from `web_questions` and are not counted.
- `owner_waits` of kind `plan_review` from `web_reviews`: the first revision's `submitted_ms` to `decided_ms` (D5).
- `owner_waits` of kind `ask` and `blocked` from items with a signal: `signal_recorded_ms` to `MIN(at_ms)` in `input_entries` for the transcript whose `item` is that item, with `at_ms > signal_recorded_ms`. No such row leaves the wait open. A later pass that finds the row closes it.

## Service lane

Add `Lane::Metrics` to `src/service.rs`. It runs the indexer, then the harvest, then the 30-day delete. A failure records the lane error in `service_lanes`, as the other lanes do. The lane writes only `metrics.db`, so it does not wake observation.

## Report

### Wire types

`portal-types/src/metrics.rs` (M0) holds the report. The server and the portal share it:

- `Histogram`: 17 bucket counts over `BUCKET_BOUNDS_MS` (100 ms to 24 h), `total_ms`, and `max_ms`, with `record`, `merge`, and `quantile`. A quantile is the upper bound of the bucket that holds the rank, capped at `max_ms`.
- `Window` (day, week, month), `ToolGroup` (tool, repository, model, host, lead, harness), and `RunGroup` (profile, model, harness).
- `MetricsReport`: version, window, groups, `until_ms`, `hosts: Vec<HostCoverage>`, `tools: Vec<ToolRow>`, `owner_waits: Vec<WaitRow>`, `runs: Vec<RunRow>`.
- `ToolRow`: tool, group, calls, errors, `unknown_status` (Codex), running, duration.
- `WaitRow`: kind (question, plan review, ask, blocked), answered, open, wait.
- `RunRow`: group, outcome counts, and histograms of queue wait, spawn latency, and time to done.

`group` is `None` for `ToolGroup::Tool`. For the other groups, each `(tool, value)` pair is one row, and a missing value is `"unattributed"`.

### CLI

`dogsled metrics [--window day|week|month] [--group tool|repository|model|host|lead|harness] [--tool NAME] [--json]`

- Without `--json`: three tables (tools, owner wait, runs) with p50, p95, and max in human units.
- With `--json`: one `MetricsReport` for this host only. The portal uses this form for peers.
- Defaults: `--window week`, `--group tool`.

### API

`GET /api/metrics?window=&group=&tool=` in `src/web/metrics.rs`. It builds the local report and asks every peer with `dogsled metrics --json`, in parallel, with the peer timeout the agents API uses. It merges rows with the same key by adding counts and merging histograms. A failed peer is a `HostCoverage` with its error; the other hosts still show.

### Portal page

A "Metrics" page in the portal navigation (`web/src`):

- Header: window selector (day, week, month) and host coverage. A failed host shows its error, as principle 4 in `PRODUCT.md` requires.
- Tools table: tool, calls, error %, p50, p95, max. Sort by any column. A group selector (tool, repository, model, host, lead, harness) and a tool filter.
- Owner wait: one row per kind with answered count, open count, p50, p95.
- Runs: one row per group with outcome counts, and p50 and p95 of queue wait, spawn latency, and time to done.
- Follow `.impeccable/surfaces/web.md` and `DESIGN.md`.

## Units

| Unit | Owns | Depends on | Acceptance |
| --- | --- | --- | --- |
| M0 (lead) | `plans/metrics.md`, `portal-types/src/metrics.rs`, `portal-types/src/lib.rs` | none | Plan merged; `cargo test -p portal-types` passes, with a histogram merge and quantile test. |
| M1 (writer) | `src/metrics/`, `src/activity.rs` decoder move, `src/service.rs` lane, `src/cli.rs` and `src/main.rs` command, `src/web/metrics.rs` and its route, `src/web_store.rs` and `src/web_store/reviews.rs` (D5), `src/store.rs` version, README command list | M0 | The M1 tests below pass; clippy and fmt are clean. At landing, the lead builds the branch once and checks that `dogsled metrics` lists local tool rows and `/api/metrics` returns local and peer coverage. |
| M2 (writer) | `web/src/` metrics page and navigation | M0 | The page renders a recorded `MetricsReport` fixture and the live `/api/metrics` response; window, group, and sort change the table. |

M1 and M2 run in parallel: they share only the M0 types.

## Tests

Keep to behaviour that can break, on real files and a real `metrics.db`.

| Unit | Case | Expected |
| --- | --- | --- |
| M0 | Merge two histograms, then take p50 and p95 | Same as one histogram of all values. |
| M1 | One real transcript per harness, indexed once | One row per call with the start, end, and error of the table above. |
| M1 | A call in one pass, its result appended before the next pass | One row, completed in the second pass; no row read twice. |
| M1 | A transcript replaced with a new file at the same path | Old rows gone, new rows indexed. |
| M1 | An `ask` signal, then two input entries indexed in one pass | The wait ends at the first input entry. |
| M1 | `metrics.db` with an older `user_version` | Tables recreated; the next pass rebuilds rows. |

Checks per unit: `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, and the tests of the changed modules. No full suite before release.

## Later

- Tokens and cost per lead, repository, model, and unit, from the `usage` of each assistant entry. The probe read $2,722 of recorded cost in 7 days on `local`.
- CI wait per repository and workflow, from `ci_watch` arguments.
- Error drill-down: the newest error results of one tool, with a link to the agent.
