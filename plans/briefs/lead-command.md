# Writer brief: `dogsled lead`

Status: design accepted 2026-11-04 from the architect unit (docs/lead-design). Implementation units L1 to L4 are not started.

## Cause and evidence

The child launch path requires an owner and installs a completion contract, so a top-level lead needs a separate lifecycle owner.

Evidence: `src/runtime.rs:139`, `src/work.rs:2290`, `src/items.rs:14`.

Source baseline: `a0ce8764258c4a70d968a53522a4e6f1b9c5ef6c`.
Approved briefs: `6dc7dd624b76497e3417d1256b3310842c4871b6`.

## Answers to the six questions

### Q1. Recording and visibility

**Do not create an `Item`.** Add a host-local lead record in a separate typed table.

`Item` represents owned child work. Its Rust field and SQL column require an owner (`src/items.rs:14`, `src/tables.rs:52`). An empty owner or self-owner would misrepresent that contract.

The lead record owns its request ID, generated name, repository, pane, terminal identity, launch phase, diagnostics, and explicit closure. It has no owner, task binding, custody, or final-result fields.

Visibility remains separate from child work:

| Surface | Lead behavior | Existing source |
|---|---|---|
| CLI status | Appears among observed agents | `src/commands.rs:725` |
| Status JSON | Appears in `hosts[].probe.agent_panes`, never `items` | `src/commands.rs:10` |
| Work board | No child-work row or needs-you count | `src/board.rs:780` |
| Portal board rows | No synthetic lead item | `src/board.rs:782` |
| Portal Agents list | Appears through the existing Herdr list | `src/web/agents.rs:94` |

Keep `agent_panes` as the full inventory. Exclude positively identified leads from the counts used for admission. The current admission rule reads `Probe.agents` (`src/plan.rs:48`).

### Q2. Launch reuse and exclusions

Reuse repository placement, immutable request validity, bounded native startup, terminal checks, and startup diagnostics.

Extract the repository-pane allocation from `work::pane` without constructing a fake `Launch` (`src/work.rs:1433`). Preserve its shown-repository/new-workspace rule and `--no-focus` behavior.

Use `jobs::issue_request_validity` and `jobs::check_validity` directly (`src/jobs.rs:143`, `src/jobs.rs:491`). A validity record does not reserve capacity.

Extract the native-start mechanism from `launch::start_with_busy_wait` (`src/launch.rs:1314`). Preserve retries only for the explicit busy refusal. Reuse `startup_error` and terminal checks (`src/launch.rs:2196`, `src/launch.rs:2626`).

**Do not reuse child launch receipts unchanged.** Their request contains child ownership, and successful launch registers a watchdog parent (`src/launch.rs:1753`, `src/launch.rs:1819`). The lead record supplies its own launch receipt.

Skip `runtime::request`, admission, reservations, queueing, preparation claims, child files, profiles, seeds, custody, and handoff. `runtime::start_using` queues unsuccessful admission, so it is not the lead entry point (`src/runtime.rs:246`).

### Q3. Lifetime and explicit operations

A lead remains open after an idle turn, a stopped turn, or Pi exit. Its remaining shell also stays open.

No `Item` means child sweep and stopped-child prompting exclude it naturally (`src/sweep.rs:218`, `src/watchdog.rs:598`). Explicit protections are still necessary:

- `done::mark` currently accepts agents without an item (`src/done.rs:317`). Refuse lead `done`, `ask`, and `blocked` signals.
- `handoff_ready` returns true for an empty candidate list (`src/launch.rs:1466`). Absence of a child launch is not sufficient protection.
- Cleanup can close an unrecorded idle shell after its age threshold (`src/cleanup.rs:1328`). Protect lead shells and their enclosing workspace.
- Worktree cleanup checks all holders (`src/cleanup.rs:1542`). A lead holder must prevent automatic checkout removal.
- Watchdog stall processing scans all observed agents (`src/watchdog.rs:224`). Exclude leads from stall intervention, but retain delivery about their children.

Explicit `close` closes only the lead pane. It never removes the repository, runs teardown, or settles child work. Preserve busy, caller-pane, group, and terminal-identity safeguards (`src/close.rs:711`).

`read` remains observational. `send` retains inbox delivery and reader checks (`src/runtime.rs:428`, `src/inbox.rs:576`). Neither creates a completion result.

### Q4. Naming

Generate `lead-<request-id>` once from `ledger::new_id`, then persist it before allocation (`src/ledger.rs:60`).

Pass that exact name to:

```text
herdr agent start <name> --kind pi --pane <pane>
```

Do not add child arguments after `--`.

Existing `read` and `send` resolve observed agent names (`src/runtime.rs:456`, `src/inbox.rs:539`). Extend explicit `close` to resolve lead names. Its current host lookup only matches pane IDs (`src/runtime.rs:769`).

Use stored terminal identity for authority. A name or reused pane ID alone must not authorize closure or capacity exemption.

### Q5. Stage 2 interface

Reserve this future host command:

```text
dogsled host lead --input <JSON>
```

Its versioned envelope contains `id`, original validity, repository `{path, origin}`, and `prompt`. It contains no owner or reservation.

The origin selects one eligible host with `plan::choose` (`src/plan.rs:107`). It persists that selection before transport. An uncertain response never permits another host selection.

The host validates the envelope and calls the same host-local lead launcher. Provisioning uses `checkout::local`, not a fabricated child request (`src/checkout.rs:180`). `checkout::ensure` currently requires a child `Request` (`src/checkout.rs:90`).

Use `Fleet::remote_setup`, which shares the existing bounded SSH transport (`src/hosts.rs:354`). Replays retain the request ID and return saved outcomes without another start or prompt.

Stage 2 routing belongs in `cli::HostCommand` and `main::host` (`src/cli.rs:724`, `src/main.rs:1260`). **Do not implement Stage 2 in these units.**

### Q6. Writer split

Use four sequential backend units:

```text
repo-list → L1 records → L2 lifetime → L3 command → L4 portal API
```

The portal UI is a separate unit. Its creation contract is only:

```text
POST /api/leads
{ "repo": "<registered selector>", "prompt": "<text>" }
201 { "pane": "<pane>" }
```

Shared types belong in `portal-types/src/agents.rs`. This brief does not authorize `web/` UI edits.

## Shared implementation contract

The public synchronous function is `leads::create`. CLI, MCP, and HTTP adapters call it directly.

Its context supplies the existing shell seam, ledger, selected config path, home directory, and Herdr client. It never reads caller pane or workspace variables.

The function reloads the selected config before resolving the repository. Persistent MCP and portal processes must not retain a stale repository list.

Stage 1 requires the registered local checkout to exist and match its stored origin. It does not clone, create a worktree, or change branches.

Persist these effect boundaries:

```text
recorded → allocating → allocated → starting → started → prompting → ready
```

Persist intent before each external effect. Persist pane and terminal identity before starting Pi.

After native startup succeeds, send the first message once through `Client::agent_prompt` (`src/herdr_socket.rs:166`). Never put the prompt on the terminal launch line.

A partial failure returns the request ID, known pane, phase, and existing error cause. It never reports success, automatically closes the pane, or retries an uncertain start or prompt.

Completed receipt replay is read-only. An interrupted effect remains uncertain until explicit inspection.

## Dispatch conditions

Repository: `github.com/oisin-ee/dogsled`.

Each writer starts from the exact committed result of its prerequisites. The lead must record that SHA before dispatch.

Generated paths: none. No dependency, release, installed-version, or deployment changes.

Publication authority: commit the owned changes. No push, PR, merge, release, or deployment without separate authority.

Allowed checks: formatting and the unit’s focused tests. The lead owns `moon run :check` on the integrated revision.

The required commit hook remains enabled: `.moon/workspace.yml:19`. Its configured checks are an exception to the manual heavy-check restriction.

On an owning blocker, stop without widening scope. Return the branch, SHA, changed paths, exact checks, results, and unverified behavior.

## L1 — Persist lead identity and exclude its demand

Branch: `feat/lead-record`.
Prerequisite: none. Start from `origin/main`; L1 uses no repo-list code. The repo-list branch also edits `src/node.rs` and `src/main.rs`, and the lead resolves that overlap at integration.
Deliverable: durable ownerless records and correct capacity counts.

| File and function | Edit |
|---|---|
| New `src/leads.rs`: `Record`, `Phase`, `matches_agent`, `protects_pane` | Define the lifecycle record and identity rules. Missing identity is unknown. A proven replacement terminal is not the old lead. |
| New `src/lead_store.rs`: `SCHEMA`, `get`, `list`, `insert`, `update`, `exists_in` | Add typed SQLite storage with unique request IDs and names. Store launch intent and outcome separately. |
| `src/main.rs:33`: module declarations | Register `leads` and `lead_store`. |
| `src/store.rs:11`: `VERSION`, `schema` | Raise `VERSION` from 4 to 5 and include the lead table. Keep the existing schema-transition policy (user decision, 2026-11-04). |
| `src/probe.rs:15`: new `AgentCounts::from_agents`, `local` | Centralize count construction. Preserve raw agent inventory. |
| `src/node.rs:679`: `observed_probe` | Rebuild capacity counts excluding exact registered lead identities. Preserve measured CPU, memory, disk, and reservations. |
| `src/commands.rs:484`: `summary` | Use observed inventory length for the displayed agent total. Keep it consistent with the displayed state breakdown. |
| `src/work.rs:214`: `expire_request_sidecars` | Retain validity while a lead record references the request. |
| New `src/leads/tests.rs`: `lead_records_round_trip`, `lead_identity_requires_terminal`, `lead_counts_exclude_only_registered_agents` | Cover persistence, restart, pane reuse, and capacity exclusion. |
| `src/store.rs`: tests | Cover schema creation, missing-database reads, and rejection of newer schemas. |

Keep records independent of child result retention. No lead record expires merely because Pi stopped or the request became old.

Acceptance: `cargo test lead_` and `cargo test --bin dogsled store::`.
Expected: records survive reopen, leads remain visible, and their declared demand is zero.

**Deployment caution:** `store::open` deletes older-schema tables (`src/store.rs:72`). The user chose to keep this rule: roll out only when no children run on any host. This unit does not authorize opening live state with the new binary.

## L2 — Protect lifetime and support explicit close

Branch: `feat/lead-lifetime`.
Prerequisite: L1.
Deliverable: only an explicit close can remove a lead pane.

| File and function | Edit |
|---|---|
| `src/leads.rs`: new `resolve_local`, `close_explicit` | Resolve name or pane against stored identity. Record explicit close intent and outcome. Refuse replacement terminals. |
| `src/close.rs:221`: `with_request`, new `close_lead` | Route registered leads before child preservation or checkout removal. Permit only explicit clean-mode pane closure. Reuse `validate_target` and `pane_only`. |
| `src/runtime.rs:344`: `close` | Resolve a local lead name before the existing child path. Preserve existing child behavior. |
| `src/done.rs:234`: `observe` | Ignore lead completion marks and signals. Retain `owns_open` and shared lifecycle publication. |
| `src/done.rs:317`, `:388`: `mark`, `signal` | Refuse lead terminal signals before writing marks. |
| `src/done.rs:1005`: `handoff_locked` | Exclude registered lead identities before branch refresh, reply capture, or closure. |
| `src/watchdog.rs:31`, `:211`: `observe`, `marks` | Exclude registered leads from stall intervention. Add ledger access to observation. Preserve child-to-lead notifications. |
| `src/node.rs:342`: `Node::tick` | Supply the ledger to watchdog observation. |
| `src/cleanup.rs:1398`, `:1651`: `idle_panes`, `held` | Protect lead agents, exited lead shells, enclosing workspaces, and checkout holders. Recheck records during destructive validation. |
| `src/watchdog_tests.rs`: `Rig::reports`, observation tests | Adapt the observation signature. Add `lead_has_no_watchdog_intervention`. |
| `src/done/tests.rs`: new `lead_cannot_handoff` | Assert refusal and absence of marks or closure. |
| `src/cleanup.rs`: new `lead_shell_survives_cleanup`, `lead_holder_blocks_checkout_removal` | Cover old idle shells and removal through another holder. |
| `src/leads/tests.rs`: new `lead_close_preserves_repository`, `lead_close_refuses_replaced_terminal` | Cover pane-only closure and identity refusal. |

`read` and `send` retain their current observed-agent paths. A stopped Pi session has no invented final result.

Acceptance: `cargo test lead_`, `cargo test --bin dogsled done::`, and focused existing close tests.
Expected: automatic paths preserve the pane and checkout. Explicit close removes only the matching idle pane.

## L3 — Implement the shared launcher, CLI, and MCP

Branch: `feat/lead-command`.
Prerequisites: L2 and landed `repo-list`.
Deliverable: CLI and MCP start one local ownerless Pi lead and submit its first prompt.

| File and function | Edit |
|---|---|
| `src/repos.rs`: new `resolve` after the repo-list unit | Resolve exactly one registered origin or path. Reject missing, ambiguous, escaping, or mismatched entries. |
| `src/leads.rs`: new `CreateContext`, `Started`, `Failure`, `create`, `start_local` | Implement the shared contract and phase transitions. Preserve the original error with ID, pane, and phase. |
| `src/work.rs:1433`: `pane`, new `repository_pane` | Extract repository allocation without child ownership. Keep child fallback behavior unchanged. |
| `src/launch.rs:1282`, `:1314`: native-start helpers | Extract a reusable native-start helper with a caller-supplied validity check. Preserve the existing child wrappers and busy behavior. |
| `src/launch.rs:2196`, `:2626`: `startup_error`, `same_terminal` | Expose the existing helpers within the crate. Do not reuse child refusal cleanup or child receipts. |
| `src/cli.rs:198`: `Command` | Add `Lead { repo, message, message_file }`. Require exactly one message source. Add no placement or profile flags. |
| `src/cli.rs:238`: `Command::Close` help | Document lead names and pane-only lead closure. |
| `src/main.rs:199`: `run`, new `lead_command` | Preserve the selected config path. Dispatch local leads before fleet discovery. Print typed success or partial-failure details. |
| `src/mcp.rs:25`, `:272`, `:672`: `Context`, `Server::lead`, `serve` | Retain the config path. Run the same creator on a blocking thread without the child `Server::call` path. |
| `src/main.rs`: `run` MCP arm | Pass the selected config path to `mcp::serve`. |
| `src/mcp.rs:472`: `Server::close` description | Describe explicit lead closure without changing child authority. |
| `tests/cli.rs:4`: module declarations | Register new `tests/cli/lead.rs`. |
| New `tests/cli/lead.rs`: launch and failure tests | Exercise CLI and MCP with private state, fake Herdr, and a fake prompt socket. |
| `README.md:114`: “Jobs, agents, and owners” | Document the lead command, naming, lifetime, and partial-failure behavior. |

CLI `--message-file -` reads stdin. MCP must reject `message_file: "-"`, because stdin carries its protocol.

Validate the repository and prompt connection before pane allocation. Preserve prompt text after the nonempty check.

Tests must prove absent caller variables, unchanged items/queue/reservations, correct workspace placement, exact native argv, and one first prompt.

Also cover invalid selectors, startup refusal, busy refusal, changed terminal, prompt failure, and interrupted effect replay.

Acceptance: `cargo test --test cli lead::`, `cargo test --test cli launch_retry::`, and `cargo test lead_`.
Expected: no child contract, queue entry, reservation, automatic cleanup, or duplicate prompt.

## L4 — Expose the portal creation API

Branch: `feat/lead-api`.
Prerequisite: L3.
Deliverable: the guarded HTTP route invokes the same creator and returns the new pane.

| File and function | Edit |
|---|---|
| `portal-types/src/agents.rs:77`: new `CreateLeadBody`, `CreatedLead` | Define `{ repo, prompt }` and `{ pane }`. Reject unknown request fields. |
| `src/web/mod.rs:52`: `AppState`, `AppState::new` | Add optional lead-launch context. Keep one-shot servers without that capability. |
| `src/web/mod.rs:114`, `:127`: `serve`, `run_until` | Supply the selected config path, home, and existing shell seam to the launch context. |
| `src/main.rs`: `run` web arm | Pass the selected config path into the portal. |
| `src/web/agents.rs:64`: `router` | Add `POST /api/leads`. |
| `src/web/agents.rs`: new `create_lead`, `lead_error_response` | Run `leads::create` with `spawn_blocking`. Return 201 only after prompt acceptance. |
| `src/web/agents.rs:390`: tests | Add route success, validation, unavailable-client, partial-failure, and origin-guard tests. |
| `src/web/mod.rs`: portal lifecycle tests | Adapt launch-context construction without changing one-shot behavior. |

Keep the existing origin/token guard (`src/web/mod.rs:217`). Do not call a shell command that recursively invokes `dogsled lead`.

Return 400 for invalid input or selectors, 503 for unavailable launch capability, and 502 for native launch or prompt failures.

Partial failures include the known request ID, pane, and phase. The route does not retry or close the pane.

Acceptance: `cargo test --bin dogsled web::agents::` and `cargo test -p portal-types`.
Expected: exact 201 body, observable shared-creator effects, guarded writes, and preserved partial launches.

## Review and handoff

No Stage 1 design questions remain. Stage 2 transport and the separate UI remain outside these writer units.

The lead must update `plans/fleet-leads.md` stage 1b to replace “item kind `lead`” with the separate record decision.

Evidence is source inspection only. No implementation or tests ran. The checkout remained unchanged.
