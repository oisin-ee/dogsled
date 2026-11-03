# Brief: plan review UI (web portal step 5c)

**Outcome.** When a Pi agent calls `submit_plan`, the portal shows the plan beside that agent's conversation. You can annotate the plan, quote passages into the message box, see what changed between revisions, and approve the plan or request changes.

**Server API.** The routes are on main in `src/web/reviews.rs`. The types are in `portal-types/src/reviews.rs`.

| Route | Purpose |
|---|---|
| `GET /api/reviews` | `ReviewList{reviews:[ReviewSummary{id,pane,session,path,revisions,annotations,decision?}]}` |
| `GET /api/reviews/{id}` | `Review{id,pane,session,cwd,path,revisions:[Revision{n,content,submitted_ms}],annotations,general,decision?}` |
| `GET /api/reviews/{id}/diff?from=n&to=m` | `DiffView{from,to,changes:[LineChange{kind:same/added/removed,old?,new?,text}]}` |
| `PUT /api/reviews/{id}/annotations` | Send the full list as `AnnotationsBody{annotations:[Annotation{id,revision,start_line,end_line,quote,comment,images}]}` |
| `PUT /api/reviews/{id}/general` | `GeneralBody{general}` |
| `POST /api/attachments` | Raw PNG, JPEG or WebP body, at most 10 MiB. Returns `Attachment{path}`. |
| `GET /api/attachments/{name}` | Returns the image. |
| `POST /api/reviews/{id}/approve` | `ApproveBody{notes?}` |
| `POST /api/reviews/{id}/request-changes` | No body. The server builds the message. |

Every POST and PUT is sent from the page's own origin.

**Existing UI code.**
- `web/src/app.rs` provides the selected pane, and renders `ReviewView {}` in the right column.
- `web/src/agent_view.rs` defines `pub struct Draft(pub Signal<String>)`. When a `Draft` context exists, AgentView's message box uses it for its text.
- For patterns, copy `web/src/question_form.rs` (polling, signals, forms) and `web/src/api.rs`.

**Edits.**
- **`web/src/app.rs`:**
  - Provide one `Draft` context at App level, so the review and the message box share it.
  - Deep links (the fallback unit uses them):
    - `/?review=<id>` shows only that review, full width.
    - `/?question=<id>&pane=<pane>` sets the selected pane to `<pane>`, and shows only the right column with QuestionForm.
- **`web/src/review_view.rs`:**
  - **Reviews to show:** poll `GET /api/reviews` every 2 s. Show the open reviews (no decision) for the selected pane, plus a "Needs you" list of all open reviews.
  - **Rendering:**
    - Render the latest revision with pulldown-cmark 0.13.5.
    - Use the source offsets to tag each block with `data-start-line` and `data-end-line`.
    - Highlight the lines of saved annotations.
  - **Popover:** selecting text in the plan opens a popover. Read the selection and its line range through `document::eval` and `window.getSelection()`. The popover holds:
    - The quoted selection.
    - A comment box.
    - Images, added by paste, drop or a file picker. Each image is uploaded to `/api/attachments` and shown as a thumbnail.
    - **Save**, which adds a draft annotation and PUTs the full list.
    - **Quote in message**, which appends `> <quote>` and `(plan lines a–b)` to the `Draft`.
    - A close button.
  - **Annotations list:** the draft annotations, with edit and delete (both PUT the full list), and a general comment textarea (PUT on blur).
  - **Revision diff:** when the review has more than one revision, a "Changes since revision n−1" toggle shows the `/diff` lines, styled by kind.
  - **Actions:**
    - **Approve**, with an optional notes field.
    - **Request changes**, disabled when there are no annotations and no general comment.
    - After either action, the review leaves the open list.
- **New `web/assets/review_view.css`:** load it with `document::Stylesheet`. Reuse the variables from `main.css`.
- **`web/Cargo.toml` and `Cargo.lock`:** add `pulldown-cmark = { version = "0.13.5", default-features = false, features = ["html"] }`, and any web-sys features you need.

**Owned paths:** web/src/review_view.rs, web/src/app.rs, web/assets/review_view.css, web/Cargo.toml, Cargo.lock.

**Acceptance.**
1. `cargo fmt --check -p web`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings` and `cargo test -p web` pass.
2. Unit tests cover the line map: a heading, a list and a code block each get the right start and end line.
3. Unit tests cover the Quote-in-message text.
4. CI on the PR passes: rust on macOS and Ubuntu, and typescript.
