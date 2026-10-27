# Review J1 repair 6 (delta only, bounded)

## Outcome
PASS or BLOCK on integrating J1 into local `main` at `cbb06e3f398a435aacd312b8cd23029d660bb70e`. Read-only.

## Scope
- Review `7ca7b44..cbb06e3f398a435aacd312b8cd23029d660bb70e` only, on branch `fix/lifecycle-job-claims-r3`. It changes `tests/cli.rs` only.
- Everything before `7ca7b44` has been reviewed already. The product code was confirmed correct at `a6970b2` and has not changed since.
- Contract: `plans/briefs/lifecycle-job-claims-repair-6.md`.

## The finding this change must close
At `7ca7b44`, the bounded-drop regression's timeout fallback called `killpg` on a cached group ID outside the `Owned` guard (`tests/cli.rs:1268–1277`). That could signal after a reap.

## Blocking bar
- **BLOCK only if this delta is wrong.** That means one of:
  - it does not close the finding above;
  - it introduces a new way to signal after a reap, hang without bound, or leave a stray process;
  - it weakens an assertion.
- **Report everything else as follow-up, not as a block.** This includes older test patterns and processes outside the command group. Five rounds have been spent on test-harness findings, so the user wants J1 landed.

## Writer claims (untrusted; do not rerun)
The acceptance command passes, no strays remain, and the hook passed.

## Reply
PASS or BLOCK, then findings with severity and file:line at `cbb06e3`, each marked **blocking** or **follow-up**.

## Limits
Read-only. Use the seeded unit and native done.
