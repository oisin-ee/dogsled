# Dioxus Components: review controls and annotation overlay

## Outcome
The review view uses official Button, Textarea, Input, Label, Collapsible and Dialog or Popover. The annotation editor no longer positions or dismisses itself with hand-written JavaScript.

## Cause or design
The portal hand-builds controls that the official components own. Plan: `plans/dioxus-components.md`. Official components live in `web/src/components/` (project-owned, formatted by rustfmt). Read their `component.rs` files for the real props before use. Do not edit them.

## Edits
- `review_view.rs:766` `AnnotationEditor`: replace the `section.rv-popover`, its `document::eval` positioning, resize/scroll observers and Escape handler with the official `Dialog` (or `PopoverRoot`/`PopoverContent` if its API fits the selection anchor better). Open state comes from the existing `editing` signal. Delete the positioning script and the `--popover-left/top` CSS.
- `review_view.rs:692` approval-notes `details`: use `Collapsible`.
- All `button`, `textarea`, `input` elements (~135, 396, 400, 613, 656, 659, 668, 694, 794-819): use `Button`, `Textarea`, `Input`, `Label`.
- Keep selection extraction, markdown rendering, image upload eval, annotations and the decision payload unchanged.
- Delete CSS the replaced controls no longer need.

## Owned paths
`web/src/review_view.rs`, `web/assets/review_view.css`
Do not touch other web source files, generated components, server code, or Cargo.toml.

## Acceptance
From the repository root: `cargo check -p web --target wasm32-unknown-unknown --locked` and `cargo test -p web` exit zero. Annotation save, edit, close and image removal still call the same handlers.

## Checks
Allowed: the acceptance commands, `cargo fmt -p web --check`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`. No heavy gates, bundles, or browser runs; the lead does those once. Commit hooks stay enabled. Publication: commit only.

## Rules
Use the official components for every control listed. Keep API calls, payloads, state, and layout classes. No wrappers that re-implement a component. No suppressions. If a component cannot do a required thing, stop and report the exact gap.

## Base
The foundation commit on `feat/dioxus-components-foundation`. Reply: branch, SHA, changed paths, command results.
