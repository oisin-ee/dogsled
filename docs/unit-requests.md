# Unit request protocol v1

Dogsled owns execution. Pi-todo owns task policy, claims, and acceptance. These commands do not validate scope, dependency completion, or acceptance criteria.

This implementation follows sections 4–8 of the approved `oisinpi` plan at `c20b8e2`, including its `not_started` correction.

## CLI

Each command takes `--input '<JSON>'` and always returns JSON. Success exits 0, including a queued receipt. Failure exits 2.

```text
dogsled unit prepared
dogsled unit prepare --input '{"origin":<UnitRef>,"placement":{}}' --tool-call-id <invocation-id>
dogsled unit commit --input '{"execution":<ExecutionRef>,"binding":<TaskBinding>,"exported":<TaskExport>}'
dogsled unit observe --input '{"execution":<ExecutionRef>}'
dogsled unit abandon-prepared --input '{"execution":<ExecutionRef>}'
```

`placement` accepts optional `host`, `branch`, and `base`. The caller's current directory supplies cwd. `HERDR_PANE_ID` supplies the owner namespace. `HERDR_WORKSPACE_ID` supplies the existing launch workspace.

Prepare records the originating session and tool-call ID. Retries retain the original invocation identity. They never extend a lease or prove abandonment through elapsed time.

Prepare returns `{executor:"dogsled",ownerId,requestId}`. Commit returns `{execution,origin,disposition,ticket?,host?,pane?}`. Disposition is `queued`, `launched`, or `unresolved`.

Observe returns `{execution,origin,phase,receipt,settlement}`. It reads existing execution facts without dispatch. A launch receipt with an open item and no captured reply projects `running`.

A captured reply is prose, not terminal evidence. Without a terminal receipt, a reply, closed item, or missing item projects `unresolved`. Only the execution-specific protocol below permits `returned`.

Errors contain `{error:{code,message}}`. Transport conflicts use `REQUEST_CONFLICT`; local profile refusals use `PROFILE_INVALID`. Other executor failures use `LAUNCH_UNRESOLVED` and never release a claim.

## Durable transitions

Prepare atomically writes `admissions/<requestId>.json` before returning. An owner-scoped lock serializes origin lookup and ID allocation. The lookup scans admission records, without another index.

The same owner and complete `UnitRef` return the same request. Changed placement conflicts. Prepare never reserves capacity, creates a checkout, or starts a worker.

Commit and abandonment use the same request lock. Commit persists the exact binding, export, and launch request before admission. An identical commit returns the existing receipt or ticket. Changed payload conflicts, even when its digest field remains unchanged.

The existing admission service recovers committed records without tickets. Queue insertion deduplicates by request ID. Interrupted unit admission before host selection remains recoverable. Interrupted preparation after host selection retains existing refusal and inspection protections.

Commit enters `runtime::start_locked`, which shares the normal start machinery and calls `work::perform_locked`. It does not reacquire its admission lock. Existing standalone commands still use the locking entry point.

New admission fields are optional when reading old records. Existing host-selected records, launch receipts, retry policy, terminal identity checks, and cleanup rules remain intact.

## Abandonment and claim recovery

Abandonment writes a permanent terminal event before returning. A later commit fails `REQUEST_CONFLICT`. A committed request cannot be abandoned. The service never abandons prepared requests automatically.

After an adapter crash, the integration must confirm invocation completion or session replacement before explicit abandonment. An uncertain invocation retains its prepared request. A timeout alone is not proof.

The approved correction makes `not_started` independent of a binding digest:

```json
{
  "version": 1,
  "eventId": "<requestId>-not-started",
  "origin": {"sessionId":"s","unitId":"u","revision":1,"contractEntryId":"e"},
  "execution": {"executor":"dogsled","ownerId":"owner","requestId":"request"},
  "result": {"kind":"not_started","reason":"prepared request abandoned before commitment"}
}
```

Dogsled cannot know the exported digest after an external claim and before commit. This event needs only the durable prepared record. Repeated abandonment and observation return the same event.

The provider must match execution and the complete origin before releasing a held claim. No matching claim produces no task mutation. A stale origin requires retained delivery. Returned and stopped settlements still require the original binding.

## Launch that never happened

A committed unit whose ticket ended `failed` or `cancelled` settles as `not_started`, with the ticket and its error as the reason. This needs the saved admission to hold no launch request and no result, so no worker can have started. The event is written under the admission lock by refresh, `unit observe`, and `unit settlements`. Delivery and acknowledgement follow the abandonment events above. A pending ticket, or a failure after a launch request was saved, stays `unresolved` until host evidence settles it. Such a unit cannot be committed again.

## Canonical export encoding

Digest input follows schema declaration order: `version`, `provider`, `contract`, `prerequisites`, then `brief`. It omits `digest`. Nested objects follow their declared field order; arrays retain their order.

Encoding uses compact UTF-8 JSON and lowercase SHA-256 hex. Ordered Rust transport structs produce the projection; generic JSON maps do not. The released `sha2` crate supplies hashing.

`tests/fixtures/unit-exports.json` contains shared encoding fixtures. Both fixtures include Unicode and escaped quotes. The review fixture also includes escaped controls. They exercise both acceptance variants, not the production brief template.

## Seeded launch and MCP

The existing MCP spawn accepts exactly one shape:

- `{unit:{id},host?,branch?,base?}` uses the Pi bridge. Without that bridge, the handler returns `PROVIDER_UNAVAILABLE`.
- `{preparedUnit:{execution,binding,exported}}` calls commit in the handler. Placement comes from prepare.
- The existing explicit `{name,task,profile?,kind?,host?,branch?,base?,cwd?}` remains available.

Mixed shapes and unknown keys fail. No interception hook launches a process. The Pi bridge matches prepared envelopes to actual tool-call IDs, including nested codemode IDs.

The selected host writes `launches/<requestId>.unit.json` beside the task artifact. Its exact body is `{version:1,execution,binding,exported}`. Atomic ledger writes persist it before startup. Replays preserve the same artifact. Changed artifacts fail.

Managed arguments add `--todo-unit-file <artifact>`, `--session-dir <private-directory>`, and `--session-id <requestId>`. The directory is `launches/<requestId>.sessions/`. Profile flags remain unchanged. Profiles with continue, resume, session, fork, no-session, seed, or argument-terminator flags fail explicitly.

Before the first allocation, an existing session directory fails. Before startup, a nonempty directory fails. Unknown startup outcomes retain the existing receipt protections. Dogsled never resets a session to make a retry work.

Pi 1.3.0 documents opaque session IDs and private-directory lookup in `docs/cli.md`. Its session format uses immutable IDs and parent links. Live Herdr argument forwarding and compatible importer startup remain integration proof, not claims from these Rust tests.

## Worker terminal acknowledgement

The worker adapter calls this command on its execution host:

```text
dogsled unit terminal-ack --input '{"execution":<ExecutionRef>,"unitRef":<UnitRef>,"digest":"...","sealEntryId":"...","sessionLeafId":"..."}'
```

The input has no caller-supplied transcript path. `HERDR_PANE_ID` must match the host launch receipt. The current terminal, Pi harness, and agent name must also match. Herdr supplies the transcript path.

The transcript must be a regular JSONL file directly inside the private session directory. Symlink files and directories fail. Its v3 header must match the request ID and launch cwd, without a parent session.

Dogsled walks parent links from the acknowledged leaf. Metadata entries can follow the final assistant reply. Substantive conversation after that reply fails. The final reply must contain text, stop normally, and contain no tool call. Its ancestry must reach the acknowledged seal entry. A reply before the seal, or on another branch, cannot match.

Dogsled validates the seal entry's identity and ordering, not task state. The provider owns seal meaning and immutable local activation. The adapter obtains the seal from `imported()` at `agent_settled`.

### `done` before the unit returns

A seeded worker can call `dogsled done` before its unit is sealed. The worker host does not treat that mark as finished until the launch has a terminal receipt. Until then, the pane stays open, `done` is false in the host snapshot, and the board shows the worker as waiting.

Only the execution owner (`execution.ownerId`) can continue such a worker. The owner sends a message or a task to the same pane. The worker host then removes the mark, and the same execution continues in the same session, worktree, and branch. The host refuses input from any other sender, and the worker's own inbox claim, and changes nothing. A mark sealed by an earlier release without a receipt follows the same rule.

Before the host accepts input, it binds the pane to exactly one seeded launch. The done mark, terminal, agent name, durable seed, and receipt must agree. When the evidence is missing, unreadable, competing, or mismatched, the host refuses the input and does not close the pane. After the receipt exists, the host refuses all input, and the normal handoff closes the pane.

Evidence reads reject malformed, partial, duplicate, cyclic, missing, or oversized data. Limits are 32 MiB per transcript, 1 MiB per line, and 100,000 entries. Failed reads retain the binding. The receipt atomically stores acknowledgement, exact reply text, reply entry ID, and session ID. Repeated acknowledgement returns that receipt. Changed acknowledgement fails.

The peer command `dogsled host unit-receipt --input '{"execution":<ExecutionRef>}'` reads the durable receipt after pane closure. Existing inbox and child refresh paths fetch it. The admission record retains the full receipt and a deterministic returned event before the item update. Refresh repairs the item from that receipt, even after settlement delivery or host unavailability. If the item does not exist yet, later refresh repairs it after registration. Identical replay preserves reply timestamps and notification acknowledgement. Contradictory receipts or correlated item replies fail explicitly. The result ID combines request and reply entry IDs. Evidence summaries contain at most 2,048 characters.

Managed children without terminal evidence retain automatic child-close protection. Idle, closed, missing-process, and missing-probe observations never emit a settlement. This implementation has no positive termination receipt, so it never emits `stopped`. A future termination owner must prove termination before adding that outcome.

## Settlement delivery

`dogsled unit settlements` returns the owner's pending `Settlement[]`. The query refreshes host evidence but starts no worker.

`dogsled unit ack-settlement --input '{"execution":<ExecutionRef>,"eventId":"..."}'` returns `{applied:boolean}`. The bridge calls it only after provider success or confirmed duplicate application.

The acknowledgement lives in the admission record, separate from inbox text acknowledgement. Missing providers, stale origins, and lost acknowledgements retain the same event indefinitely. Query and observation do not consume delivery. Duplicate acknowledgement is a no-op. No bookkeeping-only model turn is necessary.

Returned events carry the original TaskBinding. Not-started events carry origin and execution, without a fabricated digest. Observation retains the event after delivery acknowledgement.

## Pi bridge

The bridge requires Pi SDK and TUI 1.3.0-compatible peers and Node 22.19.0 or newer. Development tests pin Pi 1.3.0.
Real-runtime tests on the former Pi 1.0.4 dependency showed that aborting in `before_agent_start` did not prevent the initial model request.
The bridge does not patch that runtime or weaken the guard assertion.

The existing spawn hook acquires `oisin.tasks.acquire.v1`, inspects the unit, validates checkout identity, and prepares before claiming. The MCP handler alone commits.
The checkout origin uses canonical lowercase host and case-preserved repository path, without `.git`. SSH and HTTPS clones identify the same repository.
The bridge supplies the current session cwd to preparation. It does not repeat the brief, profile, or acceptance requirement.
A pasted `preparedUnit` cannot bypass preparation: the exact envelope must belong to the current runtime invocation.

Actual `tool_execution_end` permits observation and abandonment of a still-prepared request. A later guard may block without emitting `tool_result`.
Nested calls use their own invocation IDs. An ended duplicate cannot abandon an execution while another known invocation remains active.
Unknown outcomes retain the binding. The existing settled boundary retries failed recovery; no timeout proves termination.
The read-only `unit prepared` command lists this owner’s uncommitted requests with origin and invocation ID from existing admission records.
After a fresh runtime boundary, the adapter abandons only current-session preparations without a known live invocation. Other sessions remain held.
Recovery uses the existing refresh lifecycle and also runs before the first task request or managed spawn. It never commits or redispatches work and needs no model setup call.
Commit and abandonment retain their request lock: a concurrent committed request cannot be abandoned. Unknown outcomes remain pending.

Dogsled alone registers `--todo-unit-file`, including when pi-todo is missing. Pi scopes `getFlag()` to its registering extension and rejects duplicate flag registration.
During extension loading, Dogsled registers `oisin.tasks.seed-source.v1`. Pi-todo queries it synchronously at startup with `{version:1,reply}`.
The response is `{version:1,path}`, where a null path means no managed launch. Pi-todo rejects malformed or ambiguous responses and owns file parsing and import validation.
All extension factories finish before startup handlers run, so the query works in either extension order. The capability transports no task mutation or dispatch operation.
The loaded worker guard validates the import even if pi-todo is missing. It validates `imported()` synchronously in `before_agent_start` and calls `ctx.abort()` on failure.
Fresh receipts must match the launch execution, origin, digest, and current request/session ID. On resume or reload, pi-todo’s recorded receipt owns progress.
A recorded import does not require the original seed artifact or launch flag. Sealed workers cannot begin another model task.
Only a correlated seal at `agent_settled` causes terminal acknowledgement. Dogsled still validates the final transcript ancestry.

Settlement delivery runs through the existing refresh and settled lifecycle. Provider failure or a wrong branch leaves the event pending.
Successful duplicate application acknowledges the same event. Inbox text delivery remains independent. No model turn starts solely for bookkeeping.

## Cross-repository acceptance

With a built first-party pi-todo checkout, run:

```sh
PI_TODO_SOURCE=/path/to/oisinpi/packages/pi-todo/src npm test
```

`pi/unit-contract.test.ts` loads both actual extensions through `DefaultResourceLoader` in both orders. It also loads Dogsled alone and checks zero initial model requests for a managed seed without a provider.
The flow tests use the real provider, WASM core, importer, bridge, and Pi runtime. One unit spawn exports the contract, activates the worker, seals completion, and delivers evidence without accepting the lead task.
Executor transport is simulated in these tests; they do not prove managed launch, deployed packages, or live terminal receipt discovery. The separate live scenario remains required.

## Proof limits

Pi runtime tests exercise direct and nested later-hook blocks and aborts, and the fresh-session guard with missing, invalid, valid, and sealed receipts.
The zero-request guarantee excludes resume-time compaction and both extensions failing to load. Import acknowledgement proves activation, not initial disk durability.
Compatible deployed packages and a live local worker scenario remain integration work. This implementation does not install or dispatch workers. Fleet installation requires separate authorization.
