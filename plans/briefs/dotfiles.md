# Unit: dotfiles harness switch to tiers and teams

Deliverable: a draft PR on `feat/tiers-and-teams` in `~/dev/dotfiles` (base origin/main). The PR switches Pi from `alias/*` models to `tier/*` virtual models (`@oisin-ee/pi-tiers`, shipped inside `@oisin-ee/oisinpi`) and replaces the dogsled profiles with the 11 team profiles below.
Prerequisites, all on main or about to be:
- dogsled `prompt` lists (dogsled 346f534)
- `dogsled quota` with `[quota] management_url` (1d6fc22)
- pi-tiers (oisinpi PR #21; the lead gives you the released version)
- skills `brief-writing`, `integration`, `plan-review`, `adr` (dotfiles c479e5c4)

Read first:
- `conf.d/harness/mise.toml`, lines 295–325, the file-install table.
- `conf.d/harness/settings/pi/{settings.json,model-alias.json,prewalk.json}`.
- `conf.d/agent-ide/dogsled/{config.toml,profiles/*}`.
- `conf.d/harness/rules/topics/20-delegation.md` and `10-evidence-and-continuity.md`.
- The pi-tiers README in oisinpi `packages/pi-tiers/README.md`, which defines the tiers.json format and routing.
- dogsled README "Profiles" (prompt lists) and "Quota".

## 1. Tiers: new `conf.d/harness/settings/pi/tiers.json`

Install it in `conf.d/harness/mise.toml` as `"~/.pi/agent/tiers.json" = { source = "settings/pi/tiers.json", mode = "copy" }`, next to models.json.
Use these model ids exactly. All are `cliproxyapi/*`, verified with `pi --list-models`.

```json
{
  "plan-claude": ["cliproxyapi/claude-opus-5-5", "cliproxyapi/gpt-6-astra"],
  "plan-gpt":    ["cliproxyapi/gpt-6-astra", "cliproxyapi/claude-opus-5-5"],
  "exec-claude": ["cliproxyapi/claude-sonnet-5-5", "cliproxyapi/gpt-6.1-sol"],
  "exec-gpt":    ["cliproxyapi/gpt-6.1-sol", "cliproxyapi/claude-sonnet-5-5"],
  "mech":        ["cliproxyapi/minimax-m3.1-flash", "cliproxyapi/gpt-6-luna", "cliproxyapi/gpt-6.1-sol"],
  "scout":       ["cliproxyapi/gpt-6-luna", "cliproxyapi/minimax-m3.1-flash", "cliproxyapi/gpt-6.1-sol"],
  "review":      ["cliproxyapi/gpt-6.1-sol", "cliproxyapi/claude-sonnet-5-5"]
}
```

`cliproxyapi/minimax-m3.1-flash` appears only after the operator runs the infra MiniMax migration. Keep it in the chains.

## 2. Settings: `conf.d/harness/settings/pi/settings.json`

- Remove `npm:pi-model-fallback-alias@0.2.0` and `npm:@monotykamary/pi-retry@0.10.4` from `packages`.
- Bump `npm:@oisin-ee/oisinpi` to the version the lead gives you; it bundles pi-tiers.
- Set `defaultProvider` to `"tier"`, `defaultModel` to `"plan-claude"`, and `defaultThinkingLevel` to `"medium"`. Prewalk uses `defaultThinkingLevel ?? "medium"` for its handoff, so `medium` is deliberate.
- Delete `settings/pi/model-alias.json`. Change its `mise.toml` entry to `{ mode = "absent" }`, as the file already does for `shepherdr/profiles`, so hosts lose the copied file.
- `settings/pi/prewalk.json` currently targets `alias/luna`. Change its target model to `tier/exec-gpt`, and keep `enabled: false` and `thinking: medium`. Profiles still set their own `--prewalk-into`.
- Leave the unrelated uncommitted `pi-question` removal in the lead's checkout alone. You work in your own worktree.

## 3. Quota config: `conf.d/agent-ide/dogsled/config.toml`

Add:
```toml
[quota]
management_url = "https://cliproxy-admin.tail10e7b9.ts.net"
```
This is not a secret. The key lives only in the laptop Keychain (service `dogsled`, account `cliproxy-management`). Other hosts never collect.

## 4. Profiles: `conf.d/agent-ide/dogsled/profiles`

Delete these 7: `astra`, `opus`, `general`, `prewalk`, `explorer`, `opus-architect`, `opus-orchestrator`.
Keep and rewrite `architect` and `reviewer`.
Add `_shared/writer.md` and `_shared/lead.md`. They are not profiles, so give them no `profile.json`.
Each profile's `prompt` is a list: the shared file, then its own `prompt.md`.

| Profile | role | model | thinking | prompt | pi_args |
| --- | --- | --- | --- | --- | --- |
| writer-claude | writer | tier/plan-claude | high | [../_shared/writer.md, prompt.md] | ["--prewalk-into", "tier/exec-claude"] |
| writer-gpt | writer | tier/plan-gpt | high | [../_shared/writer.md, prompt.md] | ["--prewalk-into", "tier/exec-gpt"] |
| mechanic | writer | tier/mech | low | [../_shared/writer.md, prompt.md] | [] |
| scout | reader | tier/scout | low | [prompt.md] | [] |
| reviewer | reader | tier/review | medium | [prompt.md] | [] |
| architect | reader | tier/plan-gpt | high | [prompt.md] | [] |
| build-lead | writer | tier/plan-claude | high | [../_shared/lead.md, prompt.md] | [] |
| pm-lead | writer | tier/plan-gpt | high | [../_shared/lead.md, prompt.md] | [] |
| design-lead | writer | tier/plan-claude | high | [../_shared/lead.md, prompt.md] | [] |
| architecture-lead | reader | tier/plan-gpt | high | [../_shared/lead.md, prompt.md] | [] |
| selection-lead | reader | tier/review | medium | [../_shared/lead.md, prompt.md] | [] |

### Prompt content

Keep every prompt short and imperative. Never restate a rule from `~/.agents/rules`; point to it instead.

- **`_shared/writer.md`.** You may research, write code, run codemods, use git, and run light checks:
  - formatter or linter on changed files
  - type check of the changed package
  - unit tests covering the changed files
  - commit hooks

  Never run full suites, builds of other packages, e2e, dependency installs, CI watching, or gates. If a hook fails, fix it in scope or report `blocked` with its output. Never bypass hooks. Commit, push, open a draft PR, and never merge. Reply with branch, head SHA, PR URL, changed files, checks and results, and blockers. Don't spawn agents.
- **`writer-claude` / `writer-gpt` `prompt.md`.** Plan, make the first edit, then let prewalk hand off. One line on what the handoff means.
- **`mechanic/prompt.md`.** Only bounded mechanical units that come with an exemplar edit and an acceptance command. Follow the exemplar exactly. Do not plan, redesign, or review. If the unit needs judgment, stop and report `blocked: needs judgment`.
- **`scout/prompt.md`.** Read-only discovery. Return file paths, line numbers, and facts. No opinions. No edits.
- **`reviewer/prompt.md`.** Keep the current content, and add modes: the task says `mode: code|plan|design`.
  - code: today's text.
  - plan: use the `plan-review` skill.
  - design: use the `design` skill's review mode.
- **`architect/prompt.md`.** Read-only architecture, security, and hard diagnosis. No spawning.
- **`_shared/lead.md`.** You own a scope. Split it into units and write each brief with the `brief-writing` skill. Delegate to `writer-claude`, `writer-gpt`, `mechanic`, `scout`, `reviewer`, or `architect`, following `~/.agents/rules/20-delegation.md`. Integrate with the `integration` skill. Only leads run heavy checks, once at the final revision. Answer child questions from source; escalate only product or scope decisions.
- **Lead charters** (each lead's own `prompt.md`):
  - **build-lead.** Delivery of code changes. The only heavy-check owner. Merges after the gate.
  - **pm-lead.** Writes plan files and tickets only, using the `plan-review` skill for preflight. Never writes code.
  - **design-lead.** UI work through the `design` skill. Sends browser units with `--host local`.
  - **architecture-lead.** Read-only. Writes ADR proposals with the `adr` skill in each repo's convention. Only the user accepts them.
  - **selection-lead.** Read-only library and tool selection with the `research` skill and `research/references/compare-libraries.md`.

## 5. Rules

- **`conf.d/harness/rules/topics/20-delegation.md`.** Replace the profile/model table with the 11 profiles: name, role, tier, and when to use each. State the principle once: expensive models think, cheap models type, only leads run heavy checks. Keep the rest of the rule unchanged.
- **`10-evidence-and-continuity.md`.** Change only what references old profile names or `alias/*`.
- **Other mentions.** Run `rg -n 'alias/|opus-orchestrator|opus-architect|\bexplorer\b|profile.*general|prewalk.*luna' conf.d`. Update every mention in harness/agent-ide docs, rules, and skills that names a removed profile or alias. List each one in the PR body.

## Owned paths

- `conf.d/harness/settings/pi/**`
- `conf.d/harness/mise.toml` (file-install lines only)
- `conf.d/agent-ide/dogsled/**`
- `conf.d/harness/rules/topics/20-delegation.md`, `conf.d/harness/rules/topics/10-evidence-and-continuity.md`
- Files the `rg` above finds under `conf.d/harness/{docs,skills,rules}` and `conf.d/agent-ide`, for those mentions only.

## Acceptance

1. From the worktree, run `DOGSLED_CONFIG=$PWD/conf.d/agent-ide/dogsled/config.toml dogsled profiles`. Profiles resolve beside the config file. The output must list exactly the 11 profiles, none `broken:`, all on `tier/*` models.
2. `jq . conf.d/harness/settings/pi/{tiers,settings}.json` parses.
3. `rg -n 'alias/' conf.d/harness conf.d/agent-ide` prints nothing.
4. The lefthook pre-commit passes.

Do not run `mise run apply` or install anything on the host. The lead applies after release.

## Rules for this unit

- Work only in your worktree and stay inside the owned paths.
- Light checks only. Commit hooks run on commit and must pass; never bypass them.
- Commit `feat(harness): tier models and team profiles`, push, and open a DRAFT PR. Do not merge.
- No secrets. Do not fork, patch, or vendor third-party code.
- Final reply: branch, head SHA, PR URL, files changed, checks and results, and anything blocked or unverified. Do not spawn agents.
