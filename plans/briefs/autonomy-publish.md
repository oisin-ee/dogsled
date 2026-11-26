# Autonomy resolution, publication, routes, and prompt context

Plan: `plans/autonomous-toggle.md` (approved), sections "Which agents are autonomous" and "Settled after approval".
Needs the store unit merged (`plans/briefs/autonomy-store.md`): `src/autonomy.rs` with `fleet`, `agents`, `decisions`, `set_fleet`, `set_agent`.

## Outcome

Every host resolves which of its agents are autonomous, the portal snapshot shows it, the portal can flip both switches and read a lead's decisions, and an autonomous agent reads the rule in its prompt context.

## Design

Each host adds an `autonomy` extension to its `host` domain, as `owned_work` does. Older hosts ignore it. A new exported domain is not possible: `Export::validate` (`src/observation/peers.rs:192`) rejects unknown domains.

## Edits

1. `src/observation/owned_work.rs:130` (`serialize_host`): add `value["autonomy"] = {"version":1,"fleet":bool,"agents":[pane],"decisions":[DecisionView]}`.
   `agents` lists only panes whose stored session is the pane's live session (`details::Session::of`, used at :92). A read failure writes `{"version":1,"status":"unavailable"}`.
2. `src/observation/peers.rs:446` (`Ownership`): keep each peer's decoded extension beside `evidence`, with the same rule: a stale peer keeps its last usable value. A peer without the extension is "unsupported".
3. `src/autonomy.rs`: a pure `resolve` function. Inputs: each host's extension and each host's `owned_work::Execution` list (`owner_hostname`, `owner_pane`, `pane`).
   Output: the cause of each (hostname, pane). Order: `Fleet` when any host has `fleet`, then `Agent`, then `Owner` when the owner resolves to any cause. Guard against an ownership cycle.
4. `src/observation/project.rs`:
   - `fleet_agents` (:570) sets `AgentView.autonomous` and `AgentView.decisions` for local and remote agents. The mark counts the agent's own decisions and those whose `owner_host` and `owner_pane` name it. `HostView.hostname` maps a hostname to a label.
   - `portal` (:689) sets `PortalSnapshot.autonomy`: `fleet` holds the labels of the hosts with the switch on, `unsupported` the labels of reachable hosts without the extension. `None` when the local extension is unavailable.
   - Insert a view `autonomy`: `{"panes":{"<local pane>":"fleet|agent|owner"},"decisions":[DecisionView]}`. `decisions` holds every host's decisions, with `host` rewritten to the receiver's label.
5. `src/autonomy.rs`: `cause_of(ledger, pane) -> Result<Option<AutonomyCause>>` reads that view through `observation::ipc::Client`, as `hook` does (`src/main.rs:667`). This is the one function that answers "is this pane autonomous".
6. `src/context.rs`: `select()` (:36) adds the view. `text()` (:72) appends this block when the caller's pane has a cause:
   "Autonomous mode is on. Your owner is away. Do not ask your owner a question and do not wait for an approval. For each open question, choose the most reasonable option from the plan, the request, and the code. Record each choice with the decide tool before you act on it. Stop only when you need something that only your owner has: credentials, access, or payment. Then end the turn and say what you need."
7. `src/web/autonomy.rs` (new, mounted in `src/web/mod.rs` beside `settings`), in the style of `src/web/settings.rs`:
   - `POST /api/autonomy/fleet` with `SwitchBody`. On: `set_fleet` locally. Off: clear it on every host that publishes it on.
   - `POST /api/agents/{host}/{pane}/autonomy` with `SwitchBody`. Refuse with 409 when the pane has an owner (a launch with an owner, or an `Execution` for it).
   - `GET /api/agents/{host}/{pane}/decisions` returns a `DecisionList` from the published view: the agent's own decisions and those it owns, newest first. `gaps` names each host that is unreachable or unsupported.
   - Both POST routes use the Origin guard of `settings::change`. A remote host goes through new `PeerOperation` variants (`src/web/agents.rs:72`). An older peer answers "unknown variant"; report 502 "host X: autonomous unsupported", as files does (`src/web/agents.rs:420`).
   - After a local write, call `observation::wake::notify(ledger)` (`src/observation/wake.rs:23`) so that the service publishes again.

## Paths

Own: `src/`, `tests/`. Read: `portal-types/`, `plans/`. Do not change `portal-types/`, `web/`, or `pi/`.

## Acceptance

`cargo test autonomy` passes, with these tests:

- `resolve`: fleet on one host makes every pane `Fleet`; an agent switch gives `Agent`; a worker on host B whose owner on host A is switched on gives `Owner`; a cycle ends.
- A stale peer keeps its last extension; a peer without it is unsupported.
- The projection of a fixture with one switched-on lead and one decision of its worker sets `autonomous`, the lead's mark `{count: 1}`, and the view.
- The decisions route returns that decision for the lead; the agent switch route answers 409 for the worker.

## Checks

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test autonomy`, `cargo test owned_work`, and `cargo test context::`. Run no other test and no other build.

The root crate's tests embed the portal bundle (`src/web/assets.rs:53`, `moon.yml` task `test`). Run `moon run :web-build` once in the worktree before the first `cargo test`. `web/public/` is ignored by Git: do not commit it and do not copy it from another checkout.
