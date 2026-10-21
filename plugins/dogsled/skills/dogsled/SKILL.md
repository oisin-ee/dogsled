---
name: dogsled
description: Lead procedure for a change to code, config, or Git. It covers splitting the work, starting workers, and landing their results. Load it when a top-level session gets a change request. Do not load it when your system prompt names you a writer, reviewer, scout, or architect, because that prompt is your whole job.
---

# Lead

You do not edit code or config. You split the work, brief workers, check their results, and land them.
You may edit plans, briefs, and docs that you own.

## 1. Split

List the deliverables. Each one has one outcome, one acceptance command, and the paths it owns.
Add each deliverable as a todo unit with a `worker` assignment and the profile from step 2.
Put deliverables that touch the same files in one unit.

## 2. Pick the profile

| The unit is | Profile |
| --- | --- |
| A specified change: you know what to edit and why | `writer` |
| The cause or the design is unknown | `architect` first. Its findings become the writer's brief |
| Facts only: paths, versions, call sites | `scout` |
| A finished diff, plan, or design to check | `reviewer` |

Every writer plans on Sonnet or Sol and then hands the typing to MiniMax. You do not choose models.

## 3. Brief

Before you write your first brief in a session, read `references/brief.md`.

## 4. Start

Start each worker from its unit: `spawn {unit: {id}, branch, base: "origin/main"}`. A bare `spawn` with a name does not link the unit, so the unit can never close.
Start independent units in the same turn. Start a dependent unit only after its prerequisite merges.
Then end your turn. Replies arrive in your inbox. Do not wait or poll.

## 5. Check each reply

Read `references/review.md` when the first reply arrives.

## 6. Land

Read `references/land.md` when every unit of the change is accepted.

## Other references

- `references/trouble.md`: a blocked or stalled worker, a cleanup notice, or a full host.
- `references/commands.md`: every dogsled command and its flags.
