# Repository opening and work transfer

Status on 2026-11-10: built; landing in one batch on `land/dogsled-collect`. The user approved the first version of this plan in review `ec991e66506c8b033ce9d64e0f331f5f`, then cut its scope the same day. This file replaces that version.

## Goal

An agent opens a repository on any fleet host with one command.
Agents pass files and changes to each other with Dogsled commands, without manual copies or a pushed branch.

## Scope

In scope:

- `dogsled repo open owner/name [--host H]` and MCP `repo_open`.
- `dogsled transfer send | list | show | apply | drop` and the matching MCP tools.
- Delivery of a transfer to an agent on another host.
- Worker checkouts on another host at a standard path, `~/dev/<name>`, found from the repository identity and not from the caller's path.
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
| Integration | All slices land in one batch, as linear commits on `land/dogsled-collect`. Hooked commits run as jobs on a fleet host, not on the Mac. |
| Worker path on a peer | `dev/<name>` from the origin identity (user, 2026-11-10: agents must not need to know where a repository lives). A repository without a remote identity keeps its home-relative path. The preparation envelope format does not change. |
| Cross-host transfer | One stdin envelope to `host transfer-put`. The peer checks it all before a write, stores it once, and writes the notice. Two lost replies leave the sender copy and report the result as uncertain. |

Not built, and no longer promised by this plan: expiry of transfers. A transfer stays until its sender or recipient drops it.

## How it works

`src/transfer.rs` captures a source commit, and optionally the staged and working trees, into a Git bundle with one ref. It imports that bundle into another checkout of the same repository. Neither step changes a branch, HEAD, the index, or a file.

`src/transfer/delivery.rs` stores a transfer for one recipient terminal and sends the inbox notice of kind `transfer`. A failure leaves no transfer behind.

`src/transfer/apply.rs` compares the touched paths with the recipient's working tree, takes a recovery snapshot, and writes the files. It saves its intent before the first write and a receipt after the last write. A second apply returns the receipt.

`src/transfer/peer.rs` carries a stored transfer to a peer host and receives one there. A peer that does not report `transfer: 1` in its snapshot gets no envelope.

`src/checkout.rs` holds one resolver that finds the main checkout of a repository under `~/dev` by origin identity, or clones it. `repo open` and leads use it. `checkout::peer_relative` gives the path a worker checkout has on a peer. `repo open --host` first requires `repo_open: 1` in the peer's snapshot.

## Slices

| Slice | State on 2026-11-10 |
| --- | --- |
| Capture and import engine | On `main` at `89e7044` (PR #324, commits `e30c58f` and `2ee24e2`). |
| `repo open` and the shared resolver | Commit `1b439f9` on `feat/repo-open`. On `land/dogsled-collect` before this plan's steps. |
| `transfer send`, `list`, `show`, `drop` | Tree `6d19cbb`. Its hook passed alone as `fbe6744`. On `land/dogsled-collect`. |
| `transfer apply` and `--check` | Tree `387cb3e`. On `land/dogsled-collect`. |
| Delivery to another host, with a peer capability version | Tree `4dde69a`, with the integration lead's split of `fn host` in `src/main.rs`. On `land/dogsled-collect`. |
| Standard worker path on peers, and the capability check for `repo open --host` | Tree `98edbfa`, on `1b439f9`. On `land/dogsled-collect`. |
| Documentation | Tree `e0708dc`: `docs/transfer.md`, a README section, and the skill's command reference. On `land/dogsled-collect`. |

## Known limits

- Transfers do not expire. The sender keeps no outbox entry for a transfer sent to another host.
- A cross-host envelope is held in memory at both ends; attachments are 10 MiB each at most.
- Cross-host delivery is proven with a fixture peer, not with two real hosts.
- The minimum Git version is not proven. The code ran on Git 2.54.0 (Linux) and in macOS CI.
- Dogsled coordinates its own commands. An editor that writes during `apply` is outside this guarantee.

## Proof so far

- Engine at `2ee24e2`: `moon run dogsled:check` passed on netcup. Linux CI passed. On macOS, the unit suite passed, including the 14 transfer tests. One macOS shard failed on an observation test that this change does not touch.
- `repo open` at `1b439f9`: its commit hook passed on the Mac (988 unit tests, 753 CLI tests).
- Send, list, show, and drop: the commit hook passed at `fbe6744`.
- Cross-host at tree `4dde69a`: 12 transfer CLI tests (3 cross-host) and 2 envelope unit tests passed on the Mac.
- Standard path at tree `98edbfa`: `place`, `remote_diagnostics`, `repo_open`, `lifecycle`, `lead`, and the checkout and prepare unit tests passed on the Mac.
- Each step of the batch is committed through the commit hook on netcup. The heavy gate runs once, at the final tip of `land/dogsled-collect`.
