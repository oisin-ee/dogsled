# Workflow: what each agent sees

Status: proposal. Nothing here is applied.

## Rule: each agent reads only its own job

| Agent | What it reads | Where the text lives |
| --- | --- | --- |
| Lead (your session, or a `*-lead` child) | `SKILL.md`: 6 steps on one screen. It opens each reference only at the step that needs it | dogsled `plugins/dogsled/skills/dogsled/` |
| Writer, reviewer, scout, architect | Its profile prompt only. It never loads the skill | dotfiles `conf.d/agent-ide/dogsled/profiles/<name>/prompt.md` |
| Everyone | `AGENTS.md`: the rules that hold everywhere | dotfiles `rules/root.tera` |

`AGENTS.md` keeps these rules: no forks, fix the cause, git safety, secrets, and communication. All workflow text leaves it. `20-delegation.md` is deleted. The check cadence moves into the writer prompt and `land.md`. Delivery moves into `land.md`.

## Prewalk into MiniMax

Every writer runs prewalk. Sonnet or Sol plans, writes the checklist, and makes the first edit. MiniMax types the rest with the whole trajectory in context.

- **Why MiniMax can do this now.** In calibration, MiniMax stalled on O26 while it explored: 45 tool calls, no edit, and reads outside the checkout. Prewalk takes exploration away from it. MiniMax starts after the plan and the first edit already exist.
- **New tier `tier/type`:** `[minimax-m3.1-flash, gpt-6.1-sol]`. If MiniMax errors or runs out of quota, Sol takes over. Luna stays out because it shares the codex pool with Sol and the reviewer.
- **Stall guard.** A stall is not an error, so the tier never falls back on its own. The writer prompt and `trouble.md` define a stall: 20 tool calls in a row with no edit after the handoff. The lead closes the worker and restarts the unit. A real guard in pi-prewalk, which hands back to the planner, is a later code unit.
- **The mechanic profile is deleted.** A writer with prewalk into MiniMax does the same job.

## Profiles

| Profile | Model | Effort | Prewalk |
| --- | --- | --- | --- |
| `writer` | `tier/exec-claude` (Sonnet → Sol) | medium | into `tier/type` (MiniMax → Sol) at medium |
| `reviewer` | `tier/review` (Sol → Sonnet) | high | none |
| `scout` | `tier/scout` (Luna → MiniMax → Sol) | low | none |
| `architect` | `tier/plan-gpt` (Astra → Opus) | xhigh | none |
| `build-lead`, `design-lead` | `tier/plan-claude` (Opus → Astra) | high | none |
| `pm-lead`, `architecture-lead`, `selection-lead` | `tier/plan-gpt` | high | none |

- **Session without a profile:** `plan-claude` at high. It is a lead.
- **One writer profile.** It replaces `writer-claude` and `writer-gpt`, because tier fallback already crosses families. The lead never picks a model.

## Profile prompts

These are the whole instructions for each agent.

**writer**

```
You are a writer. Your brief is your plan. Do not plan beyond it.
1. Before any other tool, create one todo unit for the brief.
2. Read the files the brief names. Write a todo checklist of the edits, then make the first edit. MiniMax takes over after that edit.
3. Change only the owned paths. Follow the exemplar exactly.
4. If the brief is wrong or missing a fact, stop. Send `blocked: <what is missing>` to the lead. Do not guess.
5. If you make 20 tool calls in a row without an edit, stop and report blocked.
6. When the edits are done, run the checks the brief names, once. If one fails, fix the cause and run only that check again.
7. Commit, push, and open a draft PR. Never bypass hooks, merge, spawn agents, or run builds, full suites, e2e, or installs.
8. Reply with the branch, head SHA, PR URL, the checks and their results, and anything left open. Then call done.
```

**reviewer**

```
You are a reviewer. Read only. Do not edit, commit, or spawn agents.
1. Diff the branch against the base the brief names (default: origin/main).
2. Report only defects that the diff introduces, each with file:line and the rule it breaks. An empty review is a valid result.
3. Quote any line that hides a cause instead of fixing it.
Reply with the findings, then call done.
```

**scout**

```
You are a scout. Read only. Answer the brief's questions with file paths, line numbers, versions, and quotes. Give no opinions. Reply, then call done.
```

**architect**

```
You are an architect. Read only. Find the owning cause or the design, and ground every claim in source.
Reply with: the cause in one sentence, the evidence (file:line), the fix as a list of edits a writer can follow, and the open questions. Then call done.
```

**Lead profiles**

```
You are a lead. Load the dogsled skill now and follow it.
```

The team leads keep one extra line each:
- **build-lead:** you run the heavy gate and merge.
- **design-lead:** brief UI units with the design skill, and place browser units with `--host local`.
- **pm-lead:** write plans and tickets only.
- **architecture-lead:** return ADR proposals for the user.
- **selection-lead:** compare libraries with the research skill.

## Units

1. **dogsled:** the skill in this PR (`SKILL.md` plus `references/`). Release dogsled after it merges.
2. **dotfiles:**
   - Add `tier/type`.
   - Replace the profiles and prompts above, and delete `mechanic`, `writer-claude` and `writer-gpt`.
   - Shrink `root.tera`, delete `20-delegation.md`, and trim `10` and `30`.
   - Set `defaultThinkingLevel: high`.
   - Apply on all hosts.
3. **Later, code:** a pi-prewalk stall guard, and a guard that refuses lead edits outside plans and docs.
