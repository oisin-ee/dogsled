# L3: prompt stopped children and close clean stopped orphans

Source: `plans/lifecycle-cleanup.md`, section 3. There are two units:

1. **-18** sends the stopped prompt (L3N).
2. **-19** closes clean stopped orphans.

Both units start from `origin/main` after **L2 (-16 and -17) has merged**. The derived `lifecycle::State` already gives `Stopped` when a turn ends with no `done`, no signal, no open children and no other evidence (`src/lifecycle.rs`).

## -18: one corrective prompt per stopped episode

Owned files: `src/watchdog.rs`, `src/watchdog_tests.rs` and `tests/cli/watchdog.rs`.

1. **What a stopped episode is:**
   - It is keyed by `{item id, pane, agent_session, turn}`, where `turn` uses the `done::Turn` from -16: `{state_change_seq, state_since_ms}`.
   - The episode starts when the watchdog observes a child whose derived state is `Stopped`.
   - A later turn (`Turn::precedes`), or any state other than `Stopped`, ends it.
2. **Prompt once:**
   - On the first observation of a new episode, send the child one message through the existing delivery path. This is the same path the watchdog check-in uses (`inbox` put to the child pane, with `Input::Put`).
   - The text is exactly: `End your turn with done, ask, or blocked, or wait for your children.`
   - Record the episode key and the sent message id in the watchdog state file, under the existing watchdog lock, BEFORE sending. Then mark it delivered after a successful put.
3. **Failed delivery:**
   - The recorded-but-undelivered episode retries on the next tick.
   - Use the same message id, so the inbox deduplicates it. There is never a second distinct prompt.
   - Retry for at most 3 ticks, then stop and keep the row under NEEDS YOU with the reason `stopped; prompt undelivered`.
4. **No loop:** the prompt itself causes a new turn. If that turn ends `Stopped` again, it is a new episode, but no new prompt is sent unless that turn did something. The rule:
   - Do not prompt an episode whose turn began within 120 s after our prompt for the previous episode of the same session.
   - Record `prompted_after` so the rule survives restart.

   One prompt per genuine stop: at most one prompt within any 120 s window per session, and never two in a row with no agent turn activity between them.
5. **Owner view:** a stopped child whose owner is live stays under NEEDS YOU, with the activity text `stopped; prompted <age> ago` or `stopped; prompt undelivered`. Reuse the board activity path and do not add a new group.
6. Tests in `tests/cli/watchdog.rs` and `src/watchdog_tests.rs`. Each must fail on the base.
   - A stopped child gets exactly one prompt across 5 ticks.
   - A failed put retries with the same id, and gives up after 3.
   - Prompt → turn → stopped again within 120 s gives no second prompt.
   - Prompt → real working turn → stopped after 120 s gives a second prompt.
   - Working, waiting, asking, blocked, finishing, unknown and lost children get no prompt.
   - The episode key survives a watchdog state reload.

Acceptance:

```
cargo test --test cli watchdog::
cargo test --bin dogsled watchdog
```

## -19: close clean stopped orphans

This unit runs after -18 merges.

Owned files: `src/sweep.rs`, `src/items.rs`, `src/close.rs` and `tests/cli/sweep.rs`.

Current code: `sweep::settle` (`src/sweep.rs:422`) closes an orphan only when the raw state is `idle` or `done` AND `agent.done` is true. Otherwise it keeps the orphan.

1. **Add the stopped-orphan rule to `settle`:**
   - The orphan's derived lifecycle state is positively `Stopped`. Use `lifecycle::of_agent` with `Host::Up` on the reported agent, not raw `agent.state`.
   - Owner absence is already proven by `classify` (`orphaned`, not `unknown`).
   - The checkout is clean and pushed. Use the existing `Guard::Pushed` close path, which refuses dirty or unpushed work.

   When all three hold:
   - record the child's last reply on the item with a new field `Item.orphan_reply: Option<Reply>`. Use the pane tail through `watchdog::item_tail` or the transcript final message. **Never use `set_reply` or `native_done`**: this is not a result;
   - close exactly as an owner close would;
   - log `dogsled sweep: closed stopped orphan <pane> on <host>; reply recorded; task not completed`.
2. **Unit-bound orphans:** if `unit_request::managed_returned` shows a unit binding that hasn't returned, close the resources only:
   - the pane and the worktree, through the same guarded close;
   - leave the unit binding unresolved, with no acknowledgement and no settlement;
   - record `Item.resource_closed_ms` and the reason `resource-only close; task unresolved`.
3. **Protected states:** working, waiting, asking, blocked, finishing, unknown and lost never close. A dirty or unpushed stopped orphan stays with the reason `stopped orphan has unsaved work; kept until preservation (P3)`. P3 adds saving later.
4. Tests in `tests/cli/sweep.rs`. Each must fail on the base.
   - A clean stopped orphan closes, its reply is recorded, and the item has no result reply.
   - A dirty stopped orphan stays with the P3 reason.
   - A stopped child with a live owner stays.
   - Each protected state stays.
   - A unit-bound stopped orphan gets the resource-only close, and the binding stays unresolved.
   - The existing done-orphan close still works.

Acceptance:

```
cargo test --test cli sweep::
cargo test --test cli close::
```

## Rules for both units

- Make the first edit within 3 tool calls.
- Push within 20 minutes, and again after each working step.
- Use the moon hook; never use `--no-verify`. Retry hook flakes up to 3 times.
- Edit only the owned files.
- The final reply is one line with the SHA.
