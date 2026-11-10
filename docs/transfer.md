# Send files and changes to another agent

A transfer stores files or changes for another agent and puts one notice in its inbox.
A transfer never changes the recipient's checkout. The recipient applies changes with a separate command.
A transfer does not push a branch, complete a task, or close an agent.

## Send

```sh
dogsled transfer send worker --files notes.md screenshot.png
dogsled transfer send worker --changes
dogsled transfer send worker --changes --since HEAD~2
dogsled transfer send worker --changes --include-worktree
dogsled transfer send w9:p1 --host netcup --changes
```

The target is an agent name or a Herdr pane, as for `dogsled send`.
The recipient must have an inbox reader. Without one, the command fails and stores nothing.
The transfer belongs to the recipient's terminal. If another terminal reuses the pane, it does not see the transfer.

### Files

`--files` attaches regular files. Each file can have 10 MiB at most.
A directory, a symlink, a path inside `.git`, or two files with one name stops the command.
The recipient reads the files where Dogsled stores them. The files do not go into a checkout.

### Changes

`--changes` sends the commits of the current checkout, including commits that are not on the remote.
`--since REV` sets the commit that the recipient compares with.
Without `--since`, Dogsled uses the commit where the checkout left the default branch.
Uncommitted work stays where it is. `--include-worktree` also sends staged, unstaged, and untracked files that Git does not ignore.
Ignored files are never sent. If there is no change since the comparison commit, the command fails.

Sending does not change the sender's branch, index, or files.
Dogsled keeps the sent commit under `refs/dogsled/transfer/<id>` until someone drops the transfer.

## Read

```sh
dogsled transfer list
dogsled transfer show TRANSFER_ID
```

`list` shows the transfers that you sent or received on this host.
`show` prints the summary, the paths of the stored files, and the sent commit.
If a stored file no longer has its recorded bytes, `show` fails.

## Apply

```sh
dogsled transfer apply TRANSFER_ID --check
dogsled transfer apply TRANSFER_ID
```

Run `apply` in the checkout that receives the changes. Only the recipient can apply a transfer.
`apply` writes working files only. It does not change the index, HEAD, or a branch.
Your staged work, your other edits, and your untracked files stay as they are.

If one touched path differs from the comparison commit, `apply` writes nothing and names the paths.
This includes an ignored file at a path that the transfer adds.
`--check` gives the same answer and writes no file.

Before the first write, Dogsled takes a recovery snapshot of the checkout and prints its ID.
A second `apply` of the same transfer writes nothing and prints the first result.
If an `apply` stops before it finishes, later `apply` commands fail and name the recovery snapshot.
Compare the checkout with that snapshot (`dogsled recovery show ID --repo PATH`), then drop the transfer.

Dogsled coordinates its own commands only. Do not edit the touched files while `apply` runs.

## Drop

```sh
dogsled transfer drop TRANSFER_ID
```

The sender or the recipient can drop a transfer. This removes the stored files on this host.
In the sender's checkout, it also removes the transfer ref.
Transfers do not expire. Drop a transfer when you do not need it.

## MCP tools

`transfer_send`, `transfer_list`, `transfer_show`, `transfer_apply`, and `transfer_drop` take the same inputs and return the same JSON.
`transfer_send` takes `files`, or `changes` with optional `since` and `includeWorktree`.
`transfer_send` and `transfer_apply` take an optional `cwd` for the checkout.
