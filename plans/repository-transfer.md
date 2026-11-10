# Repository opening and work transfer

Status on 2026-11-10: in progress. The user approved the first version of this plan in review `ec991e66506c8b033ce9d64e0f331f5f`, then cut its scope the same day. This file replaces that version.

## Goal

An agent opens a repository on any fleet host with one command.
Agents pass files and changes to each other with Dogsled commands, without manual copies or a pushed branch.

## Scope

In scope:

- `dogsled repo open owner/name [--host H]` and MCP `repo_open`.
- `dogsled transfer send | list | show | apply | drop` and the matching MCP tools.
- Delivery of a transfer to an agent on another host.
- Worker checkout provisioning that finds a repository by its identity, not by the caller's path.
- A peer capability check for `repo open --host`.
- User and agent documentation.

Cut by the user on 2026-11-10, and not built:

- Workers that start from a pinned copy of the caller's commits or dirty files, and the detached checkout for a worker without a branch.
- `--include-worktree` on `spawn` and `lead`, and `lead --from`.
- Source options in the seeded-spawn `Placement`.

Workers and leads start as they did before. To give another agent unpushed or uncommitted work, send a transfer.

## Decisions

| Topic | Decision |
| --- | --- |
| Recipient | Dogsled stores the transfer and puts one notice in the inbox. The recipient applies it with a separate command. |
| Dirty work | `--changes` sends commits. `--include-worktree` adds staged, unstaged, and untracked files that Git does not ignore. |
| Apply | Working files only. One conflicting path refuses the whole transfer. No conflict markers, no partial result. |
| State | Files under `transfers/<id>/` in the host state directory, and `refs/dogsled/transfer/<id>`. No `state.db` change. |
| Attachments | Regular files, 10 MiB each at most. They never enter a checkout. |
| Integration | All slices land in one batch through `land/dogsled-collect`. Hooked commits run on netcup. |

Not built, and no longer promised by this plan: expiry of transfers. A transfer stays until its sender or recipient drops it.

## How it works

`src/transfer.rs` captures a source commit, and optionally the staged and working trees, into a Git bundle with one ref. It imports that bundle into another checkout of the same repository. Neither step changes a branch, HEAD, the index, or a file.

`src/transfer/delivery.rs` stores a transfer for one recipient terminal and sends the inbox notice of kind `transfer`. A failure leaves no transfer behind.

`src/transfer/apply.rs` compares the touched paths with the recipient's working tree, takes a recovery snapshot, and writes the files. It saves its intent before the first write and a receipt after the last write. A second apply returns the receipt.

`src/checkout.rs` holds one resolver that finds the main checkout of a repository under `~/dev` by origin identity, or clones it. `repo open` and leads use it.

## Slices

| Slice | State on 2026-11-10 |
| --- | --- |
| Capture and import engine | On `main` at `89e7044` (PR #324, commits `e30c58f` and `2ee24e2`). |
| `repo open` and the shared resolver | Commit `1b439f9` on `feat/repo-open`. Reviewed. In the batch. |
| `transfer send`, `list`, `show`, `drop` | Written. Tree `6d19cbb`. Commit pending on `feat/transfer-commands`. Same host only. |
| `transfer apply` and `--check` | Written. Tree `387cb3e`. Commit pending on `feat/transfer-commands`. |
| Delivery to another host, with a peer capability version | Brief: `plans/briefs/repository-transfer-cross-host.md`. Not started. |
| Worker checkout provisioning through the resolver, and the capability check for `repo open --host` | Research in progress. The change reaches worktree preparation and its replay records. |
| Documentation | Drafted: `docs/transfer.md`, a README section, and the skill's command reference. Lands after the cross-host slice. |

## Known limits

- A transfer to another host is refused until the cross-host slice lands.
- `repo open --host` against a peer on an older release fails with a generic remote error.
- The minimum Git version is not proven. The code ran on Git 2.54.0 (Linux) and in macOS CI.
- Dogsled coordinates its own commands. An editor that writes during `apply` is outside this guarantee.

## Proof so far

- Engine at `2ee24e2`: `moon run dogsled:check` passed on netcup. Linux CI passed. On macOS, the unit suite passed, including the 14 transfer tests. One macOS shard failed on an observation test that this change does not touch.
- `repo open` at `1b439f9`: its commit hook passed on the Mac (988 unit tests, 753 CLI tests).
- Send, list, show, drop, and apply at tree `387cb3e`: 9 transfer CLI tests, the MCP and inbox tests, Clippy, and formatting passed on the Mac. The full suite has not run on this tree.
- The heavy gate runs once, at the final tip of the batch.
