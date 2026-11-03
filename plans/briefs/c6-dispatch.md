Task: C6 from plans/lifecycle-cleanup.md: request-scoped preparation retirement (src/preparation_retirement.rs and the "retire-preparation" command). Your branch is feat/preparation-retirement, based on origin/main at @@MAIN@@ (C1 through C5 are merged). Run git fetch origin first, then CHECK that git merge-base --is-ancestor @@MAIN@@ HEAD succeeds; if not, git merge --ff-only origin/main. You are a standalone writer: create and start one self-assigned todo leaf before running shell commands. Copy any existing .worktrees/*/web/public into ./web/public before committing. Commit in the background (this PATH is for the hook only; run your own cargo tests with your NORMAL PATH, because mise shims break the python3 peer fixture):
nohup sh -c 'PATH="$HOME/.cargo/bin:$PWD/.mise/shims:$HOME/.local/share/mise/shims:$PATH" mise x -- git commit -m "<msg>" > /tmp/c6-commit.log 2>&1; echo EXIT=$? >> /tmp/c6-commit.log' >/dev/null 2>&1 &
Then poll the log. If launch_retry::prepare_and_launch_wait_for_repository_lifecycle_lock flakes, retry up to 3 times.
The contract below is the WHOLE contract. Its brief file is not in your worktree.
Lead decisions, binding:
- the cleanup override is CLI-only (APPROVED default);
- a versioned envelope with no downgrade.
Integrated interfaces (use them, and don't re-implement them):
- C1: the worktree lifecycle lock and identity, worktree::same;
- C2: job holds and jobs::terminal/holds;
- C3: prepare::disposition::dispose, Policy/Basis (including Basis::Cancelled from C5), the correlated preparation lookup (placement and non-placement variants);
- C4: PlacementPhase, retained placement-grants, place --resume;
- C5: the durable cancellation intent and fence.
PRE-AUTHORIZED EXTENSIONS, so DON'T STOP TO ASK. Additive changes to earlier units' code that C6 needs are AUTHORIZED in src/prepare.rs, src/prepare/disposition.rs, src/main.rs, src/cli.rs, src/hosts.rs, src/work.rs, src/cleanup.rs, src/close.rs and src/mcp.rs. That includes:
- a new typed Basis or Policy variant with its validate_policy arm;
- a new read-only lookup variant;
- a new host subcommand behind the versioned --preparation-protocol envelope;
- new fields with version handling.
Constraints: don't change how EXISTING variants apply, the lock order, the transfer path, ack correlation, or existing behaviour for existing callers. Older peers refuse new variants. List every such extension in the commit message. Fixture and wire-literal updates in any existing test are also authorized, if listed. For anything else outside scope, send the lead a one-line message and CONTINUE with the rest; don't end your turn blocked.
Publication: the LEAD OVERRIDES "commit only": pushing your own branch is AUTHORIZED (git push -u origin HEAD, then check with ls-remote). No PR.
Lessons, binding:
(1) one atomic capture of each expectation, compared later, and never replaced;
(2) list EVERY caller path before coding, and in the commit message;
(3) no refusal or hold may leave capacity or a claim stuck forever: give it a definitive release path or a visible reason;
(4) fixtures use short /tmp roots and git init -b main; compare paths with worktree::same, never as raw text;
(5) tests running "serve --once" need rig.config zero admission thresholds plus a seeded snapshot;
(6) when a behavioural test conflicts with the contract, split it by action, keeping every unaffected property, and list it.
Binding contract (verbatim):

## Shared binding requirements

### Required C3 interface

These are **prerequisite requirements**, not claims about implemented C3 code.

| Required capability | C4–C6 dependency |
|---|---|
| **Correlated preparation lookup** | Read the original request, outcome, claim, and disposition without allocating, running setup, or opening a pane. Distinguish absent, incomplete, successful, failed, and disposed records. |
| **Immutable disposition intent** | Persist the exact request identity, selected host, operation, expected claim, and operation identity before transport. Repeated delivery uses that saved envelope. |
| **Host-owned fence and release** | Under the preparation request gate, validate the envelope, persist the dispatch fence, release only the matching claim, and save a correlated acknowledgement. |
| **Caller-owned fence** | Admission, queue recovery, explicit resume, and commitment cannot dispatch a prohibited request. Checking a terminal ticket alone is insufficient. |
| **Independent execution evidence** | A disposition acknowledgement proves fencing/release, not that execution never happened. It must not manufacture `not_started` or modify Job ownership. |

C3 must cover these existing callers:

- Preparation: `prepare::run`, `local`, and `on_host` — `src/prepare.rs:56–183,498–509`.
- Admission and host launch: `work::perform_locked`, `receive_launch`, and `launch` — `src/work.rs:764–875,1196–1283`.
- Release: placement, agent/job launch completion, and native-refusal receipt repair — `src/work.rs:923–970,1163–1185`; `src/launch.rs:1075–1136`; `src/main.rs:688–700`.
- Job wrapper startup: **C2** `src/jobs.rs:1091–1155`.
- Continuation: preserve the established admission → done → launch order at `src/continuation.rs:65–67`; apply C3’s decided distinction between prohibited new dispatch and permitted historical evidence reads.

If C3 cannot provide these contracts, stop before dispatching the dependent writer. Do not recreate disposition policy in each caller.

### Atomic expectations

Each operation captures **one complete expectation**, persists or retains it, and compares later observations against it.

| Operation | Immutable expectation |
|---|---|
| Placement | Entire originating `Request`; selected `Host`, including target; reservation identity; C3 preparation request and returned claim identity |
| Cancellation | Ticket/request pair; complete request; current admission evidence; existing launch/result; cancellation operation identity |
| Retirement | Request ID; exact historical claim and preparation provenance; selected host; actor/authority; custody revision when applicable; reason |

Rules:

1. Later observations may validate an expectation, never replace it.
2. A changed request, host target, custody revision, or claim produces a conflict before release or dispatch.
3. Missing identity remains missing. Never fill historical identity from the current pathname.
4. Use C1’s checkout identity, not path equality, when an operation touches checkout ownership. C1 defines that identity at `src/worktree.rs:26–42`.
5. Keep filesystem capture and validation in the existing owner. C1’s bracketed capture and pinned-directory checks are at `src/worktree.rs:202–230,405–462`.

### Lock order

```text
service admission lock, when already held
  → origin admissions/<request-id>
    → custody, only for the short authority decision
    → origin placement-attempts/<request-id>, when selecting/reserving
    → host preparation-receipts/<request-id>
      → Git common-directory lifecycle lock
        → done
          → launches/<request-id>
            → launch-panes/<pane>
              → short store locks
```

Additional constraints:

- Release custody before host RPCs. Save the authorized intent while custody remains locked; that intent becomes the operation’s authority.
- The capacity branch uses the host preparation gate followed by the existing host-capacity lock. Do not hold the capacity lock during preparation or lifecycle work.
- Never hold `queue` while acquiring admission or making RPCs.
- Keep `prepare::used` a short, matching-claim leaf operation.
- Preserve C2’s startup order and lifetime-lock boundary. C2 takes preparation → repository → nonblocking exec → short Job lock; monitoring retains only exec.

Existing admissions-before-custody examples are `src/unit_request.rs:1013–1075,1100–1136`. C1’s lifecycle lock is `src/worktree.rs:176–187`.

### Writer execution contract

For every unit:

- **Repository:** Dogsled.
- **Generated paths:** none.
- **Publication authority:** commit owned changes only; no push, PR, merge, release, installation, or host-state cleanup.
- **Read first:** this contract, the integrated predecessor diff, the cited owners, and existing CLI fixtures.
- **Checks:** run the named focused regressions before and after implementation. Record command, revision, failure assertion, and final result.
- **Heavy checks:** the lead owns the integrated repository gate. Required commit hooks remain enabled.
- **Failure behavior:** stop on a missing prerequisite, ownership conflict, or consequential design change. Do not widen scope silently.

Use existing real-process fixtures: `Rig`, `Background`, `Owned`, and `Fifo` in `tests/cli.rs:65–203,302–370`. Use FIFO rendezvous or held kernel locks, not sleeps, to establish race order.

---


# C6 — Request-scoped retirement

**Proposed branch:** `feat/preparation-retirement`

## Owning source

| Owner | Current behavior |
|---|---|
| `src/prepare.rs:24–30,480–495` | Claims contain a tree and optional request ID; records distinguish requests sharing a path. |
| `src/prepare.rs:185–211` | Preparation outcome is separate historical evidence; incomplete outcome remains uncertain. |
| `src/prepare.rs:549–560` | `used` already releases only matching path/request records. Preserve this independence. |
| `src/unit_request.rs:1013–1075` | Admission precedes custody during an authority-sensitive mutation. |
| `src/custody.rs:26–46,308–314,336–364` | Custody records carry revision/session identity and provide a locked check. |

C2 adds independent Job holds: **C2** `src/jobs.rs:414–440`; cleanup reloads them under its final ownership check. Retiring preparation must not remove those holds.

## Concrete interleavings

| Window | Interleaving |
|---|---|
| Shared pathname | Requests A and B hold the same checkout → retire A → B must remain byte-for-byte unchanged and continue holding the checkout. |
| Late dispatch | Retirement captures A → a delayed launch or wrapper attempts A → the host preparation gate orders dispatch against the retirement fence. |
| Replacement claim | Retirement captures A’s claim → waits → stored claim changes → retirement must refuse, not adopt the replacement. |
| Lost acknowledgement | Host saves retirement fence and releases A → response disappears → origin retries the same intent and receives the same receipt. |
| Custody movement | Caller captures custody revision N → another claimant obtains N+1 → retirement authority must fail comparison, not use newly observed authority. |

## Binding implementation design

### Command and authority

Add:

```text
retire-preparation <request-id> [--host <host>] --reason <text>
```

Recommended authority modes:

- **Owner/current custodian:** validate recorded origin or current custody using live caller identity. Capture the authority tuple atomically under admission → custody.
- **Explicit cleanup workflow:** a separately authorized CLI mode records the operator and reason. Owner absence alone never grants this authority.

Do not accept a caller-supplied actor string as proof. Record host/OS identity and available pane/session identity from the invocation.

For historical claims without sufficient owner provenance, require explicit cleanup authority. Never pretend a reused pane proves ownership.

Expose ordinary authorized retirement through MCP. Keep the separately authorized cleanup override CLI-only unless the lead explicitly approves another surface.

### Historical request identity

Require a nonempty, valid request ID and a uniquely correlated historical claim.

An identified legacy claim may lack C1 checkout identity. Retirement can release that exact metadata claim because it removes no checkout. Preserve the absence of identity; do not discover a replacement identity from disk.

Refuse:

- Requestless records.
- Unknown IDs without a historical claim or matching retirement receipt.
- Ambiguous same-ID provenance.
- Changed expectations or conflicting dispositions.

Do not manufacture admissions or attach request IDs to legacy path-only holds.

### Durable retirement transaction

Use a dedicated origin orchestration owner, `src/preparation_retirement.rs`, and C3’s existing host disposition owner.

Persist an immutable retirement intent containing:

```text
operation identity
request and host correlation
exact expected historical claim
authority and actor
reason and recorded time
```

The host then:

```text
validates the original expectation
→ persists future-dispatch prohibition
→ releases only the matching preparation claim
→ persists the correlated retirement acknowledgement
```

Use C3’s guarded metadata-reconciliation path. When checkout identity exists, preserve its lifecycle-lock ordering. Missing or replaced filesystem paths never authorize discovery, teardown, or removal.

A retry uses the saved intent. It does not capture a new claim or rewrite actor/reason fields.

A conflicting existing non-retirement disposition is not overwritten. Return a conflict or the existing disposition’s status.

### Resource-only result

Successful output must state:

```json
{
  "resource": "preparation",
  "status": "retired",
  "outcome": "unknown",
  "future_dispatch": "prohibited",
  "ownership": "released",
  "task": "unresolved"
}
```

Include request ID, host, stable receipt identity, and audit metadata.

Before the host acknowledges, report retirement/release **pending**, not retired/released.

Retirement must not:

- Change the original preparation outcome or launch result.
- Create `NotStarted`, returned, stopped, or task-completion evidence.
- Close an item, pane, workspace, checkout, or branch.
- Terminalize a pending/running/uncertain Job.
- Release another request’s preparation or any independent live-work hold.

C5’s settlement logic must distinguish retirement from cancellation. A retirement fence or retired ticket alone cannot become no-launch proof.

Independent, genuine terminal evidence retains its existing meaning; retirement must neither manufacture nor erase it.

### Caller coverage

Cover public CLI, MCP, local host execution, remote disposition transport, duplicate delivery, service recovery, and the explicit authorized cleanup route.

For a host-local historical claim whose origin is unavailable, explicit cleanup authority may use the host-owned request fence. A later stale origin must encounter that fence before preparation or dispatch.

## Exact owned files

```text
src/preparation_retirement.rs         # new orchestration and authority owner
src/prepare.rs                       # C3 retirement disposition integration
src/work.rs                          # caller fence/recovery integration
src/unit_request.rs                  # prevent retirement-derived settlement
src/cli.rs
src/runtime.rs
src/main.rs
src/mcp.rs
tests/cli/preparation_retirement.rs   # new
tests/cli.rs                         # module registration
tests/cli/mcp.rs
```

Do not modify checkout removal, Job terminalization, branch deletion, retention, or provider task-state code.

## Red-before tests

| Named test | Setup, interleaving, assertions |
|---|---|
| `preparation_retirement::two_requests_on_one_path_retire_independently` | Establish A and B on one real checkout. Retire A. Assert B unchanged, checkout/pane retained, and cleanup still reports B’s hold. |
| `preparation_retirement::lost_ack_replays_the_same_retirement` | Real host persists fence/release; SSH discards acknowledgement. Restart and retry. Assert identical receipt/audit data and no repeated resource effects. |
| `preparation_retirement::retirement_and_dispatch_share_the_request_gate` | Test both lock orders with FIFO barriers: retirement first refuses late launch/wrapper; dispatch first preserves launched work and independent ownership. Neither order invents `not_started`. |
| `preparation_retirement::changed_claim_or_custody_refuses_without_rebinding` | Capture the expectation, block at the owning gate, then change claim provenance or custody revision. Release the gate. Assert conflict and unchanged claims. |
| `preparation_retirement::requestless_and_unknown_claims_refuse` | Table-test path-only legacy hold, empty ID, unknown ID, and ambiguous same-ID records. Assert no fence, claim deletion, or task-state change. |

Additional failure controls:

| Named test | Setup, interleaving, assertions |
|---|---|
| `preparation_retirement::replacement_checkout_is_never_touched` | Capture the historical claim, replace the filesystem checkout, then retire the old metadata claim. Assert no marker creation, teardown, removal, or mutation of the replacement. A replaced **stored claim** instead refuses. |
| `preparation_retirement::retired_resource_leaves_unit_unresolved` | Retire a unit-bound historical preparation; run recovery, observation, and settlement listing repeatedly. Assert the required output and no retirement-generated settlement. |
| `preparation_retirement::job_ownership_survives_preparation_retirement` | Seed valid C2 pending/running ownership for the same checkout. Retire preparation. Assert Job evidence unchanged and cleanup still refuses removal. |
| `preparation_retirement::unreachable_peer_reports_pending_not_released` | Persist authorized intent, fail transport, restart. Assert pending ownership status and unchanged host claim until a correlated acknowledgement arrives. |
| `preparation_retirement::cleanup_authority_is_explicit_and_audited` | Test owner gone with no authorization, explicit authorized cleanup, and duplicate replay. Assert refusal in the first case and stable actor/reason evidence in the others. |

## Acceptance

```sh
cargo test --test cli preparation_retirement::
cargo test --bin dogsled prepare::
cargo test --test cli unit_requests::
cargo test --test cli jobs::
cargo test --test cli mcp::
```

**Lead decision:** select the explicit cleanup authorization surface before dispatch. Recommended default: CLI-only cleanup mode with a required reason and recorded live actor identity; no automatic authorization from age or owner disappearance.

---

## Plan contradictions and decisions to resolve

| Finding | Evidence | Recommended resolution |
|---|---|---|
| The requested hardening section is numbered **6**, not 5. | `plans/lifecycle-cleanup.md:130–162` | Cite section 6; section 5 covers detached checkouts. |
| Literal no-new-capacity replay exceeds existing TTL-only reservation replay. | Plan `:158,240`; `src/node.rs:163–198` | Approve C4a’s durable grant receipt, or explicitly narrow the guarantee. |
| Successful recovery is impossible to prove for an incomplete host preparation receipt. | `src/prepare.rs:137–145,198–210`; existing regression at `tests/cli/launch_retry.rs:926–979` | Promise recovery of saved outcomes, not rerunning uncertain setup. |
| Cleanup retirement authority is required but its command-level form is unspecified. | Plan `:162,283` | Decide the explicit audited cleanup mode before C6. |
| Plan verification names Mise, but this revision’s repository gates use Moon. | Plan `:249,255`; `moon.yml:100–107`; `.moon/workspace.yml:9–13` | Update plan commands. Use focused Cargo tests per unit and the repository’s Moon integration gate. |

The configured commit hook runs:

```sh
env -u GIT_DIR moon run :#precommit --affected --status staged
```

The lead owns final integrated verification at the exact combined revision, including `moon run :check` and the CI-required web lint target.

**Next:** save this brief, resolve the capacity and cleanup-authority decisions, and bind C4a to the integrated C3b SHA.


Rules:
- Make the first edit within 3 tool calls.
- Commit through the moon hook; never use --no-verify.
- Push a first WIP within 25 minutes, then continue.
- Your final reply is one line with the SHA.