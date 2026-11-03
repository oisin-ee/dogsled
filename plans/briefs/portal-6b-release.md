# Brief: release Dogsled 0.25.0 (web portal step 6b)

**Outcome.** A merged `chore(release): 0.25.0` PR, which the lead then tags `v0.25.0`. The tag triggers cargo-dist, and the release publishes the binary archives (including `dev.dogsled.web.plist`) and `@oisin-ee/pi-dogsled` 0.25.0.

**Design.** Copy the 0.24.0 release commit exactly: `b651127` ("chore(release): 0.24.0", PR #152). It changed these nine files, and only these:
- CHANGELOG.md
- Cargo.toml
- Cargo.lock
- README.md
- herdr-plugin.toml
- package.json
- package-lock.json
- plugins/dogsled/.claude-plugin/plugin.json
- plugins/dogsled/.codex-plugin/plugin.json

Read it with `git show b651127`. Replace every `0.24.0` it set with `0.25.0`. Do not change the version of the workspace crates `portal-types` or `web`.

**CHANGELOG entry.** Use `## 0.25.0 - 2026-10-27`, written in the same plain style as the 0.24.0 entry:
- `dogsled web` serves a local portal on 127.0.0.1:7465 (`[web] port`). It shows:
  - The board, grouped by repository, with todos.
  - The Herdr agents, with their conversations.
  - A message box.
  - A panel that answers a blocked agent with the keys its prompt allows.
  - The portal is a Rust/WASM Dioxus page, embedded in the binary.
  - Only loopback Host values and the portal's own Origin are accepted.
  - The Pi tools authenticate with a per-user token in `<state>/web/token`.
- The Pi extension adds two things:
  - Its own `interview` tool, with pi-interview 0.13.0's parameters, answered in the portal.
  - A plan mode: `/plan` or `Ctrl+Alt+P`, writes limited to markdown, and `submit_plan`.
- In the portal, plans can be:
  - Reviewed, with a line map and annotations (text, images, and Quote in message).
  - Compared between revisions.
  - Approved, or sent back with requested changes.
  - An approval emits `dogsled:plan-approved`, which pi-todo consumes.
- `dogsled interview` and `dogsled review` serve the same form or review once, on a random loopback port, when `dogsled web` is not running. The Pi tools fall back to them.
- Release archives include `dev.dogsled.web.plist`. `dogsled web` exits promptly on SIGTERM and SIGINT, even with open streams.

**Owned paths:** the nine files above.

**Acceptance.**
1. `git diff --stat origin/main` lists only the nine files.
2. `rg -n "0\.24\.0" -g '!CHANGELOG.md'` prints nothing that the 0.24.0 release commit had set.
3. `cargo metadata --format-version 1 --no-deps | jq -r '.packages[]|select(.name=="dogsled").version'` prints 0.25.0.
4. Commit as `chore(release): 0.25.0`, push branch `release/0.25.0`, open a PR, and report its number and SHA. Do not create or push a tag.
