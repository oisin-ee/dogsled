# U7a: Accordions fill their column

## Outcome
The Files and Todos accordions span the full width of their tab at every portal width. Their group headers and rows use the portal's console styles.

## Cause
- **Fixed width:** `web/src/components/accordion/component.rs:15` passes `width: "15rem"` to the accordion root. Dioxus renders it as inline `style="width: 15rem"`.
  - At 1200 px the Files list is 240 px inside a 672 px tab, which the browser check measured.
  - Todos uses the same wrapper (`agent_view.rs`, `Accordion { key: "{todo.session}" … }`), so it has the same cap.
- **Dead CSS rules:**
  - `AccordionItem { class: "files-group" }` never reaches the DOM, because the wrapper sets the item class itself. The `.files-group` and `.files-group button` rules in `web/assets/console.css` match nothing.
  - `.files-list [data-orientation="vertical"] { width: 100% }` matches nothing either.

## Edits
1. `web/src/components/accordion/component.rs`: delete the `width: "15rem"` line. Change nothing else in the generated component.
2. `web/src/files.rs`: drop `class: "files-group"` from `AccordionItem`.
3. `web/assets/console.css`:
   - Replace the three dead Files rules with selectors that match the rendered DOM: `.files-list button` for the group trigger and `.files-list [data-slot="item-group"]`. This mirrors the existing `.todo-list button` and `.todo-list [data-slot="item-group"]` rules.
   - Keep the same values: padding `9px 0 4px`, 12 px / 500, `var(--console-muted)`, and gap 0.
   - No new colour values.
4. `web/src/component_tests.rs`: assert that the rendered Files accordion root has no inline `width`. Also assert the same for one Todos accordion, if a Todos SSR helper already exists.

## Paths
- **Owns:** `web/src/components/accordion/component.rs`, `web/src/files.rs`, `web/assets/console.css`, `web/src/component_tests.rs`.
- **Reads:** `web/src/agent_view.rs` (the Todos accordion), `web/src/components/accordion/style.css`.

## Acceptance
- `cargo test -p web` passes, with the new assertion.
- The lead's browser check at 1200 px and 800 px shows the Files group header chevron at the right edge of the tab, and Todos the same.

## Checks
- Run `cargo fmt --all --check` and `cargo clippy -p web -- -D warnings`. Commit through the pre-commit hook, without bypassing it.
- The lead builds `web/public` and runs web-lint and the browser check.

## Base
Branch `feat/files-tab` at `09e56ab`. This brief is complete; it depends on no plan file.
