# Portal model and thinking picker

## Outcome

The portal shows a model and thinking-level picker in two places:

1. The header of an existing conversation. A change updates the live Pi session through Pi's own setters.
2. The New conversation flow. The new lead starts with the chosen model and thinking level.

## Decisions

- Pi is the controlled harness. Every lead and every default worker runs Pi.
- Claude Code and Codex run as terminal TUIs. Neither has a supported way for Dogsled to change a live session.
  Their conversations show the recorded model read-only, with the note "Model changes work for Pi sessions only".
  Live control for them needs a new launch path (Codex app server, Claude Agent SDK). That is a separate decision.
- A change follows Pi's native behaviour: `pi.setModel()` and `pi.setThinkingLevel()`. There is no queue and no idle-only rule.
- `pi.setModel()` returns `false` when the provider has no credentials. The portal shows that refusal.
- Agents keep host-qualified identity: `/api/agents/{host}/{pane}/...`. Remote hosts use the existing peer transport.
- Each Pi session publishes its own choices: `ctx.scopedModels` when it is set, else `ctx.modelRegistry.getAvailable()`.
  Each choice carries `getSupportedThinkingLevels(model)` from `@earendil-works/pi-ai`.
- The New flow reads a serve-owned catalogue. Serve runs `pi --mode rpc --no-session`, sends `get_available_models`, and publishes the result.
  It refreshes at startup and at most hourly, and keeps the last good list on error, like the repository catalogue.
- The lead record and the `leads` table do not store the model. A store schema bump restarts every host's store empty (`src/store.rs`), and a lead never retries an uncertain start, so nothing reads it back. Pi's session records the model.
- The New lead passes `--model provider/id` and `--thinking level` to Pi. The lead goes to the roomiest host.
  If that host lacks the model, Pi's startup error is the launch failure.
- The settings channel is separate from the live-activity producer protocol. `LiveState` denies unknown fields, so a new field would break
  running sessions across upgrades.
- Pickers use the official Dioxus components already in `web/src/components`. The model list is searchable, like the repository picker.

## Wire types

`portal-types/src/models.rs`: `ThinkingLevel`, `ModelRef`, `ModelChoice`, `AgentSettings`, `SettingsChange`, `ModelCatalogue`.

## Units

| Unit | Deliverable | Depends on |
| --- | --- | --- |
| A | Live Pi settings: Pi extension publishes settings and applies changes; serve and web route them, locally and to peers; conversation header picker | types |
| B1 | New-lead model, server side: serve model catalogue, `GET /api/models`, `CreateLeadBody` model and thinking, Pi launch flags | types |
| B2 | New-flow picker, reusing A's picker component | A, B1 |
