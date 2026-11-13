# Unit B2: choose model and thinking in the New conversation flow

**Outcome.** The New conversation screen shows a model and thinking picker under the repository picker. The first message starts the lead with the chosen model and level, or with Pi's defaults when the user picks nothing.

**Design.** Read `plans/model-picker.md` first; its decisions bind you. Units A and B1 are merged on this base:
- `web/src/model_picker.rs`: `ModelPicker { choices, selected_model, selected_level, busy, on_change: EventHandler<SettingsChange> }`, a controlled component with no conversation state. The header in `web/src/agent_view.rs` uses it.
- `GET /api/models` returns `portal_types::models::ModelCatalogue { models, error }`: the last good list, plus the last refresh error. It answers 503 when observation is unavailable.
- `api::create_lead` (`web/src/api.rs:203`) builds `CreateLeadBody` with `model: None, thinking: None`. The server already starts Pi with `--model`/`--thinking` when they are set.

**Edits.**
1. `web/src/api.rs`: add `models()` for `GET /api/models`, modelled on the existing JSON getters. Give `create_lead` `model: Option<ModelRef>` and `thinking: Option<ThinkingLevel>` and pass them into the body.
2. `web/src/new_lead.rs`:
   - Load the catalogue with `use_resource` when `NewLead` mounts.
   - Hold `model: Option<ModelRef>` and `thinking: Option<ThinkingLevel>`. Both start unset, which means Pi's default.
   - Render `ModelPicker` in the header after `RepositoryPicker`, disabled while `busy`.
   - `on_change` applies the change locally; it never calls the settings API. When the model changes and the current thinking level is not among the new model's `thinking`, clear the level.
   - Pass both to `api::create_lead` in `send`.
   - On catalogue failure, or when `error` is set, show an `Alert`: "Model list unavailable: … The lead starts with Pi's default model." Sending stays enabled. A model is optional and never blocks sending.
3. `web/src/model_picker.rs`: only if `ModelPicker` cannot show an unset model or level ("Pi default"). Then make the smallest prop change that can, for example `selected_level: Option<ThinkingLevel>`. Keep the header's rendering and behaviour identical, and update its call site in `agent_view.rs`. Never add launch or conversation state to the component.
4. Tests, in `web/src/new_lead.rs` tests or `web/src/component_tests.rs`:
   - The picker renders in New conversation with the catalogue choices.
   - Picking a model, then a level, sends both to `create_lead`. Assert on the built request body, or on a pure helper you extract for it.
   - With no choice, the request has neither field.
   - A model change drops an unsupported level.
   - A catalogue error shows the alert and leaves sending enabled.

**Paths.** Owns: `web/src/new_lead.rs`, `web/src/api.rs` (`models` and `create_lead` only), `web/src/model_picker.rs` (edit 3 only), `web/src/agent_view.rs` (only the `ModelPicker` call site, and only if edit 3 changes its props), `web/src/component_tests.rs`, `web/assets/console.css` (layout of the new header row only). Reads: everything. Do not edit server code, `portal-types`, or `pi/`. If one of them is wrong, reply `blocked` with the fix.

**Acceptance.** `cargo test --locked -p web` passes, including the new tests. WASM web Clippy is clean.

**Checks.** `cargo test --locked -p web`, then `cargo clippy --locked -p web --target wasm32-unknown-unknown -- -D warnings`. The bundle in `web/public` exists already. Do not run `dx bundle` or the full suites; the lead runs them at integration.

**Base.** `feat/model-picker-types` at `4664650`.
