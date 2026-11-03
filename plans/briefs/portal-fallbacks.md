# Brief: `dogsled interview` and `dogsled review` fallbacks (web portal steps 4 and 5)

**Outcome.** When `dogsled web` is not running:
- The Pi `interview` tool still works. It runs `dogsled interview`, which opens the form in a browser tab and returns the answers.
- `submit_plan` still works. It runs `dogsled review`, which opens the review in a tab and delivers the decision as a message.

**Design.**
- Both commands serve the same router, bundle and stores as `dogsled web` (`src/web/mod.rs`, `serve` at line 102 and `router` at line 183). They differ in three ways:
  - They bind `127.0.0.1:0`, a random port.
  - They print the URL to stderr.
  - They open it with `open` on macOS and `xdg-open` on Linux, run through the `Shell` abstraction (`src/shell.rs`).
- Factor out the listener setup that `serve` and the new commands share. Do not copy it.
- **The Host and Origin guards must accept the random port.** Build the guard from the port the listener actually bound.
- **The UI deep links,** added in parallel by the review UI unit:
  - `/?question=<id>&pane=<pane>` for a question.
  - `/?review=<id>` for a review.

**Edits.**
- **`src/cli.rs` and `src/main.rs`:**
  - `dogsled interview --file <path> [--pane P] [--session S] [--cwd C] [--timeout <secs>, default 1200]`.
  - `dogsled review --file <plan.md> --pane P --session S --cwd C`.
- **New `src/web/oneshot.rs`:**
  - **interview:**
    1. Create the pending question in the store, the same as `POST /api/questions` does.
    2. Open the deep link.
    3. Wait for the answer. On an answer, print `{id, responses}` as JSON on stdout and exit 0.
    4. On timeout, withdraw the question and exit 2.
  - **review:**
    1. Submit the review, the same as `POST /api/reviews` does.
    2. Open the deep link.
    3. Wait for the decision. Then print the `DecisionEvent` JSON on stdout, ack it, and exit 0.
- **`src/web/mod.rs`:** the shared listener setup, and the module declaration.
- **`pi/interview.ts`:** when the portal is unreachable, save the parsed questions to a temp file and run `dogsled interview --file … --pane … --session … --cwd … --timeout <interview.timeout>`. Then parse stdout with the existing `parseAnswers`. An abort kills the child process.
- **`pi/plan-mode.ts`:** when `POST /api/reviews` cannot reach the portal, run `dogsled review …` in the background. When it prints the `DecisionEvent`, deliver it through the same code path as an SSE decision: the message, then `dogsled:plan-approved` on approval. Return the usual "end your turn" result at once.
- **Tests:**
  - The guard accepts the bound random port.
  - `oneshot` interview returns the answer posted through the router.
  - `oneshot` review returns the decision.
  - The TS fallbacks use a fake `dogsled` executable on PATH.

**Owned paths:** src/cli.rs, src/main.rs, src/web/mod.rs, src/web/guard.rs, src/web/oneshot.rs, pi/interview.ts, pi/interview.test.ts, pi/plan-mode.ts, pi/plan-mode.test.ts.

**Acceptance.**
1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and the tests for the touched modules pass.
2. `npm run --silent typecheck` and `npm run --silent test` pass.
3. CI on the PR passes: rust on macOS and Ubuntu, and typescript.
