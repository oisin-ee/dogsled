UNIT oisinpi-tiers: new first-party Pi extension package @oisin-ee/pi-tiers (oisinpi repo).
Base: origin/main 9db12e5. Branch: feat/pi-tiers.
Goal: replace pi-model-fallback-alias with tier virtual models that classify CLIProxy errors. Copy no code from pi-model-fallback-alias (MIT, third party); you may read ~/.pi/agent/npm/node_modules/pi-model-fallback-alias to learn behaviour only.
Read first: Pi 1.3.0 docs at ~/.local/share/mise/installs/pi/1.3.0/pi/docs/{virtual-models.md,extensions.md (agent_before_settle, boundary results, after_provider_response),settings.md} and examples/extensions/jev-router.ts; the exported types in @earendil-works/pi-coding-agent; @earendil-works/pi-ai dist/utils/retry.js (isRetryableAssistantError). Follow packages/pi-prewalk for package layout, moon.yml, tsconfig, vitest, and README style. Register the project in .moon/workspace.yml, add it to packages/oisinpi (dependencies + pi.extensions), add it to the changeset "fixed" group, and add a changeset (minor).
Behaviour:
- Config: ~/.pi/agent/tiers.json, shape {"<tier>": ["provider/model", ...]} (same shape as model-alias.json). Register each tier as virtual model provider "tier", id "<tier>" via pi.registerVirtualModel. Thinking level passes through.
- Pool of a target: model id starting "claude-" -> claude; "gpt-" -> codex; "minimax-" -> minimax; else the provider id.
- Router branch state {chosen, attempted[], budget}. Stay on chosen until it fails (cache and thinking signatures are lost on switch).
- Error classes (classify on HTTP status from after_provider_response when correlated, and on errorMessage text):
  gateway: 502/503 from the gateway, "no available server", "Bad Gateway", "Unable to connect" -> open an endpoint-wide circuit for the provider (all tiers), exponential backoff, do not mark models cooled; when the circuit is open, retry the same target after backoff instead of switching.
  quota: 429 or usage-limit text ("You have hit your ChatGPT usage limit", "usage limit", "rate_limit" 429) -> exclude that pool until resets_ms from the quota snapshot, else a default cooldown (5 min doubling to 1 h); move to the next target.
  transient: overloaded, timeout, WebSocket 1006, SSE headers timeout -> leave to Pi's native retry; when native retry is exhausted, recover to the next target.
  request/auth/config: unknown model, 400, 401, 403 -> stop, no failover.
- Recovery for non-native-retryable classes uses agent_before_settle: context_edit removing the failed assistant entry, a custom recovery marker entry, continue: true; route() consumes the marker. Bound recovery by budget (default 2 per user turn). Never fabricate user messages or rewrite text. Abort means no recovery.
- Before routing, read the quota snapshot (contract below) via `dogsled quota --json --cached` with a 2 s timeout, cached in memory for 60 s; skip exhausted pools. Missing dogsled or snapshot = no quota filtering.
- Cooldown/circuit store: ~/.pi/agent/state/pi-tiers/state.json shared by processes on one host, with a lock file and write-to-temp + rename.

Shared quota contract (dogsled owns it; pi-tiers and the board read it):
- `dogsled quota --json --cached` prints the last snapshot and never contacts CLIProxy. `dogsled quota --json` (laptop only) refreshes if older than 5 minutes; `--refresh` forces it.
- Snapshot file: `$DOGSLED_STATE/quota.json` (default `~/.local/state/dogsled/quota.json`). Shape:
  {"version":1,"fetched_ms":<u64>,"source":"<host label>","pools":[{"pool":"claude"|"codex"|"minimax","account":"<email or label>","window":"<provider window name, e.g. five_hour, seven_day, seven_day_opus, seven_day_sonnet, primary, secondary, interval, weekly>","used_pct":<f64 0..100>,"resets_ms":<u64|null>,"error":<string|null>}]}
- A pool is exhausted when every account in that pool has at least one window with used_pct >= 100 and resets_ms in the future.
Fixtures: build classifier fixtures from the real error strings listed above plus "Codex error: Our servers are currently overloaded", "WebSocket closed 1006", "SSE headers timeout", "unknown model". Every fixture lands in exactly one class.
Owned paths: packages/pi-tiers/**, packages/oisinpi/package.json, .moon/workspace.yml, .changeset/**, pnpm-lock.yaml (only via pnpm install --offline if the lockfile must gain the workspace link; otherwise leave it).
Acceptance: `moon run pi-tiers:lint pi-tiers:test` passes, with integration tests over a fake provider proving: gateway 503 opens the circuit without cooling models; 429 skips the pool and continues on the next target; overloaded goes to native retry first; unknown model stops; abort makes no recovery; recovery stops at the budget; an exhausted pool from the quota snapshot is skipped before dispatch.

Rules for this unit:
- Work only in your worktree. Stay inside the owned paths. Read the repository's AGENTS.md or README and its conventions before you edit.
- Light checks only: the formatter or linter on the files you changed, the type check of the changed package, and the unit tests that cover your change. Do not run full suites, builds of other packages, e2e, dependency installs, or CI watching. The lead runs the repository gate once, after integration.
- Commit hooks run on commit. If a hook fails, fix findings inside your scope; otherwise report blocked with the hook output. Never bypass hooks.
- Commit with feat|fix|chore|docs|test(scope): description, push the branch, and open a DRAFT PR. Do not merge it.
- Never read, print, or commit secrets. Do not fork, patch, or vendor third-party code.
- Final reply: branch, head SHA, PR URL, files changed, the light checks you ran with results, and anything blocked or unverified. Do not spawn agents.
- Plan of record: the agent-workforce plan (sections quoted below are authoritative).