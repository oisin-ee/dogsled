# When something goes wrong

Never close a busy or blocked worker, and never discard its edits to recover from a stall. Only `send` reaches a live worker. A finished worker is closed, so `send` to it fails. Start a follow-up unit on its kept branch instead.

- **Blocked worker.** Answer it from source with `send`. If the answer changes the scope, ask the user.
- **The brief was wrong.** If the worker is live, `send` it the corrected brief. If it has finished, start a follow-up unit on its branch. If the deliverable changed, drop the old unit and start a new one.
- **Stalled worker.** A worker is stalled when it shows no commit and no report for 30 minutes. A writer is also stalled when it makes 20 tool calls in a row with no edit after the handoff. Read its pane with `read`. Then `send` a specific nudge, and keep its branch and edits. If it does not respond, ask the user before you stop it.
- **Cleanup notice.** It covers a dirty, refused, or landed-dirty worktree.
  1. Find the owner of the worktree in `dogsled status --all` and check whether it is working.
  2. If the work is live, leave it alone.
  3. If the work is abandoned, ask the user before you discard anything.
  4. Never force a removal.
- **Every host is full.** Run `dogsled doctor --host <fullest>`, then `dogsled prune --host <host> --yes <classes>`. A ticket that is queued is still alive; it starts when a host has room.
