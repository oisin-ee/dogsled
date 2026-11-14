---
name: dogsled
description: Lead procedure for a change to code, config, or Git. It covers when to start workers, how to split and brief the work, and how to land the change. Load it when a top-level session gets a change request. Do not load it in an exec session given prewalk. Do not load it in a worker session, because the worker profile prompt is your whole job.
---

# Exec agents given prewalk

If you are an exec agent given prewalk, this procedure is not for you. Do not delegate. Do not spawn workers. Execute the task locally. Stay in the same session through the prewalk model handoff.

# Lead

You own the change from plan to landing. By default, you make it yourself in this session.
Start writers only when step 0 says the work needs them. Then you split the work, brief workers, check their results, and land them.
You may edit plans, briefs, and docs that you own.

## 0. Do the work yourself unless it needs parallel writers

Plan, edit, and test the change in this session. Fix review and integration findings yourself.
Start writers only when all of these are true:

- The work splits into two or more units that can run at the same time.
- Each unit owns different files behind a fixed interface.
- Each unit has its own pass or fail check.
- The units together do not fit in one session.

Never split a plan from its implementation, an implementation from its tests, or review fixes from the change.
A `research` worker for an independent question, or for a review of the finished diff, does not need these conditions (step 2).
If the user tells you to delegate, or not to delegate, do that. Never cite this skill to refuse.
If every host refuses or fails the spawn, do the units yourself, one at a time.

When you do the work yourself, keep the scope of the request. Run the repository's checks as a writer would. Then go to step 6.
Otherwise, follow steps 1 to 5.

## 1. Split

List the deliverables. Each one has one outcome, an acceptance condition, and the paths it reads or owns.
Add each deliverable as a todo unit with a `worker` assignment and the profile from step 2.
Put deliverables that touch the same files in one unit.
Put small specified edits of one request in one unit, even across files. Never start one worker per small edit.
You can keep one unit for yourself with a `self` assignment.

## 2. Pick the profile

Use only these two worker profiles:

| The unit is | Profile |
| --- | --- |
| Read-only investigation, diagnosis, design analysis, or review | `research` |
| A specified change: you know what to edit and why | `writer` |

Investigate ordinary questions in the lead session.
Delegate research only for a defined question that warrants a separate worker.
An unknown cause or design does not require a research worker.
Research produces evidence, not file changes or more agents.

The default writer uses GPT Sol for the whole session, with no typing-tier handoff. The profile owns model selection and effort.

## 3. Brief

Before you write your first brief in a session, read `references/brief.md`.

## 4. Start

Start each worker from its unit.
For a writer, use `spawn {unit: {id}, branch, base: "origin/main"}`.
For research, use `spawn {unit: {id}}` without a branch or base.
A bare `spawn` with a name does not link the unit, so the unit can never close.
Start independent units in the same turn.
Start a dependent unit only after its prerequisite is accepted and any prerequisite code changes are merged.
Then work on your `self` unit, if you kept one, or end your turn. Replies arrive in your inbox. Do not wait or poll.

## 5. Check each reply

Read `references/review.md` when the first reply arrives.

## 6. Land

Read `references/land.md` when every unit of the change is accepted.

## Other references

- `references/trouble.md`: a blocked or stalled worker, a cleanup notice, or a full host.
- `references/commands.md`: every dogsled command and its flags.
