# Brief: open the portal when an agent asks a question or submits a plan

**Outcome.** When the Pi `interview` tool or `submit_plan` hands work to a running `dogsled web`, the user sees it at once. A browser tab opens on the deep link, as pi-interview did. Today nothing opens, so the user never sees the question.

**Cause, confirmed by the lead on the installed 0.25.2.**
- A rondo agent's interview `r-1793101509764-63865-32` sat pending in `/api/questions` and showed in the portal's "Needs you" list. Nothing told the user, and no tab opened.
- `pi/interview.ts` `run()` POSTs `/api/questions` and then only waits.
- `pi/plan-mode.ts` POSTs the review and only waits.
- Only the fallback (`dogsled interview` / `dogsled review`, in `src/web/oneshot.rs`) opens a browser, through `open` on macOS or `xdg-open` elsewhere (`oneshot.rs` ~143).
- The portal already supports deep links: `/?question=<id>&pane=<pane>` and `/?review=<id>`. The questions and reviews APIs return the id.

**Edits.**
1. **`pi/portal-client.ts`:**
   - Add `portalUrl(path)`, built from the same port logic as the client.
   - Add `openInBrowser(url)`. It spawns `open` on darwin and `xdg-open` elsewhere, detached with `stdio:"ignore"`. A spawn error is ignored, because the URL is also in the tool output.
   - Match the opener choice in `oneshot.rs`.
2. **`pi/interview.ts`:**
   - After a successful POST, open `/?question=<id>&pane=<pane>`. Do this for sync and async calls alike.
   - Put the URL in `details.url` and in the "started" or "completed" text.
   - Update the tool description at line ~43 to say it opens the Dogsled portal (or a fallback tab). Remove the Orca/Glimpse wording.
3. **`pi/plan-mode.ts`:**
   - After a successful review submit, open `/?review=<id>`, but only for revision 1 of a review id, so each resubmission doesn't open another tab.
   - Put the URL in the tool text.
4. **Tests:** follow the existing tests for these files under `pi/` or `test/`.
   - The opener is called once with the right deep link for interview, for a first plan submit, and not for a resubmit.
   - Inject the opener; do not spawn a real browser.
5. **CHANGELOG:** before you commit, run `git fetch origin && git rebase origin/main`.
   - If `## 0.25.3` exists, add this bullet under it: "The Pi `interview` tool and `submit_plan` open the portal on the new question or plan."
   - Otherwise add `## Unreleased` with that bullet.
   - Do not bump versions.

**Owned paths:** pi/portal-client.ts, pi/interview.ts, pi/plan-mode.ts, their tests, CHANGELOG.md (that one bullet only).

**Acceptance.**
- `npm run --silent typecheck` and `moon run :test-shim` pass, or the repo's Pi test command (`moon run :typecheck :test-shim`).
- Push branch `fix/pi-open-portal`, open a PR, and report its number and SHA.
- Do not create or push a tag.
