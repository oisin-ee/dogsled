# Brief: portal question form (web portal step 4b)

**Outcome.** When a Pi agent calls `interview`, its form shows in the portal's right column. You answer it there, and the tool gets the answers.

**Server API.** The routes are on main in `src/web/questions.rs`. The types are in `portal-types/src/questions.rs`.
- `GET /api/questions` returns `PendingList{pending:[Pending{id,pane,session,cwd,created_ms,questions:QuestionSet{title?,description?,questions:[Question{id,type,question,options?,recommended?,conviction?,weight?,context?,content?,media?}]}}]}`.
- `POST /api/questions/{id}/answer` takes `AnswerBody{responses:[ResponseItem{id,value,attachments?}]}`. The page sends it from the same origin, so the Origin guard accepts it.

**Question schema.** It is the pi-interview 0.13.0 schema. Read `~/.pi/agent/npm/node_modules/pi-interview/README.md` and `form/` for the details.
- Value shapes:
  - single: a string.
  - multi: a string[].
  - text: a string.
  - info: no value.
- Recommended options are pre-selected, unless `conviction` is `"slight"`. When it is `"strong"`, they also get a badge.
- `weight` is `"critical"` or `"minor"`.
- Each option is a string, or `{label, content?}`.

**Existing UI code.** `web/src/app.rs` renders `QuestionForm {}` in the right column. It provides a `Signal<Option<String>>` context with the selected pane.

**Edits.**
- **`web/src/question_form.rs`:**
  - Poll `GET /api/questions` every 2 s.
  - Show a "Needs you" list of the pending forms, with the title, pane and age of each.
  - Open the first form for the selected pane by default, or the form that you click.
  - Inputs:
    - single: radio buttons.
    - multi: checkboxes.
    - text: a textarea.
    - info: no input.
  - Show the recommended badge and the pre-selection, the weight styling, and `context` as muted text.
  - Show `content` in a pre. For `lang: md`, plain text in a pre is acceptable for this unit.
  - For `image` questions and for `media`, show "not supported in the portal yet".
  - Submit posts the responses, then removes the form from the list.
  - Keep this form's API calls in this file. Do not edit `api.rs` or `main.css`.
- **New `web/assets/question_form.css`:** load it with `document::Stylesheet` in QuestionForm.

**Owned paths:** web/src/question_form.rs, web/assets/question_form.css.

**Acceptance.**
1. The checks exit 0.
2. Start `dogsled web`.
3. Post a form with curl:
   - Use `POST /api/questions` with the body `{questions:{...},pane:"test",session:"s",cwd:"/tmp"}`.
   - Send the token from `<state>/web/token`. `<state>` is `DOGSLED_STATE`, or else `~/.local/state/dogsled`.
   - Include one question of each type: single (with a recommended option), multi, text and info.
4. Answer the form in a browser, with Playwright or headless Chrome.
5. Show that `GET /api/questions/{id}/answer`, sent with the token, returns the responses.
6. Report the path of a screenshot of the form, and the answer JSON.
