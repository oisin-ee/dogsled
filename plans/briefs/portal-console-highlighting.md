# Portal console: choose a code-highlighting library

Owner: lead (pane wWH:p36). Base: `origin/plan/portal-console`. Plan: `plans/portal-redesign.md` (section Markdown).
Profile: selection-lead. Read-only: edit no repository file. Scratch builds go under `/tmp`.

## Outcome

One recommended Rust library, with exact version and features, that colours fenced code blocks in the portal's chat and plan Markdown, plus the evidence for the choice.

## Context

- The portal UI is Rust only (`PRODUCT.md:34`): a Dioxus 0.7 client built for `wasm32-unknown-unknown` with `dx bundle --web`. No JavaScript highlighter (highlight.js, Prism, Shiki) in the browser.
- Markdown goes through `pulldown-cmark` 0.13.5 with the `html` feature (`web/Cargo.toml`). The renderer is `render_plan` in `web/src/review_view.rs:246`: it collects `Parser::new(source).into_offset_iter()` events and writes them with `html::push_html` (line 322). Chat uses `render_markdown` (line 242), which calls `render_plan(source, &[])`.
- The colours come from the portal stylesheet, so class-based output (spans with CSS classes) is preferred over inline styles.
- Languages that must work: rust, bash/sh, toml, json, yaml, typescript/javascript, diff, markdown, python, sql. Unknown or missing languages fall back to plain escaped text.

## Questions to answer

1. Which candidates build for `wasm32-unknown-unknown` with no C toolchain? Check at least syntect (`default-fancy` / `fancy-regex`, `ClassedHTMLGenerator`), two-face, giallo, syntastica, tree-sitter-highlight, inkjet, autumnus and arborium. Prove a build claim with a scratch crate under `/tmp` (`cargo build --target wasm32-unknown-unknown --release`) or a primary source.
2. Release `.wasm` size added by each viable candidate with only the languages above, measured from the scratch crate.
3. Maintenance: latest release date, licence, open blocker issues for wasm.
4. The integration: how to replace the `Event::Text` inside `Tag::CodeBlock(CodeBlockKind::Fenced(lang))` with highlighted HTML in the existing event stream, as a short Rust sketch against pulldown-cmark 0.13.5. Note anything that breaks the raw-HTML escaping rule in `render_plan` (raw HTML in Markdown stays text).
5. The CSS class names the output uses, so the stylesheet can colour them.

## Reply

The pick (crate, version, features), the measured size delta, the integration sketch, the class names, and each rejected candidate with a one-line reason and its source. Cite crates.io, docs.rs or repository URLs for every external claim.
