# Implement live catalogue recovery

## Outcome
An open New conversation receives catalogue failure and recovery through the existing portal stream, without losing user input or silently changing its target.

## Cause and approved design
`web/src/new_lead.rs:86` fetches once with `use_resource(api::fetch_repos)`; `PortalSnapshot` omits the catalogue.
The user approved `plans/catalogue-recovery.md` in review `f085da93e6777084126562623044e474`; read it before editing.
Use one existing stream, preserve hourly producer attempts, and keep refresh errors separate from observation errors.
Introduce portal snapshot version 2, but retain observation IPC version 1 and strict shape decoding.

## Edits
- `portal-types/src/lib.rs`, `portal-types/src/tests.rs`: shared portal version constant, `repos: Option<RepoList>`, and version/shape coverage.
- `src/observation/project.rs`: decode the local catalogue once and share it between the existing view and snapshot. Missing is None; malformed present data is a scoped gap, not an empty success.
- `src/observation/tests/local.rs`, `src/observation/tests/projection.rs`, `src/observation/tests.rs`: controlled-time good/failure/recovery, unchanged call counts, catalogue-only publication changes, missing/malformed input, and no peer-triggered fetch.
- `src/observation/details_fixture.rs`, `src/web/test_support.rs`: update snapshot literals. Find other owned literals with `rg -n -F 'PortalSnapshot {' src web portal-types`.
- `web/src/app.rs`: reducer-owned retained catalogue and observation error; equality-based memo/context, not another subscription.
- `web/src/api.rs`: consume the shared portal version; remove unused `fetch_repos` after checking callers. Keep the server endpoint.
- `web/src/new_lead.rs`: consume shared state; keep picker/composer mounted and slug keys stable; remove duplicate prefix; distinguish failed/empty/unknown states.
- `web/src/component_tests.rs`: mounted failure-to-recovery tests, stable scopes, draft/attachment/search/selection preservation, removed selection, and unrelated snapshots.
- `tests/cli/observation_portal.rs`: real IPC/SSE regressions across multiple clients, one subscription, no reader commands, unchanged REST/CLI output. Browser fixture control, if needed, stays here, never in production.
- `README.md`: update only the catalogue description for live recovery and coordinated portal-version adoption.
Initial default selection happens once. Preserve a valid selection. If a healthy list removes it, show its slug and require explicit reselection before Send. Never retarget an in-flight request.

## Paths and generators
Owned paths are exactly the files in Edits. Read the approved plan, source, tests, repository configuration, and applicable coding/UI guidance.
Read-only: catalogue scheduling, observation IPC, HTTP routing, authentication, third-party components, version pins, and service configuration.
Generated output: `web/public` only through `moon run dogsled:web-build` or mandatory hook dependencies; do not edit generated assets or commit ignored output.
Stop with evidence before changing another owner or patching a dependency. No retries, remounts, polling, or compatibility fallback to conceal a defect.

## Acceptance
In the repository root, `cargo test --locked -p portal-types`, `cargo test --locked -p dogsled catalogue`, `cargo test --locked -p dogsled observation::tests`, `cargo test --locked -p dogsled --test cli observation::portal::`, and `cargo test --locked -p web` must pass with nonzero tests.
First show that the focused recovery regression detects the old behavior. Report the exact tested revision and distinguish the known browser reproduction from new automated evidence.
The lead owns final managed-browser verification and `moon run dogsled:fmt-check dogsled:web-lint dogsled:web-build dogsled:lint dogsled:test` on the integrated revision.

## Checks and authority
Allowed light checks: `cargo fmt --all --check`, `cargo clippy --locked -p web --target wasm32-unknown-unknown -- -D warnings`, and the focused acceptance commands.
Do not independently run the full repository gate, install tools, dispatch hosted CI, or run production browsers/services. Mandatory commit hooks remain required, including their normal dependencies.
Commit the coherent result and push only the assigned `fix/catalogue-recovery` branch. Do not open a PR, merge, release, deploy, or update local versions.
Return branch and full head SHA, changed paths, red/green regression evidence, commands/results/revisions, and any blocker or browser scope still unverified.

## Base
Reviewed implementation source base: `f443a7de94011a4c9f8c38050955a6decc9a78d6` (`origin/main`).
The worker is pinned to the local host and reads this committed brief and plan from `/Users/oisin/dev/dogsled/plans/`; its code branch starts at the implementation source base.
Compared with the plan's source base, the only change in the inspected owner/read-only path set is an unrelated timing-module declaration in `src/observation.rs`.
