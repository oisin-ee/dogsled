UNIT infra-minimax: MiniMax Token Plan provider in CLIProxy (infra repo).
Base: origin/main 3b9149e3. Branch: feat/cliproxy-minimax.
Deliverable:
1. k8s/manifests/cliproxy/external-secret.yaml: add a legacy-layout `claude-api-key` entry to the rendered config.example.yaml: api-key "{{ .minimaxKey }}", base-url "https://api.minimax.io/anthropic", models: [{name: "MiniMax-M3.1-Flash", alias: "minimax-m3.1-flash"}]. Add the ESO data entry minimaxKey <- kv app-runtime/cliproxy/cliproxy/cliproxy-secrets property minimax-api-key. Update the operator-seed comment to include minimax-api-key. Keep the file's existing comments accurate (the live config_store row must mirror the template).
2. scripts/add-minimax-provider.sh: an operator-only, value-free, idempotent script modeled on scripts/retire-netcup-model.sh (read it first): (a) prompt for the MiniMax key with `read -rs` and write it with `bao kv patch kv/app-runtime/cliproxy/cliproxy/cliproxy-secrets minimax-api-key=-` from stdin, unless the property already exists; (b) read the management key from the cliproxy-config secret, port-forward svc/cliproxy, GET /v0/management/claude-api-key, drop any entry whose base-url is https://api.minimax.io/anthropic, append the new entry (api-key read from OpenBao into a variable, never echoed), and PUT the list; (c) verify GET /v1/models with the client api-key lists minimax-m3.1-flash, printing only ok/FAIL lines. Header comment: agents must not run it.
3. tests/cliproxy-minimax-provider.test.ts (vitest, same helpers as tests/cliproxy-management-access.test.ts): the rendered ExternalSecret maps minimaxKey to property minimax-api-key and the template contains the claude-api-key entry with that base-url, model, and alias; and `bash -n scripts/add-minimax-provider.sh` succeeds.
Owned paths: k8s/manifests/cliproxy/external-secret.yaml, scripts/add-minimax-provider.sh, tests/cliproxy-minimax-provider.test.ts.
Acceptance: `pnpm exec vitest run tests/cliproxy-minimax-provider.test.ts tests/cliproxy-management-access.test.ts` passes.
PR body must state: the operator runs scripts/add-minimax-provider.sh BEFORE this merges, because ESO cannot render the config until OpenBao has minimax-api-key.
Source facts: CLIProxy v8.0.33 accepts legacy layouts and /v0/management (config.example.yaml lines 1238-1270); claude-api-key supports base-url and models name/alias.

Rules for this unit:
- Work only in your worktree. Stay inside the owned paths. Read the repository's AGENTS.md or README and its conventions before you edit.
- Light checks only: the formatter or linter on the files you changed, the type check of the changed package, and the unit tests that cover your change. Do not run full suites, builds of other packages, e2e, dependency installs, or CI watching. The lead runs the repository gate once, after integration.
- Commit hooks run on commit. If a hook fails, fix findings inside your scope; otherwise report blocked with the hook output. Never bypass hooks.
- Commit with feat|fix|chore|docs|test(scope): description, push the branch, and open a DRAFT PR. Do not merge it.
- Never read, print, or commit secrets. Do not fork, patch, or vendor third-party code.
- Final reply: branch, head SHA, PR URL, files changed, the light checks you ran with results, and anything blocked or unverified. Do not spawn agents.
- Plan of record: the agent-workforce plan (sections quoted below are authoritative).