# Local Herdr web portal

## Context

You want one browser window that shows Herdr and Dogsled together, so you stop
switching between Herdr tabs, Glimpse windows and stray browser tabs. This first
version runs only on this Mac (`127.0.0.1`). Remote hosts, phone access, SSO and
push are deferred.

First working version:
1. Dogsled board grouped by **repository**, with todos.
2. Herdr agent list with a native agent view. There is **no terminal emulator**: each agent's conversation is drawn from its transcript.
3. Send a message to an agent (`agent.prompt`).
4. **Our own plan mode and plan review with annotations**, replacing `@plannotator/pi-extension`. It covers plan mode, its tools and plan annotations, including the annotation popover with image attachments. There is no separate Ask AI: the agent's conversation sits beside the plan. Code review, file annotate (`/plannotator-annotate`), `last` and the `plannotator` tool are out.
5. **Our own `interview` tool**, replacing `pi-interview`. It uses the same tool name and question format.

Revision notes (your review):
- The UI is **Rust only**: a **Dioxus** 0.7 web client compiled to WASM.
- Plan annotation stays, matching what you use today (see "Plan annotation" under Approach).
- There is no terminal UI.
- Both third-party tools are replaced by ours.
- When the portal is down, the tools behave the way the current tools do today: a form opens in a browser tab.
- **Blocked agents** are answered in the portal: it shows the agent's visible prompt text with likely key buttons and a short input, sent through `herdr agent send-keys`. **Open in Herdr** remains the fallback.
- **No Ask AI.** The plan and the agent's conversation are side by side, so you ask in the message box.
- **The repo moves from mise to proto + moon** (step 0). This covers the repo's own tooling only; Dogsled's runtime handling of mise in other repos (trust checks, `remote_command`) is product behaviour and stays.

Run model (your choice): a long-running launchd service on a fixed localhost port.

**Assumed where you did not answer** (correct these in review):
- **Home:** `dogsled web`, a subcommand in this repo. It reuses `board::build`, `commands::status` and `board::cached_todos` directly. Todos are not in `dogsled board --json`.
- **Pi tools:** the Pi side of plan mode and `interview` lives in this repo's Pi extension (`pi/`, published as `@oisin-ee/pi-dogsled`), because it talks only to `dogsled`.
- **Rows:** the board shows all hosts' rows, grouped by a canonical repository ID that Dogsled computes. Rows from hosts that have not upgraded yet go into an "Unsorted" group.
- **Browser:** the portal opens in your default browser (`open`).
- **Timeout:** questions and reviews in the portal wait until you answer. The 20-minute `interview.timeout` applies only to the fallback form.

## Verified facts this plan depends on

- **Herdr 0.9.5 (protocol 22): CLI and socket.**
  - CLI: `herdr agent list|get|read|prompt|focus`, `herdr workspace list` and `herdr api snapshot`.
  - Event subscription is socket-only (`events.subscribe`).
  - `agent.prompt` returns `agent_blocked` without sending when the agent is blocked.
  - Events are not durable: subscribe first, then reconcile with an authoritative read.
  - Source: [socket-api.mdx](https://raw.githubusercontent.com/herdrdev/herdr/v0.9.5/docs/next/website/src/content/docs/socket-api.mdx).
- **Dogsled convention:** `src/herdr_socket.rs` carries only the methods that have no CLI form. Everything else goes through `Shell`.
- **Transcripts are already parsed.**
  - `src/activity.rs` reads bounded transcript tails for `pi`, `claude` and `codex`: `tail`, `latest` step, `reply`. Unknown formats fall back to pane text.
  - `watchdog::pane_tail` uses that path first, then `herdr pane read --source recent-unwrapped --format text`.
  - Probe agent panes carry the `transcript` path.
- **Repository identity today:**
  - `board::Row` has `group, kind, id, name, what, host, pane, state, activity, age_ms, quiet_ms, ahead, dirty`. There is no repository field.
  - Agent `cwd` and `worktree.path` are host paths.
  - `checkout::discover` (`src/checkout.rs:19`) already reads `git remote get-url origin`.
  - The canonical `host/owner/repo` form exists only in TypeScript, as `repositoryIdentity` (`pi/unit-protocol.ts:77`).
- **What the current Plannotator plan mode does** (`@plannotator/pi-extension` 0.28.19, the parts in use):
  - Toggle with `/plannotator-plan-mode` or `Ctrl+Alt+P`. The phases are `idle → planning`.
  - On entering planning, framing is delivered once as a conversation message. The system prompt is untouched, which keeps the provider cache valid.
  - A `tool_call` hook blocks `write`/`edit` outside `.md`/`.mdx` inside cwd.
  - `plannotator_submit_plan(filePath)`:
    - It is `executionMode: "sequential"`, so an edit in the same batch lands first.
    - It returns at once. The decision arrives later as a user message.
    - Resubmitting the same path updates the open review.
  - Reviewer actions: line annotations, approve, deny with feedback, and "Ask AI" questions answered in the session. (Ask AI is not carried over; see Context.)
  - Your `plannotator.json` sets `executionMode: "external"`. On approval, the extension emits `plannotator:plan-approved` `{cwd, planFilePath, planContent, feedback?}`, and the oisinpi `pi-todo` (`packages/pi-todo/src/plannotator.ts:79`) turns that into todo units. `plannotator_mark_done` is unused.
  - `pi-guardrails` README: "Planning restrictions belong to Plannotator."
- **pi-interview 0.13.0:**
  - The question types are `single | multi | text | image | info`, with options, `recommended`, `conviction`, `weight`, `content` and `media`.
  - `glimpseui` is not installed, so today the form opens in a browser tab (`open <url>`).
  - Your settings set only `interview.timeout: 1200`.
- **Crates** (crates.io, 2026-10-26):

  | Crate | Version | Licence |
  |---|---|---|
  | `axum` | 0.8.9 | MIT |
  | `dioxus` (feature `web`, `router`) | 0.7.10 | MIT OR Apache-2.0 |
  | `dioxus-cli` (`dx`) | 0.7.10 | MIT OR Apache-2.0 |
  | `rust-embed` | 8.13.0 | MIT |
  | `pulldown-cmark` | 0.13.5 | MIT |
  | `similar` | 3.2.1 | Apache-2.0 |

  - Dioxus documents the web target as its best-supported platform.
  - `dx bundle --web` writes a static `public/` folder (`index.html`, `assets/`, `wasm/`). Source: [Dioxus 0.7 bundling](https://raw.githubusercontent.com/DioxusLabs/docsite/main/docs-src/0.7/src/tutorial/bundle.md) and [web platform](https://raw.githubusercontent.com/DioxusLabs/docsite/main/docs-src/0.7/src/guides/platforms/web.md).
  - Dioxus fullstack also exists (on axum, with server functions), but `dx` builds its server binary. That conflicts with cargo-dist building `dogsled`, so this plan uses the web client only and keeps the axum server in `dogsled`.
- **Pi extension events** (Pi 1.4.0 `docs/extensions.md`): `pi.sendUserMessage()` starts a turn, which is how review decisions are delivered.
- **Blocked prompts:** `herdr agent read <target> --source visible --format text` returns the visible screen, and `herdr agent send-keys <target> <key> [key ...]` sends Herdr key-combo strings (`enter`, `esc`, arrows, `y`, `ctrl+c` and so on; see socket-api.mdx). Herdr does not expose a blocked prompt's options in structured form, so any buttons are inferred from screen text.
- **Tooling today and proto/moon** (checked 2026-10-26):
  - The repo uses `mise.toml` (node 24, lefthook; tasks `check:rust`, `check:pi`, `check`), `mise.lock`, and `lefthook.yml` (pre-commit runs `mise run check:rust` / `check:pi`). CI does not use mise: it runs `rustup toolchain install`, cargo and npm directly. Rust is pinned in `rust-toolchain.toml` (1.99.0).
  - Installed locally: proto 0.63.2, moon 2.6.1.
  - proto's `cargo:` backend (since v0.57) installs crates.io CLIs from `.prototools`, for example `"cargo:dioxus-cli" = "0.7.10"` ([tool spec](https://moonrepo.dev/docs/proto/tool-spec#cargo)).
  - moon's Rust toolchain uses rustup and runs cargo; it does not replace cargo ([Rust handbook](https://moonrepo.dev/docs/guides/rust/handbook)).
  - Your reference setup is `/Users/oisin/dev/momokaya-template`: `.prototools` pinning `moon`, `.moon/workspace.yml` with `vcs.hooks.pre-commit` running `moon run :#precommit --affected --status staged` (no lefthook), `.moon/toolchains.yml`, a root `moon.yml`, and CI using `moonrepo/setup-toolchain` then `proto install`.

## Approach

`dogsled web` is an axum server on `127.0.0.1:<port>`. It serves a Dioxus web app (Rust → WASM, built with `dx bundle --web` and embedded with `rust-embed`) and a JSON + SSE API. The portal state is the only new store: pending questions and reviews, kept under the Dogsled ledger so they survive a restart.

1. **Repository identity in Dogsled.**
   - Port `repositoryIdentity` to Rust as `checkout::identity(origin)`, with fixtures shared with the TypeScript tests.
   - The probe sets `Agent.repository`, cached per main checkout.
   - `board::Row` gets `repository: Option<String>`.
2. **Board.**
   - `GET /api/board` runs the sidebar read path (`commands::status` → `board::build(owner=None)` → `cached_todos`) and returns `repos: [{repository, rows, todos}]` in board order.
   - `GET /api/stream` (SSE) pushes changes. It rebuilds every 2s from cached ledger state, like the sidebar `REFRESH`, and never probes.
3. **Agent view (no terminal).**
   - Extend `activity.rs` with a bounded `conversation(agent, path, limit)` that returns user/assistant messages, with tool calls collapsed into one line each, for `pi`, `claude` and `codex`.
   - Unknown formats show `herdr agent read --source recent --format text` as plain text.
   - A socket subscription (`pane.agent_status_changed`, `pane.agent_detected`, `pane.exited`, `layout.updated`) triggers a re-read through `herdr agent list` / `workspace list`.
   - **Message box:** `herdr agent prompt`.
   - **Blocked agents** (state `blocked`, or `agent_blocked` from a prompt):
     - The portal shows the agent's visible prompt as plain text from `herdr agent read --source visible --format text`, refreshed on each status event.
     - Buttons offer likely keys inferred from that text: `y`/`n` for yes/no prompts, numbered or arrow choices for menus, plus `enter` and `esc` always. The inference is display-only and never sends anything by itself.
     - A short input sends typed text followed by `enter`.
     - Everything is sent through `herdr agent send-keys`.
     - **Open in Herdr** (`herdr agent focus`) stays as the fallback for anything the buttons do not cover.
4. **Questions (`interview`).**
   - **The Pi tool** keeps the same parameters as today. It posts `{questions, pane: $HERDR_PANE_ID, session, cwd}` to `POST /api/questions`, then waits on `GET /api/questions/:id/answer`, reconnecting if the connection drops. The tool's abort signal sends `DELETE`.
   - **The portal** renders the form natively in the agent's side panel and on a global "Needs you" list.
   - **The subset in v1:**
     - Question types: single, multi, text, info.
     - Recommended options, with conviction and weight.
     - Markdown and code `content`.
     - Media blocks other than images render as their caption plus source. Image upload is out.
   - **Portal down:** the tool runs `dogsled interview` (one-shot). It serves the same form from the same WASM bundle on `127.0.0.1:<random>` with a session token, runs `open <url>`, prints the answers as JSON and exits after 20 minutes or on submit.
5. **Plan mode and review.**
   - **Pi side** (`pi/plan-mode.ts`):
     - `/plan` command and `Ctrl+Alt+P` toggle the planning phase.
     - One framing message is sent when planning starts.
     - A `tool_call` gate limits `write`/`edit` to `.md`/`.mdx` inside cwd.
     - `submit_plan(filePath)` (sequential) posts `{path, content, pane, session, cwd}` to `POST /api/reviews`. Resubmitting the same path updates the review.
     - A background SSE subscription to that session's reviews delivers each decision with `pi.sendUserMessage`: approved, changes requested with annotations, or a reviewer question.
   - **On approval:** emit `dogsled:plan-approved` with the same payload pi-todo expects today, and leave planning.
   - **Portal side:**
     - The plan renders with `pulldown-cmark` (source offsets map each block to its plan line).
     - **Plan annotation**, matching the Plannotator popover you use (screenshot in review):
       - Selecting text in the rendered plan opens a popover headed by the quoted selection, with a comment box, an image button and **Save**. It can be expanded and closed.
       - Images can be pasted from the clipboard, dropped or picked. The portal saves each one under the Dogsled state directory (`web/attachments/<uuid>.png`, mode 0600) and shows a thumbnail on the annotation.
       - Saved annotations are listed beside the plan, highlighted in the text, and can be edited or deleted. They are drafts until sent, and are kept in the store so a reload or restart keeps them.
       - There is also a general comment for the whole plan.
     - **Asking about the plan:** the agent's conversation and message box sit beside the plan. The popover also has **Quote in message**, which puts the selection and its plan lines into the message box so the agent knows which passage you mean.
     - **Request changes** sends every draft annotation as one message: quoted text with plan line numbers, the comment, and the attached image paths in a "Reference images" list so the agent can read them.
     - Each revision shows what changed since the previous one (`similar`), and annotations on unchanged lines carry over.
     - The actions are **Approve** (with optional notes) and **Request changes**.
   - **Portal down:** `dogsled review` (one-shot) serves the same review view and runs `open`. The decision arrives the same way.
6. **Local safety.**
   - Bind `127.0.0.1` only.
   - Reject any non-loopback `Host`.
   - Require the portal `Origin` on every POST/DELETE.
   - The Pi tools authenticate with a per-user token in the Dogsled state directory (mode 0600).

**UI layout:**
- Left: repos, collapsible, with rows and state pills grouped like the sidebar (to land, attention, running, waiting), with host as a small label, plus a "Needs you" count.
- Centre: the selected agent's conversation and message box. When a review or form is open, it sits beside the conversation, so the plan and the agent are side by side.
- Right: that agent's todos and pending items, and the annotation list during a review.
- Colours come from Herdr's theme, the same source as `src/sidebar/palette.rs`.

## Files to modify

**Dogsled (this repo), step 0: tooling move to proto + moon:**
- New `.prototools`: pin `moon = "2.6.1"`, `node = "24"` (as today), and later `"cargo:dioxus-cli" = "0.7.10"` (step 2). Rust stays pinned in `rust-toolchain.toml`, which rustup and moon both read, so there is one Rust pin.
- New `.moon/workspace.yml`: `versionConstraint: "=2.6.1"`, one project at `.`, `vcs.defaultBranch: main`, and `vcs.hooks.pre-commit` running `moon run :#precommit --affected --status staged` (the momokaya-template pattern).
- New `.moon/toolchains.yml`: enable `rust` and `javascript`/`node` with `npm` (the repo already uses `package-lock.json`), and pin `proto`.
- New `moon.yml` with tasks that mirror the current mise tasks:
  - `fmt-check` (`cargo fmt --check`), `lint` (`cargo clippy --all-targets -- -D warnings`), `test` (`cargo test`), tagged `precommit` with Rust inputs.
  - `typecheck` (`npm run --silent typecheck`) and `test-shim` (`npm run --silent test`), tagged `precommit` with `pi/**` inputs.
  - `fmt` (`cargo fmt`, not cached, not in CI).
- Remove `mise.toml`, `mise.lock` and `lefthook.yml`. moon's VCS hooks replace lefthook.
- `.github/workflows/ci.yml`: use `moonrepo/setup-toolchain` (pinned by SHA, as momokaya-template does), `proto install`, then `moon ci` or `moon run` for the Rust and Pi jobs.
- `README.md`: the development section ("The pre-commit hook runs `mise run check`") describes `proto install` and `moon run :check`-style tasks instead. User install instructions and runtime mise behaviour are unchanged.

**Dogsled (this repo), portal:**
- `Cargo.toml`:
  - Make the repo a workspace with members `.`, `portal-types` and `web`.
  - Add `axum` 0.8, `rust-embed` 8 and `similar` 3.
  - Extend `tokio` features (`net`, `sync`, `process`, `io-util`, `time`, `macros`).
- New `portal-types/`: the serde DTOs shared by the server and the WASM client (board, agents, questions, reviews).
- New `web/`: the Dioxus 0.7 web crate (`dioxus` with `web` and `router` features, `pulldown-cmark`), with `Dioxus.toml`. `dx bundle --web` builds it, and the `public/` output is embedded.
- `rust-toolchain.toml`: add the `wasm32-unknown-unknown` target.
- `src/checkout.rs`: add `identity()`.
- `src/probe.rs`: add `Agent.repository`.
- `src/board.rs`: add `Row.repository`.
- `src/activity.rs`: add `conversation()`.
- `src/herdr_socket.rs`: add a streaming `subscribe`.
- `src/cli.rs`: add `web`, `interview` and `review`.
- `src/config.rs`: add a `[web] port` key.
- New `src/web/`:
  - `mod.rs` (server and guards)
  - `board.rs`
  - `agents.rs`
  - `questions.rs`
  - `reviews.rs`
  - `store.rs` (ledger-backed pending items)
- `pi/`:
  - New `interview.ts` and `plan-mode.ts`, plus tests.
  - Register both in `pi/index.ts`.
- `.prototools`: add `"cargo:dioxus-cli" = "0.7.10"`.
- `moon.yml`: add `web-lint` (`cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`) and `web-build` (`dx bundle --web`), with `web/**` and `portal-types/**` inputs. The `dogsled` build and test tasks depend on `web-build`.
- `.github/workflows/ci.yml`: `proto install` already provides `dx`; add the wasm target and run `web-build` before the cargo tasks.
- `dist-workspace.toml`:
  - Add a `github-build-setup` step that runs `dx bundle --web --release`.
  - `include` the new plist.
- `plugin/fetch.sh`: on the source-build path, require `dx` (with the same error style as `cargo`) and build `web/` first.
- New `plugin/services/dev.dogsled.web.plist`: same shape as `dev.dogsled.plist`, running `dogsled web`.
- `README.md`: a portal section.

**oisinpi (separate unit):**
- `packages/pi-todo/src/plannotator.ts`: subscribe to `dogsled:plan-approved` (rename the file to `plan-approved.ts`).
- `packages/pi-guardrails/README.md`: planning restrictions belong to Dogsled plan mode.

**Dotfiles (separate unit, after both releases):**
- `conf.d/harness/settings/pi/settings.json`: remove `npm:@plannotator/pi-extension@0.28.19` and `npm:pi-interview@0.13.0`, and keep `interview.timeout`.
- `conf.d/harness/mise.toml:319`, and the `settings/pi/plannotator.json` it copies: remove.
- Install and load `dev.dogsled.web.plist` the same way `dev.dogsled.plist` is installed.

## Reuse

- `commands::status`, `board::build`, `board::cached_todos`, `board::Group` and its order.
- The sidebar read path and `REFRESH` cadence (`src/sidebar/mod.rs`), and the Herdr theme colours (`src/sidebar/palette.rs`).
- Transcript parsing in `src/activity.rs` and the pane-text fallback in `watchdog::pane_tail`.
- `checkout::discover`, `prepare::main_checkout`, `worktree::common_dir`, and the `repositoryIdentity` rules and fixtures.
- The `Shell` / `System` abstraction, `herdr_socket::Client` framing, and the ledger (`src/ledger.rs`) for durable pending items.
- The existing Pi extension registration in `pi/index.ts` (`registerCommand`, tool patterns), and the pi-todo approval payload shape.
- The launchd plist pattern and the dotfiles installer for `dev.dogsled.plist`.

## Steps

Each step is one delivery unit. Steps 3 to 5 can run in parallel after step 2. Steps 7 and 8 come after the Dogsled release.

- [ ] **0. Move the repo to proto + moon.**
  - Add `.prototools`, `.moon/workspace.yml`, `.moon/toolchains.yml` and `moon.yml` with tasks that mirror today's mise tasks.
  - Switch pre-commit from lefthook to moon VCS hooks.
  - Switch CI to `setup-toolchain` + `proto install` + moon.
  - Remove `mise.toml`, `mise.lock` and `lefthook.yml`, and update the README development section.
  - Lands before step 1, so every later step uses moon tasks.
- [ ] **1. Repository identity.** Add `checkout::identity` with fixtures shared with the TypeScript tests, `Agent.repository` (cached per checkout), and `Row.repository`. `dogsled board --json` then shows it.
- [ ] **2. Workspace, server and board.**
  - Convert to a workspace and add `portal-types`.
  - Add the `dogsled web` skeleton with guards, `/api/board` and `/api/stream`.
  - Add the Dioxus app shell and the repo-grouped board with todos.
  - Wire up the build pipeline: `dx`, CI, dist and `fetch.sh`.
- [ ] **3. Agent view.** Add `activity::conversation`, the socket subscription, the agents API, the conversation UI and the message box. Add the blocked-prompt panel (visible text, inferred key buttons, short input, `send-keys`) with Open in Herdr as the fallback.
- [ ] **4. Questions.**
  - Add the questions API and store, and the native form.
  - Add the `pi/interview.ts` tool (same schema), with cancel.
  - Add the `dogsled interview` fallback.
- [ ] **5. Plan mode and review.**
  - Add the reviews API and store, including annotation drafts and image attachments.
  - Add the review UI beside the agent conversation: rendering, the annotation popover (quote, comment, image paste/drop/pick, Quote in message, Save), the annotation list, changes since the last revision, and approve / request changes.
  - Add `pi/plan-mode.ts` (toggle, framing, write gate, `submit_plan`, decision delivery, `dogsled:plan-approved`).
  - Add the `dogsled review` fallback.
- [ ] **6. Service and release.** Add the plist and dist include, then release Dogsled and `@oisin-ee/pi-dogsled`.
- [ ] **7. oisinpi.** Move pi-todo to `dogsled:plan-approved`, update the pi-guardrails docs, then release.
- [ ] **8. Dotfiles.** Remove Plannotator and pi-interview, install the plist, and bump the pinned versions.

## Verification

- **Step 0:** after `proto install`, `moon run :fmt-check :lint :test :typecheck :test-shim` passes, a staged `.rs` change triggers the Rust precommit tasks through the moon hook, and CI is green with no mise or lefthook left in the repo (`rg -n 'mise run|lefthook' -g '!CHANGELOG.md'` finds only runtime docs).
- **Checks:** all moon tasks pass, including `web-lint` and `web-build`.
- **Identity:** the Rust and TypeScript identity fixtures agree. `dogsled board --all --json | jq '.rows[].repository'` shows `github.com/oisin-ee/rondo` for rondo rows, including linked worktrees.
- **Board:** every local row sits under the same repo as its Herdr workspace's `repo_key`. Row order inside each repo matches `dogsled board`.
- **Agent view:**
  - A working Pi, Claude and Codex agent each show their latest messages, updating as they work.
  - An unknown agent shows its recent pane text.
  - A message reaches the exact pane.
  - A blocked agent (for example a Claude permission prompt) shows its prompt text. A `y` or `enter` button answers it, and the agent continues.
  - The short input sends text plus `enter`.
  - Open in Herdr focuses the pane.
- **Interview:**
  - From a Pi pane, an `interview` call appears under that agent. Submitting returns the answers to the tool call, and no browser tab opens.
  - Cancelling the turn removes the question.
  - With the service stopped, a browser tab opens and its submission returns the same way.
- **Plan mode:**
  - With `/plan` on, writing a `.rs` file is blocked and writing `plans/x.md` is allowed.
  - `submit_plan` shows the plan in the portal.
  - Selecting text opens the popover. A saved annotation with a pasted screenshot survives a page reload and a service restart.
  - Request changes delivers the annotations as one message with quoted text, line numbers and image paths, and the agent can read each image.
  - Resubmitting shows what changed, and annotations on unchanged lines carry over.
  - Quote in message fills the message box with the selection and its lines. The message reaches the agent, and its answer appears in the conversation beside the plan.
  - Approve leaves planning, and pi-todo creates units from `dogsled:plan-approved`.
  - With the service stopped, `dogsled review` opens a tab and the flow completes.
- **Safety:** a foreign `Host` gets 403. A POST with a foreign `Origin`, or a tool call without the token, is refused.

## Unknowns to settle in the steps

- **Step 3:** how far back `activity::tail`'s bounded read goes, and whether the conversation view needs its own larger bound.
- **Step 5:** whether Pi lets an extension hold a background SSE connection across turns and deliver later with `pi.sendUserMessage`. Plannotator does this with its in-process server, so the API exists. Check the Pi extension docs before step 5.
- **Step 3:** how well key buttons can be inferred from Claude, Codex and Pi prompt screens. Collect a few real blocked screens first; if inference is unreliable, show only `enter`, `esc`, arrows and the text input.
- **Step 0:** whether moon's `rust` toolchain should own `rust-toolchain.toml` sync, or just use it. The plan keeps `rust-toolchain.toml` as the only Rust pin.
- **Step 2:** check that `dx bundle --web` output paths are stable enough for `rust-embed`, or set the output directory in `Dioxus.toml`.
