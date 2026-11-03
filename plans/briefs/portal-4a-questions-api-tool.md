# Brief: questions store, API and the Pi `interview` tool (web portal step 4a)

**Outcome.** A Pi agent calling `interview` posts its questions to `dogsled web` and gets the answers back as the tool result once someone answers through the API; cancelling the turn withdraws the question. The browser form is step 4b.

**Design.** Plan `plans/web-portal.md`, Approach 4. Keep the tool name `interview` and pi-interview 0.13.0's parameter schema (installed at `~/.pi/agent/npm/node_modules/pi-interview/`, read `index.ts` and `README.md` there for the exact `questions` JSON shape: `{title?, description?, questions:[{id,type:single|multi|text|image|info,question,options?,recommended?,conviction?,weight?,context?,content?,media?}]}`). Server skeleton, guards and the token file `<state>/web/token` come from step 2a (`src/web/mod.rs`). State dir: `DOGSLED_STATE`, else `$XDG_STATE_HOME/dogsled`, else `~/.local/state/dogsled` (`src/ledger.rs:40`). Store pending items with the ledger (`Ledger::child("web")`, `read`/`write`/`with_lock`) so they survive a restart. Pi extension API: `/Users/oisin/.local/share/mise/installs/pi/1.4.0/pi/docs/extensions.md` (`pi.registerTool`, `ctx.signal`, `session_shutdown`); existing registration in `pi/index.ts`.

**Edits.**
- New `portal-types/src/questions.rs` (re-export from lib): `QuestionSet` (the schema above, unknown fields kept as `serde_json::Value`), `Pending { id, pane, session, cwd, created_ms, questions }`, `Answer { id, answers: Value }`.
- New `src/web/store.rs`: ledger-backed `Store` for pending questions (later reused by reviews): create, list, get, answer, delete; answering wakes waiters (`tokio::sync::watch` or `Notify` keyed by id).
- New `src/web/questions.rs`: `POST /api/questions` (Bearer token required; body `{questions, pane, session, cwd}` → `{id}`), `GET /api/questions` (list for the UI), `GET /api/questions/:id/answer` (token; long-poll up to 60 s, 204 on timeout, 200 with answers, 410 if deleted), `POST /api/questions/:id/answer` (portal Origin; from the UI), `DELETE /api/questions/:id` (token). Token requests are exempt from the Origin rule; requests with neither a valid token nor the portal Origin get 403.
- `src/web/mod.rs`: route registration only.
- New `pi/interview.ts` + `pi/interview.test.ts`: registers `interview` with the same parameters and description semantics as pi-interview; reads the token file; posts with `$HERDR_PANE_ID`, session id and cwd; loops on the long-poll until answered; `ctx.signal` abort sends `DELETE`. When the portal is unreachable it returns a tool error saying "dogsled web is not running" (the browser fallback is step 4b). Port from `[web] port` default 7465, overridable by `DOGSLED_WEB_PORT`.
- `pi/index.ts`: register the tool.

**Owned paths.** `portal-types/src/questions.rs`, `portal-types/src/lib.rs` (one line), `src/web/store.rs`, `src/web/questions.rs`, `src/web/mod.rs` (routes), `pi/interview.ts`, `pi/interview.test.ts`, `pi/index.ts` (registration).

**Acceptance.** `cargo test --workspace` and `npm run --silent test` pass, including: a Rust test that posts a question with the token, answers it via the Origin route and receives the answer on the long-poll; 403 without token or Origin; a restart test (new `Store` over the same dir still lists the pending item); a TS test with a fake HTTP server showing the tool returns the answers and sends DELETE on abort.

**Checks.** `moon run :fmt-check :lint :test :typecheck :test-shim` (or the cargo/npm equivalents if moon is not on the base).

**Base.** origin/main after step 2a lands (the lead fills the SHA at dispatch).
