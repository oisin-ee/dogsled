# Unit request protocol v1

Dogsled owns execution. Pi-todo owns task policy, claims, and acceptance. These commands do not validate scope, dependency completion, or acceptance criteria.

This implementation follows sections 4, 5, and 8 of the approved `oisinpi` plan at `f3334e3`, with the approved `not_started` correction below.

## CLI

Each command takes `--input '<JSON>'` and always returns JSON. Success exits 0, including a queued receipt. Failure exits 2.

```text
dogsled unit prepare --input '{"origin":<UnitRef>,"placement":{}}' --tool-call-id <invocation-id>
dogsled unit commit --input '{"execution":<ExecutionRef>,"binding":<TaskBinding>,"exported":<TaskExport>}'
dogsled unit observe --input '{"execution":<ExecutionRef>}'
dogsled unit abandon-prepared --input '{"execution":<ExecutionRef>}'
```

`placement` accepts optional `host`, `branch`, and `base`. The caller's current directory supplies cwd. `HERDR_PANE_ID` supplies the owner namespace. `HERDR_WORKSPACE_ID` supplies the existing launch workspace.

Prepare records the originating session and tool-call ID. Retries retain the original invocation identity. They never extend a lease or prove abandonment through elapsed time.

Prepare returns `{executor:"dogsled",ownerId,requestId}`. Commit returns `{execution,origin,disposition,ticket?,host?,pane?}`. Disposition is `queued`, `launched`, or `unresolved`.

Observe returns `{execution,origin,phase,receipt,settlement}`. It reads existing execution facts without dispatch. A launch receipt with an open item and no captured reply projects `running`.

A captured reply is prose, not terminal evidence. A reply, closed item, or missing item projects `unresolved` with no settlement. This unit never projects `returned`. That phase requires the future execution-specific terminal-evidence protocol.

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

## Canonical export encoding

Digest input follows schema declaration order: `version`, `provider`, `contract`, `prerequisites`, then `brief`. It omits `digest`. Nested objects follow their declared field order; arrays retain their order.

Encoding uses compact UTF-8 JSON and lowercase SHA-256 hex. Ordered Rust transport structs produce the projection; generic JSON maps do not. The released `sha2` crate supplies hashing.

`tests/fixtures/unit-exports.json` contains shared encoding fixtures. Both fixtures include Unicode and escaped quotes. The review fixture also includes escaped controls. They exercise both acceptance variants, not the production brief template.

## Dependent work

This unit does not provide a Pi provider bridge, MCP unit input, seeded artifact transport, worker import, or startup acknowledgement. It does not deliver settlements.

Commit uses the existing agent launch path with the frozen brief and selected profile. Managed seeded launches must wait for the transport and import units. Fleet installation remains separate authorization.
