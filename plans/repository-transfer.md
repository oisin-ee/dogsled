# Repository opening and work transfer

Status: design proposal. The user selected the two defaults below on 2026-11-10. Implementation has not started.

## Goal and selected defaults

Dogsled opens the correct repository on a fleet host without manual cloning, path translation, temporary pushes, or file copying.
Leads and workers use the same repository preparation contract.
Agents can also send files and changes to an existing agent.

| Topic | User decision |
| --- | --- |
| Worker source | Committed state only by default. Dirty work requires an explicit request. |
| Existing recipient | Deliver a saved transfer and an inbox notification. The receiving agent explicitly applies it. |

Committed state includes local commits that are not on the Git remote.
Transfer does not publish a branch, complete a task, accept a result, or close an agent.

## Owning cause and current evidence

Repository provisioning identifies a destination, but launch requests do not identify and transport the exact source content.
Lead and worker provisioning also use different destination rules.
The proper fix belongs in shared repository preparation and a Dogsled-owned transfer operation.

Source inspected at `b3df47fe5c2c6856eb2a9db0a17d2e5a14878a72`:

| Owner | Current contract and gap |
| --- | --- |
| `src/leads/checkout.rs::ensure` | Finds a main checkout by origin under `~/dev`, or clones one. It does not select or transport the caller's revision. |
| `src/checkout.rs::{discover,ensure,local}` | Carries origin and home-relative paths. A worker can fail on a path collision despite another matching checkout. |
| `src/work.rs::{prepare_launch,prepared_launch}` | Prepares a branch or uses a checkout path. Local no-branch work can share the caller's dirty directory. |
| `src/prepare.rs::local_guarded` | Resolves a base on the destination. An unresolved `origin/<branch>` can trigger a fetch, but no source objects travel with the request. |
| `src/preservation.rs::{snapshot,restore}` | Captures raw staged and working content. Restore uses an absent destination and owns no launch, setup, or task state. |
| `src/cli.rs::{RepoCommand,RecoveryCommand}` | Exposes repository listing and recovery list/show/restore, not general work transfer. |
| `src/unit_request.rs::prepare`, `docs/unit-requests.md` | Prepare does not reserve capacity, provision checkouts, or start workers. Commit owns execution. |
| `src/unit_transport.rs::Placement`, `pi/unit-protocol.ts::unitInput` | Seeded spawn currently accepts host, branch, and base. Its strict schemas need coordinated changes. |

> 🧠 **From Hindsight memory (Fleet leads and portal New button)** — Repository identity and host-independent lead placement are established requirements. The source above confirms the current lead path.

> 🧠 **From Hindsight memory (Recovery inspection and independent restore)** — Recovery is separate from task completion, and recovery refs remain local. Transfer must preserve that boundary.

## Proposed public contract

These commands are proposed, not available commands.
The CLI and MCP tools share one implementation and return structured receipts.

```sh
# Prepare a repository checkout and workspace without starting an agent.
dogsled repo open owner/name --json
# Explicit host selection is optional for agent or operator use.
dogsled repo open owner/name --host netcup --json

# A slug-only lead uses the repository's default branch.
dogsled lead owner/name "Investigate the failure"
# An explicit source uses that checkout's committed HEAD.
dogsled lead owner/name "Continue this work" --from .
# Dirty work travels only on request.
dogsled lead owner/name "Continue this work" --from . --include-worktree

# Existing worker commands gain the same source option.
dogsled agent --name worker --message 'Implement the task' --branch feat/task --include-worktree

# Send changes since an explicit committed base.
dogsled transfer send worker --changes --since HEAD~1
# Include staged, unstaged, and non-ignored new files.
dogsled transfer send worker --changes --since HEAD --include-worktree
# Send named files as attachments, not edits to the recipient's checkout.
dogsled transfer send worker --files notes.md screenshot.png

# Inspect and explicitly apply a received change transfer.
dogsled transfer show TRANSFER_ID
dogsled transfer apply TRANSFER_ID --check
dogsled transfer apply TRANSFER_ID
```

`repo open` returns the repository identity, host, main checkout path, workspace, current revision, and dirty-state summary.
It locates or clones the main checkout without switching its branch or changing its index and files.
It does not grant shared-write authority or create a disposable agent worktree.
Launch preparation separately creates an isolated checkout from the selected source.

Lead and worker launch call this shared preparation internally.
The user does not run `repo open` before each spawn.
Host placement retains existing lead and worker capacity rules.
No host selection is required for normal spawning.

MCP exposes `repo_open`, `transfer_send`, `transfer_show`, and `transfer_apply`.
Spawn accepts `includeWorktree`. Lead also accepts `from`.
Seeded spawn carries these execution options through its placement contract, not through task prose or a changed task export digest.

### Source selection

| Request | Source |
| --- | --- |
| Worker from a repository, without `--base` | The caller's committed HEAD, including unpushed commits. |
| Worker with `--base` | That base resolved once at the origin, not independently on each host. |
| Slug-only lead | A freshly resolved repository default-branch commit, then pinned for this request. |
| Lead with `--from PATH` | The committed HEAD of that checkout. Its repository identity must match the slug. |
| Request with `--include-worktree` | Source HEAD plus separate staged and working trees, including non-ignored untracked files. |

A missing or unreadable source fails explicitly. Dogsled does not substitute another revision.
An explicit base and `--include-worktree` are incompatible in this initial contract.
A caller must select the intended checkout before requesting its dirty state.
Ignored files are excluded from whole-checkout capture.
Named attachments are explicit file selections, including an ignored file only when the caller names it directly.
No directory-wide inclusion of ignored files is implicit.

Local and remote launches produce the same source state in isolated checkouts.
Existing destination checkouts remain on their branches with their index and files unchanged.
Writer branches are explicit. Read-only starts can use detached checkouts.
An occupied or differently based destination branch fails without reset or forced reuse.
These source defaults apply to agent launches, not existing `run` or `place` commands.
Repository jobs, non-repository jobs, and plain placement retain their existing behavior.

### Sending files and changes

`--files` and `--changes` are distinct transfer kinds and cannot appear together.
File attachments remain in private transfer storage and appear as local paths in the recipient's inbox.
They do not overwrite repository files and do not invoke `apply`.
Only explicitly named regular files are attachments in the first implementation.
Directories, special files, Git administrative paths, and symlink traversal fail explicitly.

Change transfers retain repository identity, before and after content, modes, and a digest.
A managed worker can omit `--since` to use its saved launch baseline.
Other senders must provide a base instead of relying on a guessed merge base.
For a dirty launch baseline, outgoing changes exclude the work originally supplied by the parent.
Selected repository paths can narrow a change transfer without changing its baseline.

Recipients use existing host and agent addressing.
Dogsled resolves the recipient to its current session and terminal before delivery.
A reused pane or changed session cannot inherit a pending transfer.
The inbox contains a manifest summary, transfer ID, and local inspection command, not raw file contents.

### Applying changes

Application is explicit and targets the receiving checkout.
`--check` reports applicability without changing its index or files.
An ordinary apply changes working files only and preserves existing staging.
Exact staged/unstaged restoration belongs to fresh-checkout launch, not application over existing work.

Dogsled validates repository identity, path containment, the expected before-content, and destination identity before writes.
Unrelated dirty files and staged work remain intact.
Conflicting content, path collisions, or unsupported state produce a refusal and retain the transfer.
The first implementation does not resolve conflicts automatically or write conflict markers into a live checkout.
It must not use force, partial rejection, whitespace rewriting, or unsafe-path modes.

Before a live apply, Dogsled records the operation and preserves the relevant destination state.
An interrupted apply remains visible as unresolved until its saved evidence proves the outcome.
A retry cannot blindly reapply against changed files.
The lifecycle mutex coordinates Dogsled operations, not arbitrary editors.
Implementation must not claim protection from concurrent external writers.

## Ownership and transport

### Shared repository preparation

Extract the common repository resolver from the lead-specific path.
Keep repository identity separate from transport URLs, host-local main paths, and working checkout paths.
Reuse verified host-owned checkouts and existing collision handling.
Do not recreate the removed hand-maintained repository catalogue.

The main checkout supplies Git objects and repository identity, not permission to alter its live branch.
Preparation owns the isolated checkout and registers its resource identity before launch.
Existing worktree setup runs on the selected source content before the agent starts.
Delivery and inspection alone never run repository setup or teardown.
Transferred bytes do not bypass existing repository trust and setup policy.

### Saved content and peer delivery

Use native Git object transport for committed history and captured repository trees.
Reuse the raw-content capture and validation mechanisms in `src/preservation.rs` where their contracts fit.
Separate transfer metadata and refs from close-recovery metadata and refs.
Do not turn a transfer snapshot into a synthetic commit on the user's branch.

The proposed object container is a Git bundle with an explicit, request-owned ref.
Git bundles carry objects and refs. They do not capture the index or working files automatically.
Dogsled must capture those trees first for an explicit dirty transfer.
Bundle verification must validate prerequisites before import.
Import only the intended refs into a Dogsled namespace, never all branches or recovery refs.

Use the existing authenticated fleet transport with bounded binary payload handling.
Do not place file contents on command lines, in task prose, or inside ordinary status publications.
The receiver verifies the manifest, digest, object format, prerequisites, paths, and limits before publishing a receipt.
No user branch must be pushed to GitHub to complete a transfer.

File attachments use the same transfer identity and receipt protocol, with a bounded file manifest rather than a Git history.
The receiver stores complete, verified payloads before issuing the inbox notification.
Lost replies reuse the recorded receipt and cannot deliver another payload under the same identity.

### Request and resource lifetime

Freeze source content before queue admission, not when capacity becomes available.
For seeded workers, `prepare` retains source selectors without provisioning or capture effects.
At `commit`, freeze source content once before admission and bind its digest to the execution request.
Repeated commits reuse that content. Changed source arguments or digests conflict.

Check peer capability before destination allocation or setup.
An unsupported peer fails explicitly without dropping the source options.
Retain source artifacts through queued, cancelled-but-unresolved, or uncertain launches.
Cancellation does not delete the only copy of transferred work.

Delivery acknowledgement, apply acknowledgement, and task completion are separate facts.
The service owns transfer retention, outside recovery-ref retention.
Pending, uncertain, or referenced transfers cannot expire.
A terminal apply receipt must survive payload cleanup so replay cannot apply again.
The retention duration and explicit disposal interface must be specified before the transport unit is dispatched.

Lead closure retains its current pane-only meaning.
New lead checkouts require durable ownership and remain visible after explicit lead close.
This feature must not silently add destructive lead cleanup.
`repo open` leaves a normal reusable main checkout, not an unowned temporary worktree.

## Alternatives and evidence

| Approach | Decision |
| --- | --- |
| Continue separate lead and worker paths | Reject. Host paths and source selection remain inconsistent. |
| Require push/fetch before spawn | Reject. It requires publication for private, local work and cannot carry dirty state alone. |
| Copy the working directory between hosts | Reject. It does not establish source revision, staged state, or request ownership. |
| Use a plain patch as the entire transport | Reject for launch state. It does not carry commit ancestry or separate staged and working trees. |
| Shared resolver plus saved content and native Git transport | Propose. It reuses Dogsled's preparation, recovery primitives, fleet transport, and inbox. |

Primary Git documentation inspected with local Git `2.56.0`:

- [Git bundle description and prerequisites](https://git-scm.com/docs/git-bundle#_description): bundles carry objects and refs. Incremental bundles require existing prerequisite history.
- [Git bundle reference selection](https://git-scm.com/docs/git-bundle#_specifying_references): bundle creation requires named refs, not only a raw object ID.
- [Git apply options](https://git-scm.com/docs/git-apply#_options): normal applicability failure leaves the patch unapplied. `--3way` can write conflicts and implies index handling.

These sources support the transport proposal, not proof of Dogsled implementation.
Fleet Git versions, object-format compatibility, shallow history, missing objects, and size limits require implementation evidence.
Unsupported submodules, nested repositories, unresolved indexes, sparse checkouts, and external filter content need explicit supported-or-refused behavior.
No case can silently lose content or substitute a different source.

## Delivery sequence

These are planning slices, not executable assignments.
Before dispatch, each slice needs exact owned paths, acceptance commands, and one session-sized deliverable.
Shared CLI and MCP edits land in sequence, not through concurrent owners.

| Slice | Deliverable and likely owners | Prerequisites | Acceptance |
| --- | --- | --- | --- |
| R1 | Shared resolver and `repo open`: `src/checkout.rs`, `src/leads/checkout.rs`, CLI/MCP, repository tests | None | Correct identity on local and simulated remote hosts. Collisions and checkout reuse preserve existing work. |
| T1 | Saved source and change manifests; transfer module and narrow preservation reuse | Source schema | Round-trip committed, staged, unstaged, binary, deleted, renamed, and untracked content without changing the source. |
| T2 | Peer transport, receipts, attachment delivery, send/show commands | T1, retention and limits specified | Lost replies and interrupted uploads cannot duplicate delivery. Invalid payloads and stale recipients fail before notification. |
| A1 | Explicit check/apply operations | T2 | Non-conflicting dirty destinations preserve unrelated bytes and staging. Conflicts and replay never overwrite newer work. |
| S1 | Ordinary worker source capture and preparation | R1, T2 | Local and remote workers receive the same pinned state. Queue delays and unpublished commits do not change the source. |
| S2 | Seeded worker placement and Pi transport | S1 | Strict Rust/TypeScript schemas agree. Prepare remains effect-free and commit/replay preserve source identity. |
| L1 | Lead source options through CLI, MCP, and existing HTTP launch | R1, T2 | Slug-only and explicit-source leads use shared preparation without inheriting unrelated dirty work. |
| D1 | Agent instructions, command documentation, combined proof | A1, S2, L1 | Documented workflows work without manual copies or temporary pushes. All supported surfaces obey the selected defaults. |

R1 and T1 can proceed independently after their contracts are ready.
S1 and L1 share launch-facing contracts and must coordinate their write scopes.
No portal layout work or fleet installation belongs to these slices.

## Required proof

| Area | Required cases |
| --- | --- |
| Repository identity | Missing checkout; existing matching checkout at another path; same-name repositories; SSH/HTTPS identity; wrong origin; occupied destination. |
| Source state | Unpushed and detached commits; moving branch names; dirty source excluded by default; explicit staged/unstaged/untracked capture; explicit base precedence. |
| Preservation | Binary bytes, modes, symlinks in repository trees, deletions, unusual names, ignored-file exclusion, source index unchanged, unsupported-state refusal. |
| Transport | Host interruption; corrupt or incomplete payload; missing prerequisites; unsupported peer; duplicate receipt; session replacement; expiry holds. |
| Integration | Seeded and ordinary workers, lead CLI/MCP/HTTP, existing-agent attachments, explicit apply over unrelated dirty work, conflict refusal, uncertain-apply replay. |

CLI tests must own their child processes, bound waits, and reap their children on every exit path.
Use the repository test rigs rather than permanent agents or real user checkouts for failure injection.
Run focused tests per slice, then `moon run dogsled:check` on the combined revision.
Include the affected portal checks if shared HTTP types change.
Keep required hooks enabled and inspect the final scoped diff.
A plan commit is not implementation proof, release approval, publication, or installation.
