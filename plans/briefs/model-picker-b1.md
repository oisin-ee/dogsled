# Unit B1: new leads start with a chosen model and thinking level (server side)

**Outcome.** `POST /api/leads` accepts an optional model and thinking level, the lead's Pi starts with them on whichever host it lands, and `GET /api/models` serves the host's model catalogue.

**Design.** Read `plans/model-picker.md` first; its decisions bind you. Wire types exist in `portal-types/src/models.rs` (`ModelRef`, `ModelChoice`, `ThinkingLevel`, `ModelCatalogue`). Today `CreateLeadBody` (`portal-types/src/agents.rs:235`) has only `repo` and `prompt`, and `src/leads.rs:310` starts Pi as `agent start <name> --kind pi --pane <pane>` with no Pi flags.

**Edits.**
1. Model catalogue, `src/observation/models.rs` (new), modelled on `src/observation/catalogue.rs` and wired beside it in `src/observation/collect.rs:113` and `:247`:
   - Run `pi --mode rpc --no-session` through `Shell::run_input_bounded` (`src/shell.rs:98`) with stdin `{"id":"models","type":"get_available_models"}\n`. Read stdout lines; skip lines that are not the response with `"id":"models"` (Pi also emits `extension_ui_request` lines). A missing response, `"success":false`, or a timeout is an error.
   - Keep `type == "chat"` models only. Map each to `ModelChoice`: `provider`, `id`, `name`, and `thinking` from the same rule as `getSupportedThinkingLevels` in `@earendil-works/pi-ai` 1.3.0 (`node_modules/@earendil-works/pi-ai/dist/models.js`). Port that rule exactly and cite it in a comment; non-reasoning models give `[off]`.
   - Refresh at startup and at most hourly; keep the last good list with the error, like `Catalogue::refresh`. Publish it as the view `models`: capture and insert it in `src/observation/project.rs` exactly as `repos` is (`capture_repos` at `:64`, `:151`, `repos_view` at `:348`/`:408`).
2. `GET /api/models` in `src/web/agents.rs`, a copy of the `repos` handler at `:578` that reads the `models` view as `ModelCatalogue`.
3. `CreateLeadBody`: add `#[serde(default, skip_serializing_if = "Option::is_none")] model: Option<ModelRef>` and the same for `thinking: Option<ThinkingLevel>`. Keep `deny_unknown_fields`. Fix every constructor (`web/src/api.rs:179` passes `None` for both; B2 wires them).
4. `leads::create` (`src/leads.rs:182`) takes the model and thinking. Carry them in `routing::Delivery` (`src/leads/routing.rs:9`) with `#[serde(default)]` so a peer on an older binary still parses the delivery. Do **not** add them to `leads::Record` or the `leads` table: `src/store.rs:9` restarts an older store empty, so a schema bump would wipe every host's items and queue. A lead never retries an uncertain start, so nothing reads them back; Pi's session records the model it runs. Pass them from `start` (`src/leads.rs:207`) to `launch` (`src/leads.rs:296`) as a launch argument. In `launch`, append `--`, then `--model <provider/id>` and `--thinking <level>` when set (exemplar: `src/launch.rs:2244`). `src/main.rs:341` and `src/mcp.rs:579` pass `None, None`.
5. Tests: catalogue parsing from a recorded RPC transcript (with the extra `extension_ui_request` lines), refusal and retention on failure, hourly refresh; `POST /api/leads` with and without the new fields, unknown field still refused; the Pi argv for a lead with model and thinking; a peer delivery without the fields still parses. Extend `src/leads/tests.rs` and the existing web route tests.

**Paths.** Owns: `src/observation/models.rs`, `src/observation/collect.rs`, `src/observation/project.rs`, `src/observation/catalogue.rs` (only if you share code), `src/web/agents.rs` (only the models route, `create_lead`, and their tests), `src/leads.rs`, `src/leads/routing.rs`, `src/leads/tests.rs`, `src/main.rs`, `src/mcp.rs`, `portal-types/src/agents.rs` (`CreateLeadBody` only), `web/src/api.rs` (`create_lead` only). Unit A edits other parts of `src/web/agents.rs` and `web/src/api.rs` at the same time; keep your changes there small and local to avoid conflicts. Do not edit `portal-types/src/models.rs`; if a type is wrong, reply `blocked` with the fix. Reads: everything.

**Acceptance.** `cargo test --locked` passes, including the new tests. The scoped diff never parses `pi --list-models` text.

**Checks.** `moon run :fmt-check :lint :web-lint :test` (lint depends on `web-build`).

**Base.** `feat/model-picker-types` at `1118bc2`.
