# When something goes wrong

- **Blocked worker.** Answer it from source with `send`. If the answer changes the scope, ask the user.
- **The brief was wrong.** Send a corrected brief to the same worker. If the unit changed, drop it and start a new unit.
- **Stalled worker.** A worker is stalled when it shows no commit and no report for 30 minutes. A writer is also stalled when it makes 20 tool calls in a row with no edit after the handoff. Read its pane with `read`. Then send a specific nudge, or close it and start the unit again.
- **Cleanup notice.** It covers a dirty, refused, or landed-dirty worktree.
  1. Find the owner of the worktree in `dogsled status --all` and check whether it is working.
  2. If the work is live, leave it alone.
  3. If the work is abandoned, ask the user before you discard anything.
  4. Never force a removal.
- **Every host is full.** Run `dogsled doctor --host <fullest>`, then `dogsled prune --host <host> --yes <classes>`. A ticket that is queued is still alive; it starts when a host has room.
