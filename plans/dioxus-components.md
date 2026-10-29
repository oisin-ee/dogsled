# Official Dioxus Components for the portal

## Outcome

The portal uses official Dioxus Components for standard controls and interaction widgets.
It does not retain custom tabs, overlay behavior, choice controls, or button implementations.
The migration preserves application state, API contracts, and the current screen layout and colors.

The user authorized the migration after the source audit. This is not a visual redesign.

## Owning cause

`web/src` implements controls directly in RSX and duplicates behavior that the official library owns.
Examples include agent tabs in `agent_view.rs:394`, the inbox panel in `app.rs:350`, and annotation positioning in `review_view.rs:766`.
The package has no component library dependency or generated components.

## Dependency evidence

The manifest and lockfile use Dioxus 0.7.10. The installed CLI reports Dioxus 0.7.10.
`dx components list` successfully lists the official catalog.

The [official installation guide](https://github.com/DioxusLabs/components/blob/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/README.md) prescribes `dx components add`.
The [catalog manifest](https://github.com/DioxusLabs/components/blob/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/component.json) lists the available components.
The [button manifest](https://github.com/DioxusLabs/components/blob/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/preview/src/components/button/component.json) supplies the official upstream Git dependency.
The [primitives manifest](https://github.com/DioxusLabs/components/blob/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/primitives/Cargo.toml) identifies version 0.0.1.
The [published crate](https://crates.io/api/v1/crates/dioxus-primitives) currently contains only version 0.0.0, not this implementation.

Use the documented generator and its dependency declarations. Cargo.lock records dependency resolution.
This is the normal official distribution, not a fork, dependency patch, or workaround for a dependency defect.
Keep generated third-party source and styles unchanged. Put product theme variables in a separate Dogsled stylesheet.
Do not invent a replacement component if the official API cannot support a required behavior. Report the precise blocker.

## Component mapping

| Portal surface | Official owner | Preserved behavior |
| --- | --- | --- |
| Actions, navigation rows, submit controls | Button | Action handlers, disabled state, native submission |
| Text, reply, file, and multiline fields | Input, Textarea, Label | Controlled values, labels, uploads, keyboard submission |
| Single and multiple choice questions | RadioGroup, Checkbox | Recommendations, Other, per-choice notes, payloads |
| Agent views | Tabs, TabList, TabTrigger, TabContent | Pending-first selection, request identity, conversation retention |
| Inbox overlay | PopoverRoot, PopoverTrigger, PopoverContent | Counts, selection, dismissal, focus return |
| Annotation editor | Dialog or supported Popover API | Quote, edit, image upload, save, cancel, focus return |
| Approval-note disclosure | Collapsible | Optional notes and existing approval payload |
| Host utilization and status labels | Progress, Badge | Honest values, unknown state, existing vocabulary |

Use ordinary semantic HTML for document content, lists, forms, landmarks, and screen layout.
Native overflow is not a custom interaction widget. Do not replace application-specific domain components with generic panels.
Selection extraction, Markdown rendering, uploads, and API state remain application responsibilities.
The library must own focus, keyboard navigation, dismissal, and overlay positioning.

## Units

1. **Foundation:** Generate official components, register modules and theme assets, and prove WASM compilation.
2. **Shell and agent:** Migrate shell actions, inbox, host meters, navigation controls, agent tabs, composer, and prompt controls.
3. **Questions:** Migrate question controls and labels. Preserve recommendations, Other, notes, rejection, and response payloads.
4. **Reviews:** Migrate review actions, fields, disclosure, and annotation overlay. Remove custom overlay positioning and dismissal.
5. **Integration:** Review combined code, remove obsolete widget CSS, and verify rendering and keyboard behavior.

Units 2–4 depend on the accepted foundation. They own separate source files and surface styles.
The lead writes each later brief against the integrated foundation revision.
No worker changes generated component files, dependencies owned by another unit, server endpoints, or unrelated pending work.

## Proof

Foundation checks: `cargo check -p web --target wasm32-unknown-unknown --locked` and `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`.
Surface checks: existing `cargo test -p web` tests plus focused behavior tests for changed state transitions and component composition.
The final lead runs the web bundle and relevant repository gates once against the combined revision.
The repository owns `moon run :web-lint`, `moon run :web-build`, and `moon run :check`.
Required commit hooks remain enabled. Do not repeat an unchanged passing proof only for ceremony.

Browser verification uses the managed repository browser. Check agent switching, tabs with arrows, question answers, rejection, and annotation dismissal.
Check focus return, disabled actions, pending-request priority, and bounded screen overflow.
Do not submit real user decisions merely to test controls. Use an authorized fixture or report the untested action.
Inspect both the sidebar and overlays at the supported desktop viewport.

## Current state

The component inventory and official installation mechanism are verified. No application code changed during planning.
The foundation unit is next. The complete migration is not yet implemented.
