# Workflow in the dogsled skill

Status: proposal. Nothing here is applied yet.

## Problem

The workflow lives in five places that disagree:

- `AGENTS.md` and `CLAUDE.md` (rendered from `rules/root.tera`) say "Do a single unit yourself" in their top line.
- `~/.agents/rules/20-delegation.md` repeats that rule, and adds the profile table and the spawn mechanics.
- `10-evidence-and-continuity.md` and `30-safety-and-delivery.md` hold the check cadence and the delivery process.
- The profile prompts point back to `~/.agents/rules`.
- The dogsled skill describes the tools only.

What the fleet does as a result (dogsled status and session logs, 2026-10-21):

- **Leads write code.** They follow "one unit: do it yourself" on Opus or Astra at high effort.
- **Writers mostly run on Opus or Astra.** In `fix-cleanup-branch-identity`, Opus took 28 turns before the prewalk handoff and Sonnet took 4. The lead had already planned the unit, so the planning was paid for twice.
- **Leads started before dotfiles #14 still run deleted profiles.** `guest-e-lead` and `live-audit-lead` run `opus-orchestrator`; `p4`, `p6` and `p3` run `general`, `opus` and `general`.
- **Worker todos get stuck.** Leads spawn with a name and a brief instead of the todo unit. The unit never links to the worker, so it can never close (`ACCEPTANCE_REQUIRED`).

## First principles

1. **Thinking and typing are different jobs. Pay for each at its own price.** Planning, decomposition, diagnosis, review and integration need the best model at high effort. Typing out a specified change does not. Calibration (#122): Sol and Sonnet at medium effort completed specified units as well as the merged originals. Luna failed every unit that needed judgment.
2. **The brief is the plan.** A worker sees only its brief. Anthropic's multi-agent write-up found that short, vague briefs make workers duplicate work, leave gaps, and misread the task. A good brief has an objective, an output format, the tools and sources to use, and clear boundaries ([Anthropic, 2025](https://www.anthropic.com/engineering/multi-agent-research-system)). If the brief is complete, a cheap writer needs no planning phase. If it is not, the fix is a better brief or a diagnosis first, not an expensive writer.
3. **Effort must scale to the task, and the scale must be written down.** Agents judge effort badly unless the prompt gives explicit rules for it (same source). So each role gets a fixed model tier and effort level, and leads get explicit spawn rules.
4. **Parallel contexts buy capacity. A lone child you wait on buys nothing.** Most coding work parallelizes less than research. Spawn for independent units, isolation, or a cheaper model, not as ritual.
5. **Prompts drift; tools do not.** A rule that matters every time ("leads never edit code") should be enforced by a tool, with the prompt explaining why.
6. **One source per fact.** The workflow lives in one skill, which every harness loads. Rules files keep only the invariants that apply even outside the workflow.

## Roles

| Role | Does | Never does |
| --- | --- | --- |
| Lead | Splits work into units, writes briefs, creates todo units, spawns workers, answers their questions from source, reviews results, runs heavy checks once, merges | Edits code or config. Exception: plans, briefs and docs it owns |
| Writer | Runs one unit from its brief: code, codemods, light checks, commit, draft PR | Plans scope, spawns agents, runs heavy checks, merges |
| Mechanic | Applies one mechanical change across many sites | Makes judgment calls; it reports `blocked: needs judgment` |
| Architect | Read-only design, security, diagnosis, and ADR or library proposals | Edits anything |
| Reviewer | Fresh-context review of a diff, plan or design | Edits anything |
| Scout | Read-only facts: paths, lines, versions | Gives opinions |

A user-facing top-level session is a lead.

## Profiles, models and effort

| Profile | Model tier | Effort | Prewalk | Change from today |
| --- | --- | --- | --- | --- |
| `build-lead` | `plan-claude` (Opus → Astra) | high | none | prompt only |
| `design-lead` | `plan-claude` | high | none | prompt only |
| `pm-lead` | `plan-gpt` (Astra → Opus) | high | none | prompt only |
| `architecture-lead` | `plan-gpt` | xhigh | none | effort high → xhigh |
| `selection-lead` | `plan-gpt` | high | none | was `review` at medium |
| `architect` | `plan-gpt` | xhigh | none | effort high → xhigh |
| `writer-claude` | `exec-claude` (Sonnet → Sol) | medium | none | was `plan-claude` high with prewalk |
| `writer-gpt` | `exec-gpt` (Sol → Sonnet) | medium | none | was `plan-gpt` high with prewalk |
| `mechanic` | `exec-gpt` | low | into `tier/mech` (Luna → MiniMax → Sol) | was `tier/mech` alone |
| `reviewer` | `review` (Sol → Sonnet) | high | none | effort medium → high |
| `scout` | `scout` (Luna → MiniMax → Sol) | low | none | none |

- **Session without a profile:** `plan-claude` at high. It is a lead.
- **Leads are at high or above.** `xhigh` is for read-only design work, where depth is the whole job. Pi clamps the effort to what each model supports.
- **Writers at medium.** This matches the calibration runs. Low effort is not measured on judgment units, so do not use it for writers.
- **Two writer profiles.** They differ only in model family. Use the other family from the reviewer's family, and switch when one pool's quota is high.

## Where prewalk belongs

Prewalk lets a stronger model explore, write the todo list, and make the first edit. The session then switches once to a cheaper model, which keeps the whole trajectory in context. It pays off only when two things are true: the session must discover its own plan, and the work after the first edit is repetitive.

- **Writers: no prewalk.** The lead's brief is the plan, so a planning phase pays twice. A unit whose cause is unknown goes to an `architect` first. Its findings become the writer's brief.
- **Mechanic: prewalk from Sol into `tier/mech`.** Sol reads the brief, finds the sites, writes the checklist, and makes the exemplar edit itself. Luna, then MiniMax, types the rest. This replaces the old rule that the lead writes the exemplar. MiniMax stays an executor only: it continues a plan, and it never plans or reviews.
- **Leads: no prewalk.** Prewalk switches on the first edit, and leads do not edit code.
- **Global `prewalk.json`:** stays disabled.
- **Measure next:** writers prewalking from `exec` into `mech` on large specified units. That needs a calibration arm first. Calibration showed that Luna alone fails judgment units.

## Todos and units

There are two lists with different owners:

- **The lead's todo list is the plan of record.** Each unit is a contract: deliverable, owned paths, acceptance, prerequisites, and assignment.
  - A worker unit starts only through `spawn {unit: {id}}`. Dogsled then binds the worker to the unit, and the worker's evidence closes it. A bare `spawn` with a name never links, so its unit stays open forever.
  - The lead's own self-leaf units cover integration and review, never code.
- **A worker's todo list is its own working checklist.** The mechanic's checklist also opens the prewalk gate.

## Delegation rules for leads

- **Split before anything else.** List the deliverables. Each has one acceptance command and the paths it owns.
- **Pick the cheapest profile that fits:**
  - mechanical across many sites: mechanic
  - specified change: writer
  - unknown cause: architect, then a writer
  - facts: scout
  - judgment: reviewer
- **Give every brief all four parts:** objective, output format, tools and sources, and boundaries (owned paths, checks allowed, publication authority). Use the brief-writing skill.
- **Run independent units in parallel.** Merge dependent units in order.
- **Never wait or poll.** Replies arrive in the inbox.
- **Run heavy checks once, at the final revision.** Merge only after the gate and any required approval.
- **Have each nontrivial unit reviewed.** Use a fresh reviewer whose model family differs from the writer's.

## Where text goes

| Today | After |
| --- | --- |
| `root.tera`: "IMPORTANT: … Do a single unit yourself …" | Replaced by: "Before work that changes code, config or Git state, load the `dogsled` skill and follow it." The no-forks and fix-the-cause lines stay. |
| `20-delegation.md` | Deleted. Its content moves to the skill. |
| `10-evidence-and-continuity.md`: check cadence and task state | Moves to the skill (`references/checks.md`). The evidence lines stay. |
| `30-safety-and-delivery.md`: PRs, CI budget, stacks, merging | Moves to the skill (`references/delivery.md`). Git safety and secrets stay. |
| `00`, `40`, `50`, `60` | Stay. They are invariants, not workflow. |
| Profile prompts: "Read ~/.agents/rules …" | "Load the dogsled skill. You are a <role>." plus the role's charter. |
| dogsled `SKILL.md` (tool mechanics) | Becomes the workflow. The tool mechanics move to `references/commands.md`. |

The dogsled repository owns the skill, and its plugin installs it into Pi, Claude Code and Codex.

## Enforcement

The "leads never edit code" rule should be enforced, not only written. Proposal:

- Add `role: "lead"` to dogsled profiles. Today only `reader` and `writer` exist.
- Have the guardrail extension refuse `edit` and `write` in a lead session for any path outside `plans/`, `docs/`, briefs, and `*.md`.

This is a separate code unit, after the skill lands.

## Units

1. **dogsled: workflow skill (docs).** Rewrite `plugins/dogsled/skills/dogsled/SKILL.md` and add `references/commands.md`, `checks.md` and `delivery.md`. Release it. The skill must exist before step 2 removes the rules.
2. **dotfiles: rules and profiles.** Shrink `root.tera`, delete `20-delegation.md`, and trim `10` and `30`. Apply the profile table, set `defaultThinkingLevel: high`, and give the mechanic its prewalk arguments. Apply on all hosts.
3. **dogsled and oisinpi: lead role guard** (code). Optional, and needs your yes.
4. **Restart the old-profile leads.** These are `guest-e-lead`, `live-audit-lead`, `p3`, `p4` and `p6`. Their owners can restart them once each one's current work lands. I will not close them myself.
