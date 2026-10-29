# Dioxus Components foundation

## Outcome
The web package exposes unchanged official components and loads their theme, with WASM compilation proved.

## Cause or design
`web/Cargo.toml:11` provides the framework but no official component library. `web/src/main.rs` declares only application modules.
The official catalog supports the controls listed in `plans/dioxus-components.md`. Use its generator, not handwritten substitutes.

## Edits
- From `web/`, run `dx components add button input textarea label checkbox radio_group tabs popover dialog collapsible progress badge --rev 9a758255ea26e2b20c8cecf4c1c946feb9e71da7`.
- Let the generator create its component modules, styles, theme asset, and Cargo dependencies. Record the command and resolved upstream revision.
- Register the generated module in `web/src/main.rs` using the generator's documented output. Keep unused future components public, without lint suppressions.
- Load the official theme asset once in `web/src/app.rs`, before product styles. Add a separate `web/assets/components-theme.css` for supported theme-variable mappings.
- Map official theme tokens to the existing Catppuccin tokens in `web/assets/main.css`. Preserve current controls and layout in this unit.
- Add `dioxus-ssr = "0.7.10"` as a dev dependency for later composition tests. Let Cargo update Cargo.lock normally.
- Add a focused SSR smoke test in `web/src/component_tests.rs`, declared from main.rs, that composes official Button/Input/Textarea with accessible labels and values.
- Do not modify generated third-party Rust or CSS. Do not add wrappers, overrides, patches, forks, or replacement implementations.

## Owned paths
Repository: `github.com/oisin-ee/dogsled`. Branch: `feat/dioxus-components-foundation`. Prerequisites: none.
Editable: `web/Cargo.toml`, `Cargo.lock`, `web/src/main.rs`, theme imports only in `web/src/app.rs`, `web/assets/components-theme.css`, `web/src/component_tests.rs`.
Generated: `web/src/components/`, `web/assets/dx-components-theme.css`, any registry metadata emitted inside `web/`.
The writer runs the official generator and commits its output unchanged. No edits to other application source or existing surface CSS.
Read first: `plans/dioxus-components.md`, `PRODUCT.md`, `moon.yml:81-101`, `web/assets/main.css:1-39`.
Official source: https://github.com/DioxusLabs/components/tree/9a758255ea26e2b20c8cecf4c1c946feb9e71da7

## Acceptance
From repository root: `cargo check -p web --target wasm32-unknown-unknown --locked` exits zero.
`cargo test -p web component_tests` exits zero and proves attribute/value forwarding through actual library components.
Do not claim the portal migration is complete. Existing controls migrate in dependent units.

## Checks
Allowed: acceptance commands, `cargo fmt -p web --check`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`.
Apply formatting only to Dogsled-owned edits. Never rewrite generator output or suppress lints to pass.
Required commit hooks remain enabled. Their configured checks are authorized.
Do not run manual full-workspace gates, release builds, browser tests, installs outside repository setup, releases, or deployments.
If the official dependency or generated output blocks compilation, stop and report the exact diagnostic. Do not patch it.
Publication: commit this unit. No push, PR, merge, installation, or deployment authority.
Reply: branch, exact commit SHA, changed paths, generator provenance, commands/results, and remaining limits.

## Base
Application base: `53b6ed761a08766569fcc416a7785d4fad5321e8`.
Dispatch uses its descendant documentation commit containing this brief and the migration plan.
