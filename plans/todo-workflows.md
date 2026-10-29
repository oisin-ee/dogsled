# Structured todo workflows

Status: approved in Plannotator for implementation. The user selected the existing launch path with explicit startup limits. Execution progress lives in the todo tool, not this document.

## Context

The user wants the orchestrator to decompose work into bounded units before execution. Assignments use profiles, not per-unit model, effort, or prewalk overrides. Profiles supply those execution settings. Later, teams and personas will define task-specific capabilities and restrictions.

The current missing contract belongs in `todo-core` and `pi-todo`. Tasks store content, status, and optional children, but no assignment or execution policy. Work-unit sizing and delegation remain model instructions.

## Current evidence

- `crates/todo-core/src/lib.rs` defines two-level tasks and validates names and parent closure. It requires explicit parent completion after children close.
- `packages/pi-todo/src/tool.ts` applies Rust operations and persists accepted snapshots. `src/state.ts` reads the current session branch.
- `packages/pi-todo/src/guidance.ts` describes bounded units and delegation. These instructions do not validate scope or execution assignments.
- `packages/pi-todo/src/plannotator.ts` continues approved plans through the existing todo tool. It does not create or validate assignments.
- Prewalk and guardrail source inspection confirms separate post-edit handoff and pre-execution policy hooks. Neither currently enforces task assignments.

The relevant Hindsight knowledge page has no substantive content. Current conclusions use local source instead.

## Approach

Extend todo into a typed work contract, while keeping process execution with its existing owner. Teams and persona permissions are outside this iteration.

A work unit describes its deliverable, scope, acceptance condition, dependencies, and execution assignment. A delegated assignment references an existing profile. The unit does not duplicate editable model, effort, or prewalk settings. Runtime observations can record those settings as evidence, not overrides. Persona instructions and actual permissions remain distinct.

The orchestrator proposes units and assignments. Deterministic validation rejects incomplete units and invalid transitions. Runtime gates enforce the approved execution policy at the action boundary. Prompt instructions alone cannot provide enforcement.

A runtime can require scope and acceptance evidence. It cannot prove from a task title that the decomposition is sensible. Review policy must address that limitation.

Use stable unit IDs for dependencies and worker correlation. Names remain display labels. Pi-todo owns the unit contract and desired assignment. Dogsled owns execution attempts, transport, and worker lifecycle. A task can hold a Dogsled reference, but it cannot maintain a second execution state machine.

Pi-todo validates and freezes the contract at handoff. Dogsled mechanically launches the worker and associates results. The worker's pi-todo imports and activates the contract. Neither agent recreates this bookkeeping. Scope changes require a new task revision, not edits to historical evidence.

### Responsibility boundary

| Owner         | Responsibilities                                                                                                                                                 | Explicit exclusions                                                                                                                                          |
| ------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Pi-todo       | Unit IDs, scope, acceptance, dependencies, desired self/profile assignment, branch-local task state, active-unit gate, contract export/import, acceptance review | No spawn operation, profile loading, process launch, queue, runtime attempt ledger, host selection, polling, worker reconciliation, cancellation, or cleanup |
| Dogsled       | Profiles, dispatch, host/worktree placement, queue, execution IDs, launch recovery, transport, worker facts, inbox/results, cancellation and cleanup             | Does not decide decomposition, interpret acceptance conditions, or mark a deliverable accepted                                                               |
| Pi-prewalk    | Planning-to-execution model handoff                                                                                                                              | No task or process lifecycle ownership                                                                                                                       |
| Pi-guardrails | Existing shell/edit safety policies                                                                                                                              | No duplicate task tracker or worker manager                                                                                                                  |

The Dogsled Pi adapter owns the integration: obtain a validated task contract, launch through Dogsled, and deliver associated evidence back to pi-todo. Pi-todo has no Dogsled CLI/MCP client. No `todo dispatch` or `pi-todo/src/execution.ts` is introduced.

Future teams can select assignments through the same contract. Future permissions can constrain execution at its owner. Neither requires moving process responsibilities into pi-todo.

Teams can later provide reusable role assignments and workflow stages. They must not introduce another task database or a separate model registry.

### Alternatives considered

| Approach                         | Assessment                                                                                              |
| -------------------------------- | ------------------------------------------------------------------------------------------------------- |
| Stronger todo instructions only  | Small change, but does not satisfy forced workflow requirements.                                        |
| Typed tasks and start validation | Recommended foundation. Enforces metadata and lifecycle, but not actions outside todo.                  |
| Typed tasks plus runtime gates   | Selected. Enforces a local unit prerequisite and structured handoff without adding persona permissions. |

### Confirmed product decisions

- All implementation requires a work contract. Genuinely small work can remain one unit.
- The orchestrator proceeds within predefined rules without approval for each assignment. Existing user approval boundaries still apply.
- The orchestrator can execute an explicitly assigned unit itself. An active unit does not require a subagent. Bounded work is the primary goal, not mandatory dispatch. Existing guidance about delegating independent units remains separate.
- Teams, personas, and their permissions are outside this iteration. Assignment and execution boundaries must permit their later addition.

### Confirmed integration scope

This iteration includes the required changes in the owned Dogsled repository. Managed workers use Pi only. Claude and Codex remain outside this workflow contract.

Keep execution bindings separate from task content so additional harnesses and teams can follow. Do not create profile files dynamically or parse Dogsled private state from pi-todo.

Future persona permissions must not become a hidden dependency of this first release.

### Verified integration constraints

`/Users/oisin/dev/dogsled/src/profiles.rs` already owns profile loading and `Profile::pi_flags()`. Profiles supply model, thinking, prompt, and `pi_args`. The local `general` profile uses `alias/sol` with medium thinking, while `astra` uses `alias/astra` with high thinking.

`dogsled/src/work.rs::Action::Agent` accepts harness, name, message, and profile. `agent_args()` rejects profiles for Claude and Codex. `dogsled/src/mcp.rs::SpawnArgs` exposes the same profile-only selection. Per-unit overrides therefore require an owned launch-contract change, not merely extra todo fields.

`packages/pi-prewalk/src/flags.test.ts` verifies `--no-prewalk` and `--prewalk-into`. The target flag selects the model, but does not independently specify target thinking. Profile-only assignments preserve these existing semantics and configuration dependencies. Do not claim that the profile alone specifies target thinking or expose a new per-unit override.

`crates/prewalk-core/src/protocol.rs::turn()` selects the target after an edit/write result, even when that edit failed. `packages/pi-prewalk/src/index.ts::boundaries()` processes completed turns, not pre-execution calls. Therefore, current prewalk does not guarantee that the execution model authors the first edit. Preserve that distinction in the plan. Do not silently redefine prewalk as a pre-edit model switch.

The explorer also located nested-call ordering in `packages/pi-prewalk/src/branch.ts` and coverage in `src/adapter.test.ts`. These are candidate tests for the workflow integration. The lead verified the critical handoff timing directly against the local protocol and extension entry point.

Pi's installed `docs/extensions.md` documents mutable tool-call input and nested tool-call hooks. Dogsled's Pi adapter can use that boundary to resolve a unit reference before its existing MCP spawn executes. Test direct and nested calls. Hiding declarations alone does not prevent callable-tool access.

`packages/pi-guardrails/src/pi/index.ts::toolViolation()` validates shell/edit policies. Its input extraction recognizes known shell tools but does not prove arbitrary program side effects. Version one enforces workflow prerequisites, not persona permissions or operating-system isolation.

`packages/pi-todo/src/work.ts::TodoWork` publishes read-only display snapshots. Keep execution commands separate from this presentation protocol. Dogsled's `pi/work-todo.ts` deliberately treats task state as opaque and hosts the provider's renderer. Assignment display does not require another presentation protocol.

`dogsled/src/launch.rs::{allocate,start_or_reconcile}` already stores launch receipts and protects uncertain startup outcomes. `runtime::request()` currently creates a new request ID for every call, while `work::perform()` records progress only after admission. Therefore crash-safe unit claim needs a Dogsled-owned prepared request before the task is frozen. This is an explicit Dogsled change, not an existing guarantee or a pi-todo retry manager.

### Proposed work contract

An outcome holds leaf work units. A small outcome is itself an executable unit. Each executable unit has an immutable ID, deliverable, scope, acceptance condition, dependencies, and assignment. Dependencies reference IDs, never labels. Unknown dependencies and cycles fail atomically.

Scope names the repository, owned paths, and generated paths. It documents the work boundary but is not a filesystem permission grant. Acceptance records the command or concrete review artifact that proves the deliverable.

An assignment identifies `self` or `worker`. Workers reference an existing Pi profile. Model, thinking, and prewalk are not editable fields on a unit. Dogsled keeps profile resolution on the execution host.

Self-assigned work retains the current session's model and prewalk state. Starting a unit does not load a worker profile into the orchestrator or change its system prompt. Record actual session settings as evidence.

The orchestrator chooses from existing profiles, with shared guidance recommending the cheapest suitable profile. This iteration does not generate profiles, predict costs, or add persona configuration.

### Lifecycle and operations

Keep `init`, `append`, `view`, `start`, `done`, and `drop`. Add `update` for pending contracts, not a dispatch operation. Pi-todo generates unit IDs. Within one creation request, local keys can reference new prerequisites. An explicit `start` selector can atomically activate a complete self-assigned unit.

Creation can retain incomplete draft units, but `start` and contract export refuse incomplete contracts. `start` activates self-assigned work without spawning. For delegation, the agent calls Dogsled's existing spawn tool with a unit reference and necessary placement choices. The Dogsled Pi adapter obtains the validated frozen contract and its provider-rendered brief, uses the assigned profile, derives a worker name, and supplies those to the existing launch path. No second agent-authored brief or activation call is required.

Allow one locally executing leaf per session alongside delegated leaves, even with overlapping owned or generated paths. `init` cannot replace a list containing an active local unit or a held delegation binding; finish or explicitly drop eligible work first. A completed dependency satisfies a prerequisite. Dropped work does not. Unknown dependencies and cycles fail atomically. Represent owned paths as repository-relative files or directory prefixes, not arbitrary glob patterns. Same-repository overlap does not require ordering. A second local activation returns `LOCAL_UNIT_ACTIVE`, regardless of paths. Read-only inputs do not claim ownership. This checks declared scopes, not actual filesystem access.

Block contract mutation, completion, or replacement while an affected gated call is in flight. Exclude read-only and control wrappers from that execution lease. Release leases on success, error, abort, and shutdown. Mutating operations serialize and check the current revision after asynchronous work. Explicit `done` records a short note from the lead. An integration-review unit gives the lead a bounded context for shell-based review.

A started leaf cannot change its scope, dependencies, or assignment. The owning lead can close any open leaf with `done` and a short note, or with `drop`. Closing a leaf that a worker holds clears its binding and records the execution as released. A later settlement for a released execution is a no-op. Parents still require explicit integration review after their children close.

Pi-todo owns task status and acceptance. Dogsled owns launch, queue, worker, and cancellation facts. A worker's final message becomes evidence for review, not automatic task completion. A queued receipt is not a running worker.

The current session must have its own active executable unit for gated work. A delegated unit or active parent alone does not authorize unrelated local implementation. A worker receives the exact assigned unit and activates it mechanically after validation. It does not call todo to reconstruct or start that same unit.

Dogsled creates and persists execution IDs in its request/admission records before task claim; queued work, launch receipts, and items reuse that identity. Correlation uses the task's origin, ID, and revision. The Dogsled Pi adapter reconciles through existing Dogsled lifecycle/refresh paths and projects the result to the task provider. Pi-todo neither queries workers nor interprets queue or process states.

Pi-todo persists only its frozen task revision, delegation binding, acceptance state, and attached evidence references. Dogsled supplies whether the external binding is still held, safely releasable, or unresolved. Missing confirmation leaves the binding held. The provider enforces this task-level hold without recreating the process state machine.

On reload or branch change, Dogsled reconciles its records and offers the current projection to the active task provider. No new polling loop or automatic redispatch. Unknown launch outcomes remain Dogsled's responsibility. Late results match only their original task revision and binding.

Dropping a task never cancels a worker. A held delegated contract cannot be dropped, reassigned, or accepted until Dogsled releases the binding. The agent uses existing Dogsled cancellation/close tools when necessary. Rejected returned work can be reopened as a new task revision while preserving evidence.

### Profile-only assignments confirmed

The user selected profile-only assignments. Per-launch model, thinking, and prewalk overrides are out of scope. Existing profile resolution remains unchanged. A missing combination requires a separately configured profile, not a dynamically generated one.

The active-unit gate lives in pi-todo. With pi-todo loaded, every session must have a local active executable unit before gated work. Local-only work needs no Dogsled call.

### Confirmed mechanical handoff

The user selected exact structured handoff in Interview and requested mechanical steps wherever an LLM adds no judgment. After task authoring, one Dogsled spawn call performs contract handoff, launch, and association through the owning components.

Dogsled transports the validated contract and records its origin reference. On a fresh managed worker with the Dogsled guard loaded, pi-todo imports and activates it before the initial task request. Invalid or missing import aborts that request. The startup limits in section 6 apply. The worker does not rewrite the brief, recreate todos, or repeat activation. Existing prewalk otherwise requires a successful todo call after its planning nudge. Add an explicit query for validated active work so seeded units satisfy that gate mechanically. Do not fake a tool result or change the first-edit handoff.

Pi-todo mechanically handles task IDs, dependency checks, contract validation, and task display. Dogsled handles profile resolution, launch metadata, worker association, execution reconciliation, and result delivery. LLM work remains decomposition, implementation, scope-change proposals, and acceptance review.

Dogsled updates execution facts and delivers evidence to the task provider. Pi-todo can show that evidence awaits review without storing queued/running/failed worker states. Neither component automatically accepts the deliverable. No new scheduling, team configuration, or persona permissions are added.

### Confirmed enforcement boundary

The user confirmed that shell/code execution requires an active work unit, but not a subagent. Structured read/search tools remain available without a unit. Known mutating tools also require an active unit. Nested calls receive the same checks through the runtime, not source-text guessing.

The gate checks the current session's executable unit, not whether any task anywhere is active. Known shell/code and mutation tools are gated, including context-mode execution and LSP fixers. Registered tools with unknown effects default to gated unless explicitly classified as read-only or workflow control. Metadata hints alone do not grant an exception.

Todo operations, structured reads, profile discovery, user questions, and workflow control remain usable without a local implementation unit. The Dogsled Pi adapter authorizes spawn against the selected worker unit through the task provider. Pi-todo does not replace spawn with another tool. Codemode is a control wrapper: nested tool calls get their own checks. Arbitrary-code execution tools require a unit even when their supplied program claims to be read-only.

Planning and research that use shell or writes have their own bounded self-assigned units. There is no blanket Markdown exemption. The gate does not override plan-mode restrictions or existing guardrails. Human terminal commands and trusted extension internals are outside this agent-tool gate.

The planner selections `tier/plan-claude` and `tier/plan-gpt` skip the activation check. The gate evaluates the selected model at each tool call, so the exemption starts and ends with the selection. Every other selection, including another tier name, another provider, and a physical model with the same name, still requires an active unit. The exemption is a workflow prerequisite, not a write permission. It does not grant write access, and it does not change the read-only policy, which is independent of task state.

This is a workflow prerequisite, not a sandbox. Once a unit is active, version one does not police arbitrary program effects or file permissions. It cannot prove that a model's decomposition is sensible. The contract, explicit completion evidence, and review make bounded scope inspectable.

In managed Pi sessions, Dogsled's own adapter requires a validated unit handoff for worker launches, including nested spawn calls. Recognized direct shell agent-launch shortcuts must not bypass this requirement. Implement launch-specific enforcement in Dogsled, not a Dogsled command parser inside pi-todo. Shell jobs such as tests remain Dogsled jobs under the current active unit, not new subagents or units. Arbitrary code remains outside any sandbox guarantee.

### Normative v1 contract

The types, operations, and failure rules in this section are the proposed contract. They describe new behavior, not APIs that already exist. All wire objects reject unknown fields and unsupported versions. Runtime IDs are opaque, nonempty strings; revisions and sequence numbers are positive integers. No API below invokes an LLM.

#### 1. State owned by each component

| Object                                                              | Only writer    | Durable location                                     | Other component's access                                     |
| ------------------------------------------------------------------- | -------------- | ---------------------------------------------------- | ------------------------------------------------------------ |
| Unit, dependencies, assignment, acceptance                          | Pi-todo        | Existing `todo-state` entries on the Pi branch       | Dogsled receives an immutable export only                    |
| Task binding and accepted evidence references                       | Pi-todo        | Same task snapshot                                   | Dogsled calls the narrow claim/settle methods below          |
| Execution request, profile resolution, queue, launch, worker, reply | Dogsled        | Existing request/admission/queue/launch/item records | Pi-todo stores only an opaque execution reference            |
| Imported worker contract and local activation                       | Worker pi-todo | Worker branch's `todo-state` entry                   | Dogsled receives an import acknowledgement, not write access |
| Model handoff                                                       | Pi-prewalk     | Existing prewalk branch entries                      | Reads validated local activation; no worker management       |

No second queue, job table, attempt state machine, worker poller, profile resolver, or cancellation method exists in pi-todo. Dogsled stores a contract snapshot only as launch input; it never mutates the task's domain fields.

#### 2. Pi-todo's exact domain types

```ts
type Result<T> =
  { ok: true; value: T } | { ok: false; error: { code: string; message: string; unitId?: string } };

type UnitRef = {
  sessionId: string;
  unitId: string;
  revision: number;
  contractEntryId: string; // Pi branch entry that established this revision
};

type Assignment = { kind: "self" } | { kind: "worker"; profile: string }; // profile name, not model settings

type Scope = {
  repositoryId: string; // logical repository identity, no host-local root
  owns: string[]; // relative file paths or directory prefixes
  reads: string[];
  generates: string[];
};

type Acceptance =
  | { kind: "command"; cwd: string; argv: string[]; expected: string }
  | { kind: "review"; artifact: string; criterion: string };

type UnitContract = {
  ref: UnitRef;
  title: string;
  deliverable: string;
  scope: Scope;
  acceptance: Acceptance;
  dependsOn: string[]; // unit IDs in this originating task list
  assignment: Assignment;
};

type ExecutionRef = {
  executor: "dogsled";
  ownerId: string; // Dogsled owner/ledger namespace, opaque to pi-todo
  requestId: string; // Dogsled-generated durable execution ID
};

type TaskBinding = {
  contract: UnitRef;
  execution: ExecutionRef;
  exportDigest: string;
};

type EvidenceRef = {
  execution: ExecutionRef;
  resultId: string;
  summary: string; // bounded display text, not an acceptance verdict
};

type UnitState = {
  contract: UnitContract;
  status: "pending" | "in_progress" | "completed" | "abandoned";
  binding: TaskBinding | null;
  frozenExportJson: string | null; // local snapshot only; not repeated in binding messages
  evidence: EvidenceRef[];
  settlement: { requestId: string; eventId: string } | null;
};
```

`UnitState` has no host, pane, PID, queued/running state, retry count, launch error, or cancellation state. Those remain in Dogsled. The task can be in progress while its worker is queued; the shared Dogsled panel shows execution facts.

An empty `owns` is valid for read-only research. Reject absolute paths, `..`, glob patterns, and duplicate paths within each scope list. Allow equal and prefix overlaps between concurrent worker units and one local unit, including overlaps between owned and generated paths. This is declared-scope validation, not filesystem authorization. Acceptance commands are structured evidence requirements; creating a task never executes them.

The lead authors the complete contract once; pi-todo generates IDs, revision references, and export formatting. `contractEntryId` is derived when reading the Pi entry that introduced the revision, not embedded through a circular self-reference during append. Later status-only entries preserve that origin. Changing domain fields increments the revision and records a new contract entry. Bound or locally executing revisions cannot change. A new execution after a returned failure requires a new task revision, preventing accidental duplicate launches of the old revision.

#### 3. Pi-todo provider methods, callable only by extensions

The provider registers `oisin.tasks.acquire.v1` on `pi.events`. A synchronous request carries `{version: 1, reply}`. If pi-todo owns `todo`, it returns the capability object below. No responder means unavailable; multiple responders are an error. The capability is in-process, not JSON, and is not exposed as an agent tool.

`brief` uses a fixed template: title, deliverable, scope, acceptance, and origin reference. It is generated by pi-todo, which owns those field meanings. Dogsled adds only launch-context instructions. Digest input uses an explicitly ordered JSON projection of the fixed export schema, preserving array order; publish shared byte fixtures. No timestamp, host path, or runtime settings enter the task digest.

```ts
type TaskExport = {
  version: 1;
  provider: "pi-todo";
  contract: UnitContract;
  prerequisites: Array<{ unitId: string; completedRevision: number }>;
  brief: string; // deterministic rendering of exactly this contract
  digest: string; // SHA-256 of canonical exported data, excluding digest
};

type Settlement =
  | {
      version: 1;
      eventId: string;
      binding: TaskBinding;
      result:
        | { kind: "returned"; evidence: EvidenceRef }
        | { kind: "stopped"; evidence?: EvidenceRef; reason: string };
    }
  | {
      version: 1;
      eventId: string;
      origin: UnitRef;
      execution: ExecutionRef;
      result: { kind: "not_started"; reason: string };
    };

interface TaskProviderV1 {
  inspect(unitId: string): Result<UnitContract>;
  claim(input: {
    expected: UnitRef;
    execution: ExecutionRef;
  }): Result<{ binding: TaskBinding; exported: TaskExport }>;
  settle(input: Settlement): Result<{ applied: boolean }>;
  active(): Result<null | {
    ref: UnitRef;
    localSessionId: string;
    seeded: boolean;
  }>;
  imported(): Result<null | {
    version: 1;
    execution: ExecutionRef;
    unitRef: UnitRef;
    digest: string;
    localSessionId: string;
    seal: null | {
      entryId: string;
      disposition: "completed" | "abandoned";
    };
  }>;
}
```

| Method                | Reads/writes                             | Validation and exact effect                                                                                                                                                                                                                           |
| --------------------- | ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `inspect`             | Reads active branch only                 | Returns latest contract for the ID; no freeze, process lookup, or profile lookup                                                                                                                                                                      |
| `claim`               | Atomically writes one task snapshot      | Checks revision, worker assignment, full contract, completed prerequisites, ownership conflicts, and no competing binding. Freezes the contract, sets task `in_progress`, stores binding, returns deterministic export                                |
| `settle`              | Atomically writes matching task/evidence | Accepts only matching binding, origin/revision, and execution. Clears binding and records settlement ID. `returned` attaches evidence and leaves task `in_progress` for review. `not_started`/`stopped` set it `pending`, never accepted or abandoned |
| `active` / `imported` | Read local activation/import receipt     | `active` returns only an executing local leaf. `imported` confirms seed import even after completion, without reopening it. Used by gate/prewalk and Dogsled startup respectively                                                                     |

Claim persists its prerequisite evidence and canonical export bytes with the binding in the same task entry; these are frozen task data, not execution state. Repeated `claim` returns those exact bytes rather than recomputing dependencies from newer state. A different execution receives `UNIT_ALREADY_BOUND`. Repeated settlement event IDs return `{applied:false}`. A settlement for another branch/revision returns `STALE_ORIGIN`; Dogsled retains it for later delivery. No TTL releases a binding.

The provider does not expose `spawn`, `cancel`, runtime `status`, `retry`, `wait`, profile resolution, or `markAccepted`. Only the normal todo `done` operation with acceptance evidence can complete a task. Dogsled never calls it.

#### 4. Dogsled's agent-facing input and durable execution

The existing Dogsled spawn tool accepts this new alternative; it is not a new todo operation:

```ts
type UnitSpawnInput = {
  unit: { id: string };
  host?: string;
  branch?: string;
  base?: string;
};
// Existing explicit {task,name,profile,kind,cwd,...} remains for
// non-managed callers. Mixing it with `unit` is invalid.
```

Example: `dogsled.spawn({unit:{id:"u-17"},branch:"feat/reset-token"})`. Worker names are derived as `unit-<id>-r<revision>` using Dogsled's normal name validation; no extra model-generated name is required. The agent does not repeat the profile, brief, scope, model, or task JSON. The Pi adapter derives cwd from the originating session and validates repository identity; host/worktree translation stays in Dogsled.

Dogsled's Pi adapter resolves `unit` via `inspect`, then uses these internal operations. Expose CLI JSON subcommands `dogsled unit prepare`, `commit`, `observe`, and `abandon-prepared` for the adapter; do not register them as additional MCP tools. These are mechanical calls inside the single existing spawn flow:

```ts
prepareUnit({origin: UnitRef, placement}): ExecutionRef
commitUnit({execution: ExecutionRef, binding: TaskBinding, exported: TaskExport}): SpawnReceipt
observeUnit({execution: ExecutionRef}): DogsledExecution
abandonPrepared({execution: ExecutionRef}): Settlement // only before commit
```

`prepareUnit` durably records an unlaunched request and placement in Dogsled before pi-todo can claim the task. For one `(owner, UnitRef)`, repeated prepare returns the same request. Different placement on that request is `REQUEST_CONFLICT`. It creates no pane, worktree, reservation, or worker.

`commitUnit` verifies the prepared origin, binding, version, digest, and worker profile. It then enters existing Dogsled admission/queue/launch logic. Same request plus same digest is idempotent. Changed payload is `REQUEST_CONFLICT`. The selected host resolves the profile through the existing profile loader. Pi-todo never loads the profile.

```ts
type Placement = { host?: string; branch?: string; base?: string };

type DogsledExecution = {
  execution: ExecutionRef;
  origin: UnitRef;
  phase:
    | "prepared"
    | "queued"
    | "launching"
    | "running"
    | "returned"
    | "stopped"
    | "not_started"
    | "unresolved";
  // Projection of existing executor facts, not a second persisted status table.
  receipt: SpawnReceipt | null;
  settlement: Settlement | null;
};

type SpawnReceipt = {
  execution: ExecutionRef;
  origin: UnitRef;
  disposition: "queued" | "launched" | "unresolved";
  ticket?: string;
  host?: string;
  pane?: string;
};
```

The receipt is returned by Dogsled and remains in Dogsled's records/UI. Pi-todo keeps only `TaskBinding`, not `SpawnReceipt`. `observeUnit` is called only by Dogsled's integration/CLI, never by pi-todo.

Persist the prepared phase in Dogsled's existing `admissions/<requestId>.json` record family. Extend its representation to distinguish prepared requests from host-selected attempts. Preserve reading older records. Use an owner-scoped lock to resolve one origin revision to its existing request before allocating another ID; scan these records initially instead of adding another index with separate truth.

Commit and abandonment serialize on the same request identity. `work::perform()` already takes an admission lock: refactor transition ownership so commit does not recursively acquire it. Persist commitment and the frozen export before admission side effects. Reconciliation must recover committed records even when the process died before queue insertion.

Queue insertion must deduplicate by request ID; current `enqueue()` allocates tickets per call and does not supply this guarantee. Repeated commit returns the existing ticket or receipt. These are Dogsled-owned changes. Current admission and cleanup safety checks stay intact.

#### 5. One-call handoff sequence

1. Dogsled Pi adapter obtains `TaskProviderV1`, calls `inspect(unit.id)`, and calls Dogsled `prepareUnit`.
2. Adapter calls `provider.claim({expected: contract.ref, execution})`. Pi-todo atomically freezes and exports the task; the adapter adds that exact envelope to the spawn input.
3. The existing MCP spawn handler calls `commitUnit` with that prepared request and envelope. Only the handler launches; the interception hook creates no process side effects.
4. On the selected host, Dogsled writes the export and execution reference beside its existing task artifact. It passes `--todo-unit-file <path>` and the selected profile's normal flags.
5. Worker pi-todo validates/imports/activates the exact task before work. Dogsled returns its normal spawn result with execution correlation.

`prepareUnit` is necessary for crash recovery between steps, not an extra user-facing planning step. Task claim failure causes Dogsled to abandon the still-unlaunched prepared request. No user/model intervention is required for this deterministic rollback.

The MCP schema must accept a unit-reference alternative before tool input validation. Dogsled's Pi hook turns that input into the enriched wire shape below. It does not call commit itself. The schema admits both shapes so validation before or after the hook is safe.

```ts
type PreparedUnitSpawnInput = {
  preparedUnit: {
    execution: ExecutionRef;
    binding: TaskBinding;
    exported: TaskExport;
  };
};
// Spawn input is exactly one of UnitSpawnInput,
// PreparedUnitSpawnInput, or the existing explicit input.
// Placement was frozen in prepareUnit; it is not supplied again here.
```

For managed Pi calls, the Dogsled adapter accepts `preparedUnit` only when it matches the prepared call recorded for that `toolCallId`. An agent cannot paste another execution's envelope into a fresh call. Nested calls use Pi's actual nested `toolCallId`, not the wrapper's ID. This is trusted extension coordination, not an OS security boundary. Without interception, the handler rejects unresolved `unit` input. It never interprets a unit ID as prose.

If another extension blocks the tool after claim, no worker has launched. Blocked/aborted calls can bypass Pi's `afterToolCall` and therefore `tool_result`. Dogsled must observe execution-end and settled boundaries plus its durable prepared records; `tool_result` alone is insufficient. Confirmed invocation termination permits abandonment. Uncertainty retains the hold.

Tests block spawn in a later handler, abort it, and repeat both through nested codemode calls. Prove reconciliation without a tool-result event and without a second launch.

#### 6. Worker import contract

Dogsled owns the handoff file path and transport. The JSON body is `{version:1, execution, binding, exported}`. Pi-todo owns the parser and semantic validation. The runtime binds the imported worker to that one contract; it does not change the contract's original worker assignment to `self`.

On successful startup, worker pi-todo appends one atomic `todo-state` snapshot containing the exact frozen contract and a local activation binding. In Pi 1.3.0, startup entries stay in memory until a user/assistant message creates the session file. Import acknowledgement proves activation, not disk durability. Dogsled retains the durable seed artifact. Reimport is safe only before durable worker progress; thereafter branch replay wins. Imported prerequisite evidence is retained with the contract; the worker does not create dummy completed tasks for the lead's dependencies. Repeated startup for the same execution/digest does nothing. Different payloads, another repository, an unsupported version, or another todo owner are errors. A resume uses the recorded state, never the original file to reset progress.

The local activation is `{unitRef, localSessionId, importedExecution}`. It is not a worker process record. It permits the local shell/edit gate and the prewalk active-unit query to pass. Worker `init`, reassignment, or unrelated unit activation fails with `SEEDED_SCOPE_LOCKED`. A scope change is a lead decision.

**Confirmed in Interview: use the existing launch path with explicit limits.** No SDK-hosted worker, replacement launcher, or unconditional startup barrier is in scope.

Managed launches create fresh Pi sessions. With the Dogsled guard loaded, `before_agent_start` checks `imported()`. Missing or invalid acknowledgement calls `ctx.abort()` synchronously and reports `IMPORT_FAILED`, before the initial task request. A successful import requires no worker LLM setup call.

Pi catches startup and `before_agent_start` handler errors, so throwing is not the blocking mechanism. `_prepareRun()` can compact before the hook on resume. The contract therefore does not promise zero model requests during resume or if both extensions fail to load. The active-unit gate protects implementation calls while loaded; it is not protection against missing extensions.

Acknowledgement is `{version:1, execution, unitRef, digest, localSessionId, seal}` from `imported()`. The seal is null until worker-local completion or abandonment. It proves validated activation, not disk durability. Query after startup handlers finish so registration order does not decide success. Runtime tests must prove the fresh-session loaded-guard guarantee and explicitly document the two excluded cases.

`/new` does not copy the seed. Resuming the seeded session preserves its unit. A finished worker unit cannot silently reactivate merely because the process restarts.

#### 7. Result and release contract

Dogsled alone decides when a launch is not started, a worker has returned, or a worker has stopped. Idle panes and a missing probe response are not sufficient evidence. It uses its existing receipts, completed reply capture, cancellation result, and process reconciliation.

A worker's final text is untrusted result content. Dogsled stores it in its existing reply record and retains structured settlement for mechanical provider delivery. Its Pi adapter calls `provider.settle`, without asking the LLM to copy statuses.

Settlement acknowledgement is separate from transcript acknowledgement. Existing `InboxReader.ended()` acknowledges text delivery, not task settlement. Dogsled records settlement delivery only after provider success or confirmed duplicate application. `STALE_ORIGIN` or a missing provider leaves settlement pending. Lost acknowledgement safely replays the same event ID.

Applying settlement must not trigger an LLM turn solely for bookkeeping. Existing substantive worker-reply notification can still wake the lead for review.

Dogsled can emit `returned` only with execution-specific terminal evidence, not `items::finished()` alone. Idle/done panes can receive further work. Require a persisted worker seal that disables further local activation for this execution, followed by a correlated final settlement boundary, or confirmed process termination. Merely capturing prose is insufficient. Seal and terminal acknowledgement are mechanical and replayable. Without them, retain the binding and expose the reply without settlement.

Worker `todo done` or `todo drop` closes the seeded local unit and records an immutable seal in the same branch entry. `imported()` exposes its entry ID and disposition; it never seals work on Dogsled's behalf. The worker cannot reopen that execution. Lead acceptance remains independent of worker-local completion.

At the final `agent_settled` boundary, Dogsled's worker adapter reads the seal and records `{execution, unitRef, digest, sealEntryId, sessionLeafId}` through Dogsled's existing host-owned execution record. Dogsled pairs that acknowledgement with its correlated final reply before emitting `returned`. `agent_settled` alone is insufficient. A crash before acknowledgement leaves the binding held until a resumed sealed session acknowledges or process termination is confirmed.

A sealed worker cannot reopen its unit or accept new implementation under the same execution ID. Pi-todo enforces only its local activation seal; Dogsled owns this terminal acknowledgement and release decision. New work requires a new execution. If cleanup is pending or a branch needs landing, Dogsled retains those facts separately. Safe evidence delivery does not delete the worktree or imply that changes merged.

Dogsled can emit `not_started` only for a request conclusively abandoned before launch, or a proven pre-launch failure. It can emit `stopped` only after confirmed termination/cancellation. Unknown launch state produces no settlement and leaves the task bound. These decisions never move into pi-todo.

Dogsled attaches deterministic `eventId` and `resultId` values from its records. Its adapter replays settlement delivery on resume/reload using the existing refresh/inbox lifecycle. Pi-todo deduplicates evidence attachment. No new pi-todo timer, inbox reader, runtime query, or retry loop exists.

#### 8. Failure and recovery matrix

| Failure point                                 | Dogsled action                                                                                  | Pi-todo state                                    |
| --------------------------------------------- | ----------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| Before prepared request persists              | No process side effects; return error                                                           | Unchanged                                        |
| Prepared request exists, claim rejects        | Abandon uncommitted request                                                                     | Unchanged                                        |
| Claim persists, adapter crashes before commit | Reconcile durable prepared request, abandon it, deliver `not_started`; never assume it launched | Binding held until settlement, then pending      |
| Commit received, response lost                | Observe same request/receipt; never allocate another execution for this revision                | Binding remains held                             |
| Transport timeout with uncertain launch       | Existing Dogsled reconciliation; no safe-release event yet                                      | Binding remains held; no acceptance/reassignment |
| Profile/import validation fails               | Record proven failure, stop any created worker, then emit appropriate settlement                | Pending only after safe settlement               |
| Worker returns, task branch is inactive       | Retain settlement and replay when that origin is active                                         | Other branch unchanged                           |
| Duplicate or reordered settlement             | Replay exact durable ID; contradictory terminal outcomes are an executor error                  | Duplicate no-op; mismatched binding rejected     |

Before abandoning a prepared request, Dogsled serializes that action with `commitUnit`. Abandonment records a permanent terminal marker; a later commit of that ID fails `REQUEST_CONFLICT`. It cannot release a task while a concurrent commit starts a worker. Background reconciliation does not abandon a currently executing spawn invocation. Record the originating tool-call/session identity in the request and use confirmed invocation completion or session replacement; uncertainty retains the prepared request. No TTL proves it abandoned.

A prepared request without a task claim can be abandoned without a provider write. A held claim receives `not_started`. If its origin branch is inactive, Dogsled retains the settlement until that branch returns. No time-based assumption proves a worker absent.

Implementation correction: a prepared request knows the origin and execution ID, but not the export digest before commit. Therefore `not_started` carries `origin` and `execution`, not a fabricated `TaskBinding`. The provider matches the exact execution, origin revision, and contract entry before releasing a hold. No matching claim on the correct origin is an idempotent no-op; another origin returns `STALE_ORIGIN`. Returned/stopped settlements retain full binding/digest checks. This fixes the claim-before-commit crash path without another state store or metadata round trip.

#### 9. Errors and compatibility

Provider error codes are `PROVIDER_UNAVAILABLE`, `VERSION_UNSUPPORTED`, `UNIT_NOT_FOUND`, `STALE_ORIGIN`, `CONTRACT_INCOMPLETE`, `DEPENDENCY_OPEN`, `LOCAL_UNIT_ACTIVE`, `WRONG_ASSIGNMENT`, `UNIT_ALREADY_BOUND`, `BINDING_MISMATCH`, and `SEEDED_SCOPE_LOCKED`. Each returns `{code, message, unitId?}`. Errors perform no partial task mutation.

Dogsled owns `REQUEST_CONFLICT`, `PROFILE_INVALID`, `LAUNCH_UNRESOLVED`, and `IMPORT_FAILED`. It never fabricates a provider error by inspecting task domain internals. Its transport validator checks only envelope/version/correlation/digest fields; the provider/importer validates task semantics.

All new JSON fields in persisted Dogsled records are optional for old records. New managed unit launches require v1 at both ends; they fail rather than downgrade to a prose brief. Existing standalone Dogsled CLI behavior remains available outside a managed Pi session. Existing v2 todo presentation remains unchanged.

The capability API is trusted in-process cooperation, not cryptographic authorization. The digest detects mismatched payloads; it does not grant filesystem or command permissions. No security-isolation claim is made.

#### 10. Responsibility contract tests

| Test                                                                 | Required observable result                                                                     |
| -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| Start a complete self unit with Dogsled absent                       | Shell/edit allowed; zero subprocess/MCP calls from pi-todo                                     |
| Spawn by unit ID                                                     | Exactly one exported revision and one Dogsled request; no worker LLM todo setup                |
| Return a worker result                                               | Dogsled reply remains inspectable; task gets evidence but is not completed                     |
| Kill adapter after task claim                                        | Existing prepared request reconciles; no duplicate worker and no abandoned task lock           |
| Queue/host/worker state changes                                      | Only Dogsled persistence/UI changes; pi-todo has no corresponding process fields               |
| Cancel via todo                                                      | No such operation; task drop cannot stop a process                                             |
| Mutate scope through Dogsled provider bridge                         | No such method; rejected without task mutation                                                 |
| Resume another branch while a worker runs                            | No process cancellation or redispatch; only matching revision receives later evidence          |
| Fresh seeded startup, Dogsled guard loaded, importer missing/invalid | Zero initial task model requests; guard calls abort, not merely throw                          |
| Resume-time compaction or both extensions absent                     | Explicitly outside the zero-request guarantee; do not report universal startup enforcement     |
| Commit crashes before enqueue                                        | Recover the same request without a duplicate ticket                                            |
| Settlement text reaches the wrong branch                             | Transcript acknowledgement does not consume pending settlement; no bookkeeping-only model turn |
| Worker merely becomes idle                                           | No release without execution-specific terminal evidence                                        |

Prewalk uses only `active()`. With validated activation, its todo prerequisite is already satisfied. Without the capability, preserve the existing successful-todo rule. Preserve the existing post-edit handoff timing.

### Existing sessions and compatibility

Continue reading legacy flat and hierarchical snapshots. Preserve their recorded status and display, but do not treat a legacy active string as an executable contract. The first mutating operation assigns stable IDs and persists the upgraded snapshot. Use the legacy exact-content selector only to address unmigrated records. New records use IDs. Pending or active legacy work needs `update` before it authorizes execution. Never auto-reopen completed work.

Keep the shared version-2 presentation contract unchanged. Pi-todo renders task scope, desired profile, assignment, and review state. Dogsled retains worker/runtime display in the shared work panel. Do not mirror the fleet board in the todo widget. Show blocked reasons once rather than repeatedly injecting the whole list.

If another extension owns `todo`, retain existing collision behavior for ordinary sessions. In a seeded session with the Dogsled guard loaded, missing provider acknowledgement aborts the initial task request. Both guards failing to load is outside this guarantee. Deploy compatible pi-todo and Dogsled versions before enabling seeded dispatch across hosts.

## Files to modify

Paths are relative to `oisinpi` unless marked `dogsled/`. New module names describe ownership; implementation can adjust names without widening scope:

| Paths                                                                                                                                               | Expected responsibility                                                                                                                                            |
| --------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `crates/todo-core/src/{lib,operations,deserialization,widget,reminder}.rs`, new contract module, `crates/todo-wasm/src/lib.rs`                      | Work-unit contract, IDs, validation, migration, lifecycle, WASM exports.                                                                                           |
| `packages/pi-todo/src/{index,tool,state,guidance,plannotator,lifecycle}.ts`, new `gate.ts`, `handoff.ts`, `provider.ts`, and tests                  | Task operations, local gate, startup import, task-provider capability, evidence attachment, approval handoff. No dispatch or worker client.                        |
| `packages/pi-todo/src/{display,widget,embedded}.ts`, tests, README, `docs/todo-workflows.md`                                                        | Reuse the renderer for assignment visibility; document the execution protocol and limits.                                                                          |
| `packages/pi-prewalk/src/{adapter,ports}.ts`, `crates/prewalk-core/src/{types,protocol}.rs`, and tests                                              | Accept validated, already-active seeded work as todo-gate evidence, without a redundant LLM todo call. Preserve post-edit handoff timing.                          |
| `/Users/oisin/dev/dogsled/src/{work,mcp,cli,runtime,launch,items,inbox,sweep}.rs`, `pi/{index,inbox}.ts`, new `pi/work-units.ts`, and related tests | Existing spawn integration, unit transport, execution correlation/reconciliation, task-provider bridge, and result delivery. No profile override or new scheduler. |

Extend Dogsled's existing runtime lookup and refresh paths with task/execution correlation as needed. All process queries stay in Dogsled. Keep profile parsing and selection unchanged.

Generated outputs: `packages/pi-todo/src/generated/` and `packages/pi-prewalk/src/generated/`, through their existing `generate` tasks. Never edit generated files directly.

## Reuse

- `runTodo()` and `latestSnapshot()` preserve one branch-local task store.
- Rust `apply()` and `validate()` remain the task validation boundary.
- `workUnitGuidance` remains shared guidance, not an authorization mechanism.
- `PlannotatorHandoff` remains the approved-plan entry point.
- Dogsled's `runtime::start`, request/queue, `launch::{allocate,start_or_reconcile}`, item/inbox records, and Pi refresh lifecycle retain execution ownership. Prewalk retains model handoff.

## Steps

Each deliverable includes behavior tests and a real caller. Merge dependent work in order. After unit 1, Dogsled transport can proceed alongside local gate/import work. Dogsled's provider bridge integrates only after the provider is available. Delegate independent work; do not delegate this whole cross-repository plan as one assignment.

- [ ] **1. Executable task contracts.** Own todo-core/WASM, tool schema, migration, task display, and protocol specification. Deliver creation/update/start/completion through the real todo tool. Acceptance: `moon run todo-core:test pi-todo:test`. Generated output: todo bindings.
- [ ] **2. Local gate and provider.** Own `gate.ts`, `provider.ts`, task registration, and tests. Deliver local enforcement and atomic contract claim/evidence interfaces without a Dogsled client. The local gate is the active-unit capability's first caller. Acceptance: `moon run pi-todo:test`. No new generated output.
- [ ] **3. Worker import and prewalk.** Own `handoff.ts`, startup integration, prewalk adapter/core, and tests. Deliver `--todo-unit-file` with automatic activation and no worker bookkeeping calls. Acceptance: `moon run pi-todo:test pi-prewalk:test prewalk-core:test`. Generated output: affected WASM bindings.
- [ ] **4. Dogsled execution integration.** Separate ordered units: (a) durable prepared/committed request lifecycle with CLI caller and crash/race tests; (b) seeded transport and settlement correlation through existing MCP/queue/inbox paths; (c) Pi spawn/provider integration and startup acknowledgement. Each unit merges independently with its tests and existing callers. Acceptance: scoped `mise run check:rust` or `mise run check:pi` at final revision. All execution, recovery, and process queries stay in Dogsled.
- [ ] **5. Integration proof and docs.** After all producers and consumers land, verify one Dogsled spawn call exports the task, launches the exact worker contract, and attaches returned evidence. Add protocol boundary tests and update guidance; no new execution module in pi-todo. Acceptance: `moon run pi-todo:test -- --run src/workflow.integration.test.ts`, plus the scoped live scenario. No new generated output.

## Verification

No code changed and no tests ran during planning. Implementation must prove five groups:

- **Core:** reject incomplete contracts, duplicate IDs, malformed or duplicate paths, dependency cycles, and dropped prerequisites. Accept overlapping concurrent worker scopes and one overlapping local unit, including owned/generated paths. Reject a second local unit with `LOCAL_UNIT_ACTIVE`, regardless of scope. Preserve immutable dispatched scope, evidence-required completion, and explicit parent review. Invalid operations leave state unchanged.
- **Gate:** no-unit shell/edit/code fails; reads and workflow control work; a parent or another worker's unit cannot authorize local execution. Test direct and nested calls, LSP mutations, in-flight completion races, planning units, and tool-owner collisions.
- **Import/prewalk:** with the Dogsled guard loaded in a fresh worker, the exact contract activates before the initial task request, without a worker todo call. Missing/invalid import aborts that request. Cover branch/reload/new behavior, prewalk on/off, and unchanged post-edit switching. Do not assert zero requests for resume-time compaction or absent guards.
- **Dogsled/results:** test queue versus running state, invalid profiles, lost responses, duplicate launch, structured correlation, stale/duplicate evidence, and cancellation visibility in Dogsled. Provider tests prove that evidence attachment cannot accept the deliverable and absent confirmation cannot release a held contract.
- **End-to-end and ownership:** prove local-only work with Dogsled absent. Then use one Dogsled spawn call with a unit reference to launch a temporary owned fixture. Verify automatic import, result association, explicit acceptance, and safe cleanup. Assert no todo dispatch operation, Dogsled client, runtime polling, queue states, or worker reconciler exists in pi-todo. Prewalk requires no synthetic todo call.

Run scoped fixers and light checks only after a unit's edits finish. Query affected Moon targets before heavy verification. Run affected todo/prewalk Rust and TypeScript gates once at the final integrated revision, without repeating equivalent hook or child evidence. Dogsled's named gate is `mise run check`; scope Rust-only or Pi-only changes to `check:rust` or `check:pi` where applicable.

Add `src/workflow.integration.test.ts` to the existing Vitest task for the real core, provider, and importer. Dogsled's own tests cover launch and reconciliation with that protocol. Keep launch failures deterministic and share versioned contract fixtures rather than copying either component's policy. Run one live local worker scenario with the approved compatible packages, then stop and clean up only its own resources. Never install packages or modify host services implicitly.

Release/install is a separate authorization step. Until compatible packages reach each selected host, report the feature as implemented or tested, not operational fleet-wide.

## Concrete agent workflow

Local work: `todo init/append` supplies the contract and a local start selector. Pi-todo activates it in the same call. The agent performs work and calls `todo done` with acceptance evidence. Dogsled is never consulted.

Delegated work: the lead creates a contract assigned to a profile, then calls existing Dogsled spawn with `{unit:{id}, branch?}`. Everything from export to worker activation is mechanical. Dogsled delivers the correlated result. The lead reviews and calls `todo done` if accepted. No worker setup todo call, repeated brief, model override, manual status copying, or second dispatch tool.

## Review changes

First revision removed `todo dispatch` and pi-todo's execution client/reconciler. The second revision specifies exact data types, provider methods, spawn input, durable ownership, prepare/claim/commit sequence, settlement events, error codes, and recovery.

The cost is explicit: Dogsled needs a durable prepared-request phase before task claim and typed result correlation. Pi-todo needs only task binding/export/import/evidence methods. Runtime queue, host, pane, retry, and cancellation fields are prohibited in pi-todo state. Boundary tests enforce those exclusions.

Independent review added commit-before-enqueue recovery, queue deduplication, blocked-call reconciliation without `tool_result`, and separate settlement acknowledgement. The user selected existing launch behavior with a loaded-guard startup guarantee, excluding resume-time compaction and both guards failing to load.

The separately authorized cleanup repair remains independent of this workflow initiative. Its workers must not implement workflow changes.

## Research state

Lead inspected the todo contract, Pi extension hooks, Dogsled profiles, and the Dogsled launch request. Explorer `workflow-prewalk-evidence` finished read-only research at revision `404702824d3d4784bcc94a09ab8ed562a4654ea1` and closed. Its report identified prewalk's post-edit handoff and the absence of workflow state in guardrails. The lead verified both consequential claims against local source. The second review pass also inspected Dogsled `runtime::request`, `work::perform`, and `Ledger::with_lock/write` for the prepare/claim crash boundary. Installed Pi 1.3.0 evidence: `dist/core/extensions/runner.js::emitBeforeAgentStart`, `dist/core/agent-session.js::{_beforeToolCall,_handleAgentEvent,abort}`. These source observations support the proposed integration; runtime contract tests are still required. No tests ran and no workflow implementation changed.
