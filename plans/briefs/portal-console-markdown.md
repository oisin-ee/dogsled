# Portal console: Markdown and code highlighting

Owner: lead (pane wWH:p36). Base: `origin/plan/portal-console` after unit 3 merges; the spawn names the SHA. Profile: writer. Unit 4 of `plans/portal-redesign.md`.

## Outcome

Chat turns and plans render GitHub-style Markdown (tables, strikethrough, task lists) with coloured fenced code, styled to the console contract in `.impeccable/surfaces/web.md`, with the existing safety rules intact.

## Current code (review_view.rs is unchanged by unit 3)

- `web/src/review_view.rs:242` `render_markdown(source)` calls `render_plan(source, &[])` (line 246), which parses with `Parser::new(source)` and no options (line 247), adds `data-start-line`/`data-end-line` to blocks listed in `block()` (line 215), turns raw HTML into text through `marked()` (line 279), drops unsafe link and image URLs (`safe_url`, line 229), and wraps annotation quotes in `<mark>` (`quoted_ranges`, line 354).
- Chat calls `render_markdown` from `Conversation` in `web/src/agent_view.rs`. Turns are `{ role, text }` only (`portal-types/src/agents.rs:57`).
- pulldown-cmark 0.13.5 option names (checked in its `src/lib.rs`): `Options::ENABLE_TABLES`, `ENABLE_STRIKETHROUGH`, `ENABLE_TASKLISTS`; `Parser::new_ext(text, options)`.

## Highlighter (chosen by unit 2, sources in the lead's notes)

```toml
syntect = { version = "=5.3.0", default-features = false, features = ["html", "regex-fancy"] }
two-face = { version = "=0.5.2", default-features = false, features = ["syntect-fancy"] }
```

Defaults stay off on both, or feature unification brings back Oniguruma (C). Load `two_face::syntax::extra_newlines()` once (`std::sync::LazyLock`). Use `ClassedHTMLGenerator::new_with_class_style` with `ClassStyle::SpacedPrefixed { prefix: "syn-" }`, so `entity.name.function` becomes `syn-entity syn-name syn-function`.

## Edits

1. Parse with `Parser::new_ext` and the three options. Add `Tag::Table` to `block()` so tables get line ranges for annotations.
2. In `render_plan`, buffer every `Event::Text` between `Start(Tag::CodeBlock(CodeBlockKind::Fenced(info)))` and `End(TagEnd::CodeBlock)`; resolve the first info token (aliases `rs`, `sh`/`shell`, `js`, `ts`, `yml`, `md`, `py`), and emit the generator's HTML as one `Event::Html`. Unknown or missing language, a generator error, or any annotation mark inside the block: keep today's escaped (and marked) text. Never send parser-originated raw HTML down the highlighted path.
3. Put the highlighter in a new `web/src/highlight.rs`.
4. Add `web/assets/markdown.css`, loaded next to the other stylesheets in `App` (`web/src/app.rs`): headings, paragraphs, lists, task-list checkboxes, `del`, blockquote, links, inline code, code blocks (JetBrains Mono, raised panel, horizontal scroll) and tables, plus `syn-*` colours from the console palette. Scope it to the Markdown containers so component styles are untouched.
5. Do what two-face's Legal section requires for the embedded grammars' licences, in `docs/third-party/`, and name the file in your reply.

## Owned paths

`web/src/highlight.rs`, the render functions and their tests in `web/src/review_view.rs` (`block`, `render_markdown`, `render_plan`, `marked`, `quoted_ranges` and helpers), `web/assets/markdown.css`, the stylesheet line in `web/src/app.rs`, `mod highlight` in `web/src/main.rs`, `web/Cargo.toml`, `Cargo.lock`, `docs/third-party/`. Not the `ReviewView` components, `question_form.rs`, or `web/src/components/`.

## Acceptance

From the repository root: `moon run :web-lint :web-build` and `cargo test -p web` exit zero, with new tests: a fixture with a heading, list, inline code, a `rust` fence (output has `syn-` classes), a `toml` fence, a table, `~~struck~~` (`<del>`), and a task list renders each element; an unknown-language fence and a fence containing `<script>` stay escaped text; raw HTML outside fences stays text; existing annotation tests pass.

## Checks

Allowed: the acceptance commands, `cargo fmt -p web --check`. Report the size of the release `.wasm` in `web/public` before and after your change (same `moon run :web-build`). Not `moon run :check`; the lead runs it. Commit hooks stay enabled. No suppressions. Publication: commit and push your branch only.

## Reply

Branch, SHA, changed paths, command results, the two `.wasm` sizes, and the acknowledgements file.
