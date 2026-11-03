# Dioxus Components: shell and agent controls

## Outcome
The shell, board, and agent view use official Button, Tabs, Popover, Textarea, Input, Badge and Progress. No hand-built tab, overlay, or button remains.

## Cause or design
The portal hand-builds controls that the official components own. Plan: `plans/dioxus-components.md`. Official components live in `web/src/components/` (project-owned, formatted by rustfmt). Read their `component.rs` files for the real props before use. Do not edit them.

## Edits
- `agent_view.rs:394`: replace the `nav.av-tabs` buttons with `Tabs`/`TabList`/`TabTrigger`/`TabContent`. Drive the active value from the existing `tab` and `choice` state through `value` and `on_value_change`. Keep pending-first selection.
- `app.rs:273` and `app.rs:350`: replace the Inbox toggle and the `inbox-panel` section with `PopoverRoot`/`PopoverTrigger`/`PopoverContent`. Drop the manual Escape handler and `aria_expanded` wiring.
- `app.rs:287`: render host load with `Progress`.
- All other `button`, `textarea`, `input` elements in these three files (composer, prompt keys, reply, nav rows, inbox items, Open in Herdr): use `Button`, `Textarea`, `Input`. Status and kind tags use `Badge`.
- Delete CSS rules these elements no longer need. Keep layout rules.

## Owned paths
`web/src/app.rs`, `web/src/board.rs`, `web/src/agent_view.rs`, `web/assets/main.css`, `web/assets/agent_view.css`
Do not touch other web source files, generated components, server code, or Cargo.toml.

## Acceptance
From the repository root: `cargo check -p web --target wasm32-unknown-unknown --locked` and `cargo test -p web` exit zero. The existing web tests still pass.

## Checks
Allowed: the acceptance commands, `cargo fmt -p web --check`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`. No heavy gates, bundles, or browser runs; the lead does those once. Commit hooks stay enabled. Publication: commit only.

## Rules
Use the official components for every control listed. Keep API calls, payloads, state, and layout classes. No wrappers that re-implement a component. No suppressions. If a component cannot do a required thing, stop and report the exact gap.

## Base
The foundation commit on `feat/dioxus-components-foundation`. Reply: branch, SHA, changed paths, command results.
