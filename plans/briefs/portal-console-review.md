# Portal console: plan review, questions and key prompts

Owner: lead (pane wWH:p36). Base: `origin/plan/portal-console` at the SHA in the spawn (after unit 4 merges). Profile: writer. Unit 5 of `plans/portal-redesign.md`.

## Outcome

A plan review, a question form and a key prompt look and work like the rest of the console (`plans/portal-redesign.md`, Pending items).

- **Plan review:** at 1100px and wider it matches `.impeccable/mocks/decision/v6-console-review.png`. Below 1100px it matches `v6-console-review-narrow.png` and `v6-console-review-narrow-notes.png`.
- **Questions and key prompts:** these have no sketch. Build them from the same parts: the 640px conversation measure, panel surfaces and the pinned bottom bar.

## Design sources

- **Contract:** `.impeccable/surfaces/web.md`. It sets the palette, type and radii. It allows no shadows or gradients, and keeps yellow only for "waiting on you".
- **Sketch:** `.impeccable/mocks/decision/v6-console-review.html`. `?tab=plan` and `?tab=notes` show the narrow tabs.
- **Wireframe:** `plans/portal-redesign.squig.json`, variations `wide-review` and `narrow-review`.

## Current code (at the base)

Find code by name, because unit 4 moves the lines in `review_view.rs`.

- **`web/src/agent_view.rs`, `AgentSession`:**
  - With a plan open (`plan_open`), `section.lead-panel[data-review-open]` keeps the lead header and adds `div.pending-review.console-scroll` holding `ReviewView { review_id, embedded: true }`. The lead header holds the lead title, an X icon that closes the review and Open in Herdr.
  - In the Pending tab, `QuestionForm { requested }` or `BlockedPanel` renders inside `div.lead-chat` > `ScrollArea.conversation-scroll`.
  - The width comes from `use_context::<crate::app::ConsoleWidth>()`.
- **`web/src/review_view.rs`:**
  - `ReviewView` (list and loader).
  - `ReviewEditor`, which renders:
    - the embedded `Item` header: path, `Revision n` badge and `ReviewDecisionActions` at the top;
    - a `Card "Plan"` with `#review-plan`, filled from `render_plan` output through `dangerous_inner_html`, and a `CardFooter` "Changes since revision n−1" button that toggles a diff `pre`;
    - a `Card "Your notes"`: annotation `Item`s with Edit and Delete text buttons, plus the `review-general` `Textarea`;
    - the `Dialog` with `AnnotationEditor`;
    - on the standalone page only, the approval-notes `Collapsible`.
  - `upload_image` and `MAX_IMAGE_BYTES` duplicate `api::upload_image` in `web/src/api.rs`.
- **`web/src/question_form.rs`:** `QuestionForm`, `AskForm` (submit "Send answers"), `QuestionField`, `MediaView`, `ChoiceOption`. Its tests are in `web/src/question_form/tests.rs`.
- **`web/assets/console.css`:** review-mode layout under `[data-review-open="true"]` and `.pending-review`.
- **`web/assets/markdown.css`** (unit 4) styles the plan's Markdown. Do not duplicate its rules.
- **Load order:**
  - `App` (`web/src/app.rs`) loads `dx-components-theme.css`, then `console.css`, then `markdown.css`.
  - Component `#[css_module]` sheets load after all of them, so scope every rule that overrides a component element under `.console-provider .console-shell`.
  - Do not use `!important` or IDs.

## Edits

1. **One review header** replaces the lead header while a plan is open. It shows:
   - the plan title (its first heading, else the path);
   - meta: `Revision n · <lead title> · <role> · <repository>`;
   - `Plan` / `Changes` tabs. Use the official `Tabs`. `Changes` shows the existing diff and appears only when there is more than one revision;
   - the close icon.

   Pass what the header needs from `AgentSession` into `ReviewView` as props (for example `lead` and `on_close`).
2. **Body:** the plan at a 680px measure, centred in the remaining space, scrolling on its own. To its right is a 300px notes margin with its own scroll, headed `Your notes` and the count. Each note shows:
   - the quote, with a 2px yellow left rule;
   - the comment;
   - Pencil and Trash icon buttons (`dioxus-icons` lucide) with `aria-label`s.

   Annotated passages in the plan get a faint yellow underline. With no notes, the margin says "Select text in the plan to add a note."
3. **Decision bar,** pinned under the plan and notes. It holds:
   - the `Note for the whole plan` field: an `aria-label` and no placeholder;
   - `Request changes`, secondary;
   - `Approve plan`, primary indigo.

   Keep the button enable rules. Remove the top `ReviewDecisionActions` row in embedded mode. Leave the standalone page's approval notes as they are.
4. **Below 1100px,** the header tabs become `Plan` / `Changes` / `Notes N`, and the notes margin becomes the `Notes` tab.
5. **`AnnotationEditor` dialog:** a console panel surface with no shadow and icon controls where they fit.
6. **Questions:** the form fills the centre column at the 640px measure under the lead header, with the form title and age. Number each question block. Use official `RadioGroup`/`Checkbox` rows, give recommended options an indigo `Recommended` badge, and keep the Other field. `MediaView` blocks stay inside the measure. A pinned bar at the bottom shows a muted `n of m answered` and `Send answers` (primary). No helper text.
7. **Key prompt** (`BlockedPanel` in `agent_view.rs`):
   - the question as the heading;
   - the agent screen `pre` in JetBrains Mono on the panel colour, with a 8px radius and its own scroll;
   - the option and key buttons as compact outline buttons in one wrapping row, styled like the inbox key buttons;
   - the reply composer pinned below.
8. **Stylesheet:** add `web/assets/review.css`, loaded after `markdown.css` in `App`. Move the review-mode rules out of `console.css` into it.
9. **Uploads:** switch `review_view.rs` to `api::upload_image`, then delete its local `upload_image` and `MAX_IMAGE_BYTES`.

## Owned paths

You own:
- `web/src/review_view.rs`
- `web/src/question_form.rs` and `web/src/question_form/tests.rs`
- `web/assets/review.css`
- in `web/src/agent_view.rs`, only the `plan_open` header and body branches, the Pending tab and `BlockedPanel`
- in `web/src/app.rs`, only the one `Stylesheet` line
- in `web/assets/console.css`, only the rules you move out
- `web/src/component_tests.rs`

You do not own `web/src/components/`, `dx-components-theme.css`, `markdown.css`, `highlight.rs` or the server.

## Acceptance

From the repository root, `moon run :web-lint` and `cargo test -p web` exit zero. Add `dioxus-ssr` tests that check these:
- An embedded review renders `Plan` and the close control. It renders `Changes` only with two or more revisions.
- The decision bar's note field has no placeholder.
- A question form renders the `n of m answered` count and `Send answers`.

## Checks

- **Allowed:** the acceptance commands, `cargo fmt -p web --check`, `pnpm exec oxfmt --check web/assets/review.css web/assets/console.css`.
- **Not allowed:** bundle builds, servers or browsers. The lead runs `:web-build` and the browser checks at 1440, 1280 and 1024 widths.
- **Commit hooks** stay enabled. No suppressions or `#[allow]`.
- **Publication:** commit and push your branch only.

## Reply

Send:
- the branch and SHA;
- the changed paths;
- the command results;
- any part of the sketch that the official components could not produce, with the component named and the gap.
