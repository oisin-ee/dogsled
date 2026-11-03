# Portal console: finish fixes

Owner: lead (pane wWH:p36). Base: `origin/plan/portal-console` after `origin/main` merges into it (the SHA is in the spawn). Profile: writer. Unit 6 of `plans/portal-redesign.md`.

## Outcome

The merged console matches the approved sketches at the places below. The lead saw each item in the browser on the base build at 1440, 1280 and 1024 widths (`/tmp/console-verify6/`). Where the lead measured a cause, the item names it.

## Design sources

- **Contract:** `.impeccable/surfaces/web.md`: palette, type, radii; no shadows or gradients; yellow only for "waiting on you".
- **Sketches:** `.impeccable/mocks/decision/v6-console.html` (main screen) and `v6-console-review.html` (plan review), with their `.png` captures.
- **Cascade rule:** component `#[css_module]` sheets load after the portal sheets. Scope every override of a component element under `.console-provider .console-shell`. Do not use `!important` or IDs in selectors.
- **Components:** do not edit `web/src/components/` or `web/assets/dx-components-theme.css`.

## Edits

1. **Host load meters (top bar).** The sketch shows short meters on the right. The meters are 200px wide, because the `Progress` sheet sets `min-width: 200px` on `.dx-progress-*`, and the console rule sets only `width: 48px`. In `console.css`, let the console rule also set the minimum width, so each meter is 48px.
2. **Badges and cards that lose their component class.** `Badge` and `Card` (`web/src/components/badge`, `card`) spread the caller's attributes after `class: Styles::…` without `merge_attributes`. A `class` from the caller replaces the component class, so the component styles never apply. The lead measured the inbox count: it has only `class="inbox-count"`, padding 0 and radius 0, so it shows as a bare yellow bar.
   - Stop passing `class` to `Badge` and `Card`. Target them with a data attribute instead, for example `"data-part": "inbox-count"`. The call sites are:
     - `app.rs`: `Badge { class: "inbox-count" … }`
     - `question_form.rs`: `Badge { class: "recommended-badge" … }`, `Card { class: "question-block" … }` and `Card { class: "question-rejection" … }`
   - Update the matching selectors in `console.css` and `review.css`. Where the restored component styles conflict with the contract (for example a shadow), override them.
   - Both badges match the sketch pill: 12px, weight 600, line height 18px, padding 0 7px, radius 9px. The inbox count is yellow on the ground colour while items wait. `Recommended` is indigo with white text.
   - The question cards and the rejection card look the same as on the base.
3. **Review title in the inbox.** A plan review row shows the plan's absolute file path (for example `/tmp/work/plans/console-review-fixture.md`), on two lines. The sketch shows `Typed SQLite state store, revision 3`. The cause is the server: `src/web/inbox.rs` sets `title: review.path.clone()`.
   - Move `plan_title` from `web/src/review_view.rs` into `portal-types` (for example `portal_types::reviews::plan_title`), with `pulldown-cmark = "0.13.5"` and `default-features = false`. The review header and the inbox then use one function.
   - In `src/web/inbox.rs`, the review title is `{plan title}, revision {n}`, from the latest revision. If the plan has no heading, use the file name (the last path segment) instead of the title.
   - Add a unit test next to the existing inbox tests.
4. **Review header meta.** The sketch reads `Revision 3 · Move state into typed SQLite · lead · dogsled`. The base shows the lead's full task title, and then `github.com/oisin-ee/dogsled`. At 1024px the title's ellipsis hides `lead` and the repository.
   - Show the repository by its last path segment.
   - Put the lead title in its own element that truncates with an ellipsis, so the revision, the role and the repository always stay visible.
5. **Review decision note field.** The sketch shows no resize handle. Give the `Note for the whole plan` field `resize: none` and content sizing (1lh to 8lh), as the composer has.
6. **Todos column** (`Todos` in `agent_view.rs`).
   - The wide header shows `n of m` at the right, as the sketch's `28 of 32` does. Count completed todos against all todos, as the narrow `Todos n/m` tab already does.
   - Do not render a section that has no items, such as `Pending · 0`.
   - Completed and abandoned items show their title only, in muted text, with no description or worker line. In-progress and pending items keep their worker line and description.
   - The empty state `No todos are recorded for this agent.` is muted 13px text with the column padding, like the notes margin's `Select text in the plan to add a note.`, not a raised panel. Do the same for the inbox empty state `Nothing is waiting on you.`. Style it in the column CSS, not in `notice.rs`.
7. **Agents sidebar** (`RepoGroup` in `board.rs`, and `app.rs`).
   - A repository group header reads `dogsled` on the left and the agent count on the right, as in the sketch, not `dogsled · 4 agents`.
   - `29 finished branches have no repository — see Land and clean up` is a large raised panel at the end of the list, and no `Land and clean up` section exists any more. Render it as a muted row in the style of the `n branches to land` row, without the dead reference: `29 finished branches have no repository`.
8. **Markdown tables and the plan ID** (`markdown.css`).
   - Header cells are centred by the browser default. The sketch aligns them to the start. Set `text-align: start` on `th`.
   - The container sets `overflow-wrap: anywhere`, so in a narrow table column a long token such as `r-1793628516452-21481-9995` splits between digits over four lines. In table cells use `overflow-wrap: break-word`; the table already scrolls sideways.
   - Every rule selects `:is(.conversation-turns > article > div, #review-plan)`, which uses an ID. Give the plan body and each chat message body one class (for example `markdown-body`), and select only that class. Keep `id: "review-plan"`, because `review_view.rs` uses it in `closest(…)`.

## Owned paths

You own:
- `web/src/app.rs`, `web/src/board.rs`, `web/src/agent_view.rs`
- `web/src/review_view.rs`
- `web/src/question_form.rs` and `web/src/question_form/tests.rs`
- `web/src/component_tests.rs`
- `web/assets/console.css`, `web/assets/review.css`, `web/assets/markdown.css`
- `portal-types/src/reviews.rs` and `portal-types/Cargo.toml`
- `src/web/inbox.rs`
- `Cargo.lock` (dependency edits only)

You do not own `web/src/components/`, `dx-components-theme.css`, `notice.rs`, `highlight.rs` or the other server files.

## Acceptance

From the repository root:
- `moon run :web-lint` exits zero.
- `cargo test -p web` and `cargo test -p portal-types` exit zero.

Add or update tests that check these:
- The inbox count and the `Recommended` badge keep the component badge class (`dioxus-ssr`).
- The todos header renders `n of m`, and an empty section is not rendered (`dioxus-ssr`).
- The review header renders the repository's last path segment and the lead title in its own element (`dioxus-ssr`).
- `plan_title` returns the first heading without markup (`portal-types`).
- The review inbox title is `{title}, revision {n}`, or the file name when the plan has no heading (`src/web/inbox.rs`).

## Checks

- **Allowed:** the acceptance commands, `cargo fmt --all --check`, and `pnpm exec oxfmt --check` on the CSS files you changed.
- **Not allowed:** bundle builds (`dx`, `:web-build`), the root crate's build, Clippy or tests, servers and browsers. The root crate embeds `web/public`, which only a bundle build fills. The lead runs `:web-build`, the root tests and the browser checks at 1440, 1280 and 1024 widths.
- **Commit hooks** stay enabled. No suppressions or `#[allow]`.
- **Commits:** The hook runs the root build for `portal-types/`, `src/` and `Cargo.lock`, and a writer may not run that build. So:
  1. Make edits 1, 2 and 4 to 8 first. Commit and push them yourself. They touch only `web/`.
  2. Then make edit 3. Leave all of edit 3 uncommitted, including its `web/src/review_view.rs` part, so that each commit builds.
  3. Reply with `ask`, not `done`, so your worktree stays open. The lead commits edit 3 with the hook in your worktree and pushes it. Then the lead tells you to finish.

## Reply

Send:
- the branch and the SHA of your commit;
- the paths you left uncommitted for edit 3;
- the command results;
- any edit you could not make with the official components, with the component named and the gap.
