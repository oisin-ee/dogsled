# Dioxus Components: question controls

## Outcome
The question form uses official RadioGroup, Checkbox, Label, Textarea and Button. Single and multi choice no longer use hand-built label/input pairs.

## Cause or design
The portal hand-builds controls that the official components own. Plan: `plans/dioxus-components.md`. Official components live in `web/src/components/` (project-owned, formatted by rustfmt). Read their `component.rs` files for the real props before use. Do not edit them.

## Edits
- `question_form.rs:723` `ChoiceOption`: single choice uses `RadioGroup`/`RadioItem`; multi choice uses `Checkbox`. Keep the Recommended badge (`Badge`), per-choice note, and Other behavior.
- `question_form.rs:558` and the note textareas at ~298 and ~767: use `Textarea` with a `Label` so every text answer has its own accessible name.
- Submit, reject and other buttons (~95, 293, 304): use `Button`.
- Keep `collect_responses`, `choose`, `set_note` and the response payload shape unchanged. Existing tests in `question_form/tests.rs` must pass unmodified; add one test for any new state logic.
- Delete CSS the replaced controls no longer need.

## Owned paths
`web/src/question_form.rs`, `web/src/question_form/tests.rs`, `web/assets/question_form.css`
Do not touch other web source files, generated components, server code, or Cargo.toml.

## Acceptance
From the repository root: `cargo check -p web --target wasm32-unknown-unknown --locked` and `cargo test -p web` exit zero. Response payloads are identical for the same inputs.

## Checks
Allowed: the acceptance commands, `cargo fmt -p web --check`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`. No heavy gates, bundles, or browser runs; the lead does those once. Commit hooks stay enabled. Publication: commit only.

## Rules
Use the official components for every control listed. Keep API calls, payloads, state, and layout classes. No wrappers that re-implement a component. No suppressions. If a component cannot do a required thing, stop and report the exact gap.

## Base
The foundation commit on `feat/dioxus-components-foundation`. Reply: branch, SHA, changed paths, command results.
