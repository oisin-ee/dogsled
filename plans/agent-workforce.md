# Agent workforce: tiers, CLIProxy-aware routing, quota panel, teams

Status: draft for review. The work spans four repositories:

| Repository | Owns |
| --- | --- |
| `infra` | CLIProxy |
| `oisinpi` | Pi tier router |
| `dotfiles` | Profiles, settings, rules, skills (`~/dev/dotfiles` = `~/.config/mise`) |
| `dogsled` | Profile loader, quota collector, right-side board |

## Context

**Problem:**
- There are 9 overlapping dogsled profiles, and orchestrators send code to `astra` or `opus`.
- Implementation prompts say "validate", so writers run heavy checks. The `prewalk` prompt bans all checks, so it cannot run light ones either.
- Tier aliases cross families with no plan. For example, `luna` → Sol → Opus.

**Accounts:** 2× Claude Max, 2× ChatGPT, and 1× MiniMax Token Plan Max for `MiniMax-M3.1-Flash`. All traffic goes through CLIProxy (`cliproxy.momokaya.ee`). There are no direct-to-provider targets.

**Failure evidence** (netcup alias logs, 385 warnings):

| Class | Count | Error text (HTTP status) |
| --- | --- | --- |
| Gateway or transport | 332 | "no available server" (503), "Bad Gateway" (502), "Unable to connect" |
| Quota | 27 | "You have hit your ChatGPT usage limit." (429) |
| Transient | 17 | WebSocket 1006, timeout |
| Configuration | 9 | Unknown model |

The laptop log follows the same pattern (1,301 failovers since 2026-10-14). `pi-model-fallback-alias` handles every failure the same way: it cools the target and moves on. But a gateway outage is not a fault in the target.

**User decisions (2026-10-21):**
- Build a first-party router on `pi.registerVirtualModel`. Copy no third-party code.
- Read quota the way QuotaMenu does (CLIProxy URL and management key from local config). Add no poller in the cluster.
- Show agents, jobs, todos, and quotas in a right-side panel, not in the session's vertical space.
- Choose executor models from evidence of what is generally "good enough".
- First teams: Build, Design, Architecture, Library/tool selection, and Project management.

## Findings

### Executor quality (`rs-executor-models`)

Independent data from Vals Terminal-Bench 4.0, same harness ([source](https://www.vals.ai/benchmarks/terminal-bench-4)):

| Model | Pass rate | Cost per task |
| --- | ---: | ---: |
| Sonnet 5.5 | 64.1% | $16.51 |
| GPT-6.1 Sol | 55.1% | $1.72 |
| MiniMax M3.1 Flash | 25.3% | $0.73 |
| GPT-6 Luna | 13.6% | $0.35 |

- **Code Migration v1.1:** Sonnet 86.9%, Sol 77.3%, Luna 46.7%. There is no result for MiniMax.
- **Vibe Code Bench:** Luna 81.7% scores above MiniMax 73.9%. So the order of the two cheap models changes with the task.
- **Conclusion:** Sol is the cheapest model that is generally good enough to write code without supervision. Sonnet is the escalation. MiniMax and Luna are suitable only for tightly bounded, mechanical units that have an exemplar edit and an acceptance command. These results come from benchmarks; nobody has tested post-plan execution in Pi.

### Router (`rs-router-cliproxy`, verified locally)

**Pi does not retry CLIProxy's errors.** `isRetryableAssistantError` returns `false` for all four CLIProxy error texts. It returns `true` for "503 Service Unavailable" and "overloaded" (checked on the laptop). Consequences:
- `route(reason: "retry")` never fires for these errors.
- `registerVirtualModel` alone would lose the failover we have today.

**The supported recovery path:**
- `agent_before_settle` (see `docs/extensions.md:65–67,118`) runs when the session would otherwise stop.
- It can append a `context_edit` that drops the failed assistant entry, plus a recovery marker, and return `continue: true`.
- `route()` then reads the marker on the continuation request.
- Limits:
  - Use this only for errors that native retry does not handle.
  - Give it a bounded per-target budget.
  - Never rewrite message text or fabricate user messages.

**Correlating errors:**
- `failed.message.errorMessage` is bare text. `after_provider_response` carries the HTTP status but no request identity.
- The router therefore correlates requests itself and classifies by both status and text.

**Attribution:**
- A ChatGPT 429 was logged against `cliproxyapi/claude-opus-5-5`, so a target name does not prove which backend served the request.
- Quota state is keyed by CLIProxy credential pool, not by model name.

**What CLIProxy already does:** `request-retry: 3` means three extra rounds, and each round can try every credential (config v8.0.33). Credential rotation therefore stays in CLIProxy. The router switches only between model pools.

**`pi-retry` conflicts with the router:** `@monotykamary/pi-retry` suppresses Pi's native retry (`src/session-registry.ts:115–129`). Remove it when the router ships.

**Costs of switching models mid-session:** prompt-cache reuse is lost, and thinking blocks from the other provider are dropped. Stay on the chosen target until it fails.

### Quota and panel (`rs-quota-panel`, verified locally)

**Quota data path, as QuotaMenu does it:**
- `GET /v0/management/auth-files` lists the OAuth accounts.
- `POST /v0/management/api-call` with an `auth_index` calls the provider; CLIProxy fills in `$TOKEN$`.
- Claude: `GET https://api.anthropic.com/api/oauth/usage` (header `anthropic-beta: oauth-2025-04-20`).
- Codex: `GET https://chatgpt.com/backend-api/wham/usage`.

**MiniMax needs a different lookup:**
- `claude-api-key` entries can be missing from `auth-files`.
- `GET /claude-api-key` returns each entry's `auth-index`, but it also returns the key values. Only trusted code may make this call, and the values must never reach logs or a model.
- Then call `api-call` → `GET https://www.minimax.io/v1/token_plan/remains`.

**Laptop setup:** QuotaMenu 1.1.3 stores one site in `~/Library/Preferences/QuotaMenu.plist` and its key in the Keychain, service `com.quotamenu.app`, account = site UUID.

**Panel options:**
- Herdr 0.9.5 plugin panes support `--placement split --direction right` (checked against installed `herdr plugin pane open --help`). Today dogsled uses `placement = "popup"` (`herdr-plugin.toml:15`).
- Pi side overlays draw over the transcript. Herdr's split reserves width instead.

**The todo gap:** todos exist only inside each Pi process, on the in-process event bus (`pi/work-todo.ts`). The dogsled CLI board has agents, jobs, and the queue, but no todos.

**Polling:** quotas every 5 minutes, work state every 10 seconds, with refreshes deduplicated and backoff on errors. Show how long ago the last success was. QuotaMenu caches for 300 seconds.

### Teams (`rs-teams-*`)

- Leads delegate and must not write unit code. The current `architect` cannot become a lead, because its prompt forbids spawning.
- All leads share one pool of workers; teams get no private pools.
- The reviewer is diff-only (`profiles/reviewer/prompt.md:5`). It cannot review a plan or a rendered design.
- Design references already exist in `harness/skills/design/references/` (review, browser-grounding, design-exploration, design-token-enforcement, visual-diff). `design/SKILL.md` does not route to them.
- ADR conventions already exist: infra uses descriptive names (`infra/docs/adr/`), and rondo uses numbered records with maintainer approval (`rondo/docs/adr/README.md`).
- Tool flags cannot tell a light check from a build inside `bash`.

## Approach

### 1. Gateway: MiniMax in CLIProxy

- Add one `claude-api-key` entry with `base-url: https://api.minimax.io/anthropic` and model `MiniMax-M3.1-Flash`. The key comes from OpenBao `kv/app-runtime/cliproxy/cliproxy/cliproxy-secrets`.
- Change the ESO template and the live `config_store` row together, as the file comment requires.
- Pi finds the model through `/v1/models`. Add a `models.json` override only if the discovered context window or `maxTokens` is wrong.

### 2. Tiers: a first-party router package `@oisin-ee/pi-tiers` in `oisinpi`

**Virtual models** under provider `tier`, defined in one config file (`~/.pi/agent/tiers.json`, sourced from dotfiles):

| Tier | Chain | Used for |
| --- | --- | --- |
| `tier/plan-claude` | Opus 5.5 → Astra | Planning and leads, Claude-first |
| `tier/plan-gpt` | Astra → Opus 5.5 | Planning and leads, GPT-first |
| `tier/exec-claude` | Sonnet 5.5 → Sol | Execution after prewalk, Claude family |
| `tier/exec-gpt` | Sol → Sonnet 5.5 | Execution after prewalk, GPT family |
| `tier/mech` | MiniMax M3.1 Flash → Luna → Sol | Bounded mechanical units only |
| `tier/scout` | Luna → MiniMax → Sol | Read-only discovery |
| `tier/review` | Sol → Sonnet 5.5 | Fresh review |

**Router state** on the branch: `{chosen, attempted, budget}`. The router stays on `chosen` until that target fails.

**Error classes and actions:**

| Class | Signal | Action |
| --- | --- | --- |
| Gateway | 502 or 503 from CLIProxy itself, "no available server", "Unable to connect" | Circuit-break the CLIProxy endpoint for every tier, then back off and retry. Do not cool models, because every target is behind the same gateway. |
| Quota | 429 or usage-limit text | Exclude the pool until the reset time from the quota cache, or for a default cooldown. Move to the next target. |
| Transient | Overloaded, timeout, WebSocket 1006 | Leave these to native retry. If that is exhausted, use the bounded `agent_before_settle` recovery to the next target. |
| Request, auth, config | Unknown model, 400, 401 | Stop and report. Do not fail over. |

**Shared state:** a same-host cooldown store with file locking and transactional writes. The router reads quota snapshots from the dogsled cache (see section 3) to skip exhausted pools before sending a request.

**Prewalk:** no change. Profiles pass `--prewalk-into tier/exec-…`. Prewalk takes the thinking level from `settings.defaultThinkingLevel ?? "medium"` (`pi-prewalk/src/adapter.ts:147`), so set `defaultThinkingLevel: "medium"`. Moving the handoff into the router state is a later step and is out of scope here.

**Rollout:** replace `alias/*` with `tier/*` in profiles and settings, remove `pi-model-fallback-alias` and `@monotykamary/pi-retry` from `settings.json`, and delete `model-alias.json`.

### 3. Quota collector in dogsled

- New dogsled module: `dogsled quota [--json] [--refresh]`.
- **Credential (decided):** the collector runs on the laptop only. The CLIProxy URL and management key live in the macOS Keychain, in the same way QuotaMenu stores them. dogsled reads its own Keychain item (service `dogsled`, account `cliproxy-management`) through `security`. The key never goes into the dotfiles-linked `config.toml`, and Linux hosts never hold it.
- **Sources:** `auth-files` (Claude and Codex accounts), `claude-api-key` (MiniMax `auth-index`, used inside the collector only), and `api-call` per account.
- **Output:** a normalized `{pool, account, window, used_pct, resets_at, fetched_at, error}` written to `~/.local/state/dogsled/quota.json` on the laptop. Other hosts read the cache through dogsled's host transport and never call CLIProxy management. When the laptop is offline, the snapshot is shown as stale, with its age.

### 4. Right-side board

- The Herdr pane `board` changes to `placement = "split"` with a right direction, and runs a compact `dogsled board --side`.
- Sections: quotas, agents and jobs (existing), and todos.
- **Todo bridge (decided: same view, no rewrite):** the dogsled Pi extension already holds pi-todo's `TodoSnapshot` and its provider-owned view (`pi/work-todo.ts`).
  - On each new revision, the extension calls `display.createView(...)` and then `render(width)` at the side-panel width. It publishes the lines that pi-todo itself rendered, with `{session, branch, revision, current}`, to the dogsled service.
  - The board prints those lines unchanged. pi-todo is still the only renderer, and neither the rendering nor the task state is copied.
  - The board shows one `current` line per agent, plus the full rendered view for the owner's session (`--owner <pane>`).
  - No change is needed in `pi-todo`.
- **Herdr owns display when it is available (review feedback):** the dogsled Pi extension detects a Herdr pane (`HERDR_ENV=1` plus a reachable Herdr socket).
  - When it finds one, it opens or reuses the dogsled right split for that tab and renders nothing above the editor.
  - Only without Herdr does it keep today's Pi widget.
  - This is a dogsled decision. Pi and Herdr need no changes.

### 5. Profiles and teams (dotfiles `agent-ide/dogsled/profiles`)

**Shared workers:**

| Profile | Role | Model → prewalk | Thinking | Limits |
| --- | --- | --- | --- | --- |
| `writer-claude` | writer | `tier/plan-claude` → `tier/exec-claude` | high, then medium | Research, code, codemods, git, light checks only. No spawn. |
| `writer-gpt` | writer | `tier/plan-gpt` → `tier/exec-gpt` | high, then medium | Same prompt as `writer-claude` |
| `mechanic` | writer | `tier/mech` (no prewalk) | medium | Brief must contain an exemplar edit and an acceptance command. On failed assumptions it stops and reports `blocked`. |
| `scout` | reader | `tier/scout` | low | Read-only. Returns file:line evidence and separates "not found" from "absent". |
| `reviewer` | reader | `tier/review` | medium | Modes `code`, `plan`, `design` (rendered). Fresh session. |
| `architect` | reader | `tier/plan-gpt` | high | Deep design, security, and diagnosis. No spawn. |

**Leads.** Every lead gets the common `lead.md` (charter, dispatch, briefs, integration, escalation) plus its own team charter:

| Lead | Role | Model | Owns | Workers | Skills |
| --- | --- | --- | --- | --- | --- |
| `build-lead` | writer | `tier/plan-claude` high | Briefs, dispatch, integration edits, merge, the only heavy checks (once, at the final revision) | All workers | `code-standard`, `diagnosing-bugs`, `research`, `git-machete` (stacks), `integration`, `brief-writing` |
| `pm-lead` | writer (plan files only) | `tier/plan-gpt` high | Plans and tickets: acceptance, dependencies, owners, open decisions | scout, architect, reviewer(plan) | `to-tickets`, `research`, `batch-grill-me`, `plan-review` |
| `design-lead` | writer | `tier/plan-claude` high | Direction, briefs, rendered evidence, user choice points | scout, writers, reviewer(design) | `design` (with fixed routing), `ideas`, `impeccable`, `brand`, `prototype` |
| `architecture-lead` | reader | `tier/plan-gpt` high | A proposed ADR in the repository's convention, or a diagnosis note | scout, architect (one fresh `architect` tries to disprove) | `adr`, `audit`, `diagnosing-bugs`, `code-standard` |
| `selection-lead` | reader | `tier/review` medium | A comparison note: eligibility gates, trade-offs, one recommendation, no forks | scout (one per candidate), architect, reviewer | `research` + `compare-libraries` reference |

**Removed profiles:** `astra`, `opus`, `general`, `prewalk`, `explorer`, `opus-architect`, `opus-orchestrator`. This is a hard cutover with no alias profiles.

**Browser work:** design units that need a browser are placed with `--host local`. Visual review uses Sonnet through `tier/exec-claude` for routine checks; ambiguous cases escalate to `tier/plan-claude`.

**Writer prompt** (shared, replaces "validate"):
- **Allowed:** formatter or linter on changed files, type check of the changed package, unit tests that cover changed files, and commit hooks.
- **Forbidden:** full suites, builds, e2e, dependency installs, CI watching, and gates.
- **When a hook fails:** fix the findings in scope, or report `blocked` with the output.

**dogsled change:** `prompt` accepts a string or an array. Each file becomes one `--append-system-prompt`, which Pi allows multiple times. This lets leads compose `../_shared/lead.md` with their charter.

### 6. Skills and rules (dotfiles `harness/`)

**New skills:**
- `brief-writing`: the dispatch contract.
- `integration`: reconcile evidence and run final checks.
- `plan-review`: find contradictions in dependencies and acceptance criteria.
- `adr`: repository-native records and supersession.

**Changed skills:**
- Add the reference `research/references/compare-libraries.md`.
- Fix the routing in `design/SKILL.md` so it reaches the review, exploration, and token references.

**Rules:** rewrite the profile table and model tiers in `rules/topics/20-delegation.md`. Update the writer light-check boundary in `10-evidence-and-continuity.md` if it differs.

### Out of scope (separate plan)

- Enforcing heavy checks in `pi-guardrails`. This needs a role-aware shell policy and per-repository declarations of heavy commands. Until then, the writer prompt is the only enforcement.
- Moving the prewalk handoff into router state.

## Files to modify

| Repository | Path | Change |
| --- | --- | --- |
| infra | `k8s/manifests/cliproxy/external-secret.yaml` | MiniMax `claude-api-key` entry |
| infra | live `config_store` row, OpenBao `cliproxy-secrets` | Same entry; MiniMax key |
| oisinpi | `packages/pi-tiers/**` (new) | Router, error classifier, cooldown store, quota reader, tests |
| oisinpi | `packages/oisinpi` (bundle manifest) | Include `pi-tiers` |
| dotfiles | `conf.d/harness/settings/pi/settings.json` | Remove alias and `pi-retry` packages; set `defaultThinkingLevel` |
| dotfiles | `conf.d/harness/settings/pi/tiers.json` (new), `model-alias.json` (delete), `mise.toml` (install mapping) | Tier config |
| dotfiles | `conf.d/agent-ide/dogsled/profiles/**` | 6 workers, 5 leads, `_shared/{writer,lead}.md`; delete 7 profiles |
| dotfiles | `conf.d/harness/skills/{brief-writing,integration,plan-review,adr}/` (new); `research/references/compare-libraries.md`; `design/SKILL.md` | Skills |
| dotfiles | `conf.d/harness/rules/topics/20-delegation.md` (+ `10-…` if needed) | Rules |
| dogsled | `src/profiles.rs` | `prompt` as a string or an array |
| dogsled | `src/quota.rs` (new), `src/cli.rs`, `src/main.rs`, `src/config.rs` | Quota collector and command |
| dogsled | `src/board.rs`, `herdr-plugin.toml`, `plugin/board.sh` | Side board and split placement |
| dogsled | `pi/work-todo.ts`, `pi/index.ts`, service inbox or ledger | Todo bridge; shrink the Pi widget |
| dogsled | `README.md`, `CHANGELOG.md`, `docs/design.md` | Documentation |

## Reuse

- Pi 1.3.0: `registerVirtualModel`, `agent_before_settle` boundary results, `after_provider_response`, `isRetryableAssistantError`, and `--append-system-prompt` (repeatable).
- `@oisin-ee/pi-prewalk`, unchanged, through `--prewalk-into tier/…`.
- CLIProxy management API (`auth-files`, `claude-api-key`, `api-call`), the same method QuotaMenu uses.
- Herdr plugin panes with `placement = "split"`.
- dogsled: the profile loader, host transport, `status` and board rendering, and the Pi `TodoSnapshot` bus.
- Existing skills: `research`, `to-tickets`, `code-standard`, `diagnosing-bugs`, `audit`, `design` references, `ideas`, `benchmark` (for calibration).
- Repository ADR conventions in infra and rondo.

## Steps

1. [ ] **Gateway:** put the MiniMax key in OpenBao, add the `claude-api-key` entry to ESO and the live row, and check that `/v1/models` lists the model.
2. [ ] **Calibration:** run the same 6–8 real post-plan units on Sol, Sonnet, MiniMax, and Luna with the `benchmark` skill. Confirm or change the order of `tier/exec-*` and `tier/mech`.
3. [ ] **`pi-tiers`:** classifier with fixtures from the real logs, router state, `agent_before_settle` recovery, locked cooldown store, and tests.
4. [ ] **dogsled:** `prompt` as an array, with tests.
5. [ ] **dogsled:** `quota` collector on the laptop, Keychain credential, cache, and cross-host read.
6. [ ] **dogsled:** side board, split pane, todo bridge. The Pi widget is suppressed when Herdr is available.
7. [ ] **dotfiles:** tiers config, settings (remove alias and `pi-retry`), and `defaultThinkingLevel`.
8. [ ] **dotfiles:** new profiles, shared prompts, and removal of the old profiles.
9. [ ] **dotfiles:** skills and rules.
10. [ ] **Release and install:** oisinpi, then dogsled, then the dotfiles apply on every host.

Steps 1, 3, 4, and 5 are independent and run in parallel as delegated units. Step 2 needs step 1. Steps 7–8 need steps 3 and 4. Step 6 needs step 5.

## Verification

| Area | Evidence |
| --- | --- |
| Gateway | `/v1/models` lists `MiniMax-M3.1-Flash`. A Pi one-shot on `cliproxyapi/MiniMax-M3.1-Flash` succeeds. `api-call` to `token_plan/remains` returns 200. |
| Classifier | Unit tests over every distinct error text and status in the alias logs; each lands in exactly one class. |
| Router | Integration tests with a fake provider: gateway 503 opens the endpoint circuit without cooling models; 429 skips the pool and continues on the next target; overloaded uses native retry first; unknown model stops; abort makes no recovery; recovery is bounded. |
| Prewalk | A writer session hands off from `tier/plan-*` to `tier/exec-*` at medium thinking (the footer shows the physical model). |
| Quota | `dogsled quota --json` returns Claude, Codex, and MiniMax windows. No secret appears in output or logs (grep test). |
| Board | The Herdr right split shows quotas, agents, jobs, and todos, and refreshes on its cadence. The todo lines are byte-identical to pi-todo's own `render(width)` output at the same width (test). A netcup Pi shows the laptop's quota snapshot, with its age. |
| Profiles | `dogsled profiles` lists exactly the 11 new profiles. Each lead spawns a reader child. Writer prompts contain no "validate". |
| Repository checks | Each repository's own gate runs once at its final revision (oisinpi `moon`, dogsled `mise run check`, dotfiles hooks). |

## Decisions after research (2026-10-21)

- Quota collector: the laptop only. The key is in the macOS Keychain, and other hosts read the cache through dogsled.
- Side board: todos are in the first version. They reuse pi-todo's own rendered view; nothing is copied or rewritten.
- Executor: Sol on the GPT side and Sonnet on the Claude side. MiniMax is used only through `mechanic`, and a calibration on 6–8 real units decides any promotion.
- Profiles: hard cutover. The 7 old profiles are deleted in the same change.
