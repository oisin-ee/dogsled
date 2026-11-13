# Unit A: live Pi model and thinking settings in the portal

**Outcome.** A portal user changes the model or thinking level of a running Pi conversation on any host from its header, and the Pi session really switches.

**Design.** Read `plans/model-picker.md` first; its decisions bind you. Wire types exist in `portal-types/src/models.rs` (`AgentSettings`, `SettingsChange`, `ModelChoice`, `ThinkingLevel`). Today nothing carries settings: `CreateLeadBody` and `src/web/agents.rs:563` routes have no settings, and `pi/` never calls `pi.setModel`.
Serve owns the state, because peers reach a host through serve IPC (`ipc::Client::activity_command` at `src/observation/ipc.rs:892`, `activity_subscribe` at `:941`), not through the web process. Keep it separate from the live-activity producer protocol (`LiveState` denies unknown fields).

**Edits.**
1. `pi/settings.ts` (new), registered from `pi/index.ts` beside `registerActivity(pi, portal, owner)` (`pi/index.ts:63`). With a pane (`HERDR_PANE_ID`):
   - On `session_start`, `model_select`, `thinking_level_select`: PUT the session's `AgentSettings` to `/api/agents/{pane}/settings` with session evidence (`session_file`, `session_id`, as `pi/activity.ts:509` reads them). Choices: `ctx.scopedModels` when non-empty, else `ctx.modelRegistry.getAvailable()`. Levels: `getSupportedThinkingLevels(model)` from `@earendil-works/pi-ai`. Bound the list (at most 1000 choices, ids at most 256 bytes).
   - Hold an SSE subscription to `/api/agents/{pane}/settings/requests` (exemplar: `pi/plan-mode.ts:236` with `PortalClient.events`). Retry with backoff; never fail a hook.
   - For each request: `ctx.modelRegistry.find(provider, id)` then `pi.setModel`; then `pi.setThinkingLevel`. POST the outcome to `/api/agents/{pane}/settings/requests/{id}/result`: applied settings, or an error (`unknown model`, `no credentials for <provider>` when `setModel` returns false).
2. Serve state: a settings module under `src/observation/` keyed by pane and session, with IPC commands for publish, current, request (waits up to 10 s for the result), and a Pi request subscription. Follow `src/observation/live_activity.rs` for session validation and bounds. A session change drops its settings and refuses its pending requests.
3. `src/web/` routes, bearer-checked like the activity producer paths:
   - Pi side, local: `PUT /api/agents/{pane}/settings`, `GET /api/agents/{pane}/settings/requests` (SSE), `POST /api/agents/{pane}/settings/requests/{id}/result`.
   - Portal side: `GET /api/agents/{host}/{pane}/settings` (404 `settings_unavailable` when no Pi session publishes) and `POST /api/agents/{host}/{pane}/settings` with `SettingsChange` (400 when empty or invalid; 409 when Pi refuses, with Pi's message; 504 when Pi does not answer in 10 s; 200 with the new `AgentSettings`).
   - Remote hosts: add `PeerOperation::Settings` and `PeerOperation::ChangeSettings(SettingsChange)` in `src/web/agents.rs:72`, routed like `Register`/`Publish`.
4. Portal: `web/src/model_picker.rs` (new), a reusable `ModelPicker` component: searchable model combobox (exemplar `RepositoryPicker` in `web/src/new_lead.rs:42`) and a thinking `Select` from `web/src/components/select` limited to the chosen model's levels. Props: choices, selected model, selected level, busy, `on_change(SettingsChange)`. Unit B2 reuses it, so keep it free of conversation state.
5. `web/src/agent_view.rs:656` header: for a Pi agent, load settings and show `ModelPicker`; a change POSTs and shows the result or the error with `crate::notice::Alert`. Refresh on the agent's detail events. For Claude and Codex agents, show the recorded model read-only with the text "Model changes work for Pi sessions only". `web/src/api.rs`: add `agent_settings` and `change_settings`.
6. Tests: Pi unit tests in `pi/settings.test.ts` (publish on each event, apply, refusal on `false`, stale session ignored); Rust tests for routes, session change, timeout, peer routing; web SSR tests in `web/src/component_tests.rs` for the picker and the read-only note.

**Paths.** Owns: `pi/settings.ts`, `pi/settings.test.ts`, `pi/index.ts`, `src/observation/` (new settings module, `ipc.rs`, `mod` wiring), `src/web/agents.rs`, `src/web/mod.rs`, new `src/web/settings.rs`, `web/src/model_picker.rs`, `web/src/main.rs`, `web/src/agent_view.rs`, `web/src/api.rs`, `web/src/component_tests.rs`, `web/assets` styles for the picker. Do not edit `portal-types/src/models.rs`; if a type is wrong, reply `blocked` with the fix. Reads: everything.

**Acceptance.** `cargo test --locked && pnpm test` pass, including the new tests. In the scoped diff, the Pi side calls only `pi.setModel` and `pi.setThinkingLevel` to change settings, never typed slash commands or herdr keys.

**Checks.** `moon run :fmt-check :lint :web-lint :typecheck :test` (lint depends on `web-build`), and `pnpm test`.

**Base.** `feat/model-picker-types` at `1118bc2`.
