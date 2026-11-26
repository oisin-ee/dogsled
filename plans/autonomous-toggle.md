# Autonomous toggle and decision log

Status: approved on 2026-11-27 (portal review `a5974878`, revision 1, no notes). Implementation is in progress.
The owner answered the product questions on 2026-11-27. Their answers are in "Owner decisions".
Revision 2 records what the code settled after approval: see "Settled after approval". No owner decision changed.

## Outcome

The owner switches an agent, or the whole fleet, to Autonomous in the portal.
An autonomous agent does not ask the owner a question and does not wait for an approval.
It chooses the most reasonable option from the plan, the request, and the code.
It records each such choice with `dogsled decide`: the question, the decision, and the reason.
The owner reads the recorded decisions in a Decisions tab on the lead's page.

## Owner decisions

1. There are two switches. One in the top bar covers every agent on every host. One in a lead's header covers that lead and its workers.
2. An autonomous agent stops only when it needs something that only the owner has: credentials, access, or payment.
3. A destructive or irreversible action does not stop an autonomous agent. The agent records it as a decision.
4. A submitted plan does not wait for approval while its agent is autonomous. Dogsled approves it and records that as a decision.
5. When a switch goes on, requests that already wait in the inbox are released. The agent decides and records.
6. The right column becomes 400px wide (it is 300px). The session thread uses the full width of its column, with less side padding.
7. The first iteration is a manual switch. Dogsled does not detect that the owner is away.

Points 3 and 4 remove two safety stops. With the fleet switch on, every agent can delete, force-push, or publish without a question.
The Decisions tab is then the only record. Say so in review if either point must change.

## Wireframe

- Editable canvas: [autonomous-toggle.squig.json](autonomous-toggle.squig.json).
- Render: `.impeccable/mocks/autonomous-toggle/wireframe.png`.

The canvas has one feature, grouped by screen size:

- Desktop, 1600 × 1000: the lead page with the lead switch on and the Decisions tab open.
- Desktop states: the Decisions tab when empty (switch off, switch on) and when a host does not answer.
- Desktop states: the six states of the two switches.
- Compact, 1280 × 800: the same page below 1400px, where Decisions is the fifth header tab.

I drew the canvas with the Squig file tools on `netcup` and checked the render in the isolated browser.
No editor session is open. To edit it, run `pnpm squig serve <path>` in a Squig checkout on your computer.

## Behavior

### Which agents are autonomous

An agent is autonomous when one of these is true, checked in this order:

1. The fleet switch is on.
2. The agent's own switch is on.
3. The agent's owner is autonomous. A worker follows its lead, also across hosts.

The fleet switch wins. While it is on, a lead's switch shows on and is locked.
A worker's header shows the state of its lead and is read only.
Only an agent with no owner has an interactive switch.

### What an autonomous agent does

| The agent wants to | Today | Autonomous |
| --- | --- | --- |
| Ask a question (`interview`) | A form waits in the inbox | No form. The tool tells the agent to decide and record. |
| Get a plan approved (`submit_plan`) | The review waits in the inbox | Dogsled approves it at once and records a decision that links the plan. |
| Do something destructive | Confirms with `interview` | Same as a question: it decides and records. |
| Get credentials, access, or payment | Ends the turn and says so | Unchanged. It ends the turn and names what it needs. |
| Ask its lead (`end_turn` kind `ask`, worker only) | The lead answers | Unchanged. The lead is an agent, not the owner. |

### When a switch changes

- On: each covered agent that is working, or that has a waiting question or plan review, gets one Dogsled inbox message.
  The message says that Autonomous is on and what the rule is. Waiting questions and plan reviews are released as in the table.
- On: an idle agent with nothing waiting gets no message. It reads the rule in its prompt context on its next turn.
- Off: each covered agent that is working gets one message that says to ask the owner again. Nothing else changes.
- A worker on another host follows its lead after the next peer exchange. The peer cache is 10 seconds (`peers::CACHE_MS`).

### The decision record

`dogsled decide` and the MCP tool `decide` take the same fields:

| Field | Required | Meaning |
| --- | --- | --- |
| `title` | yes | One line that names the decision. |
| `decision` | yes | What the agent chose. |
| `why` | yes | The reason, with the facts it used. |
| `question` | no | The open question that it settled. |
| `alternatives` | no | The options it did not choose. |

The title is one line of at most 200 bytes. Each other field has at most 2000 bytes.
Dogsled adds the time, the host, the pane, the harness session, the agent name, the owner pane and owner host, and the source.
The source is `question` (after a refused or released form), `plan` (Dogsled approved a plan), or `agent` (the agent recorded it unprompted).
`decide` works whether or not Autonomous is on. `dogsled decisions [--pane <pane>] [--json]` prints the records.

### Limits of the first iteration

- Permission prompts of a harness still wait. So do the native question tools of Claude Code and Codex. Dogsled does not own them.
- An agent can still end its turn with a question in plain text. Only the prompt rule prevents that.
- If the host that holds the fleet switch is unreachable, other hosts keep the last value they received.
  A host whose service restarts in that time asks again until contact returns.
- A host on an older release ignores both switches. The portal names that host when a switch does not reach it.

## Design

### Owner of the state

Each host's `state.db` owns the switches and the decisions of its own agents. Schema version 8 becomes 9 (`src/store.rs`).

- `autonomy_fleet`: at most one row. It exists while this host holds the fleet switch on.
- `autonomy_agents`: one row per switched-on agent, keyed by pane and harness session, as todo snapshots are.
- `autonomy_decisions`: one row per decision, with the fields above. Each host keeps its newest 100 and drops older rows.
- `web_questions.state` gains `released`.

A new module `src/autonomy.rs` owns reads, writes, and the rule in "Which agents are autonomous".

### Publication

Each host adds a versioned `autonomy` extension to its `host` domain, as `owned_work` does (`src/observation/owned_work.rs:130`).
The extension holds the fleet switch, the switched-on panes whose session is still live, and the host's decisions.
The projection (`src/observation/project.rs`) resolves each pane to `none`, `fleet`, `agent`, or `owner`.
It reads the local tables, the peers' extensions, and the ownership evidence that hosts already exchange (`owned_work::Execution`).
The fleet switch is stored on the host whose portal took the click. Every host treats it as on when any host publishes it on.
Switching it off clears it on each host that publishes it on.

One function answers "is this pane autonomous": prompt context, the question route, and the review route all call it.

### Where the rule is enforced

| Point | File | Change |
| --- | --- | --- |
| Prompt context for Pi, Claude Code, Codex | `src/context.rs` | Add an Autonomous block when the caller's pane is autonomous. |
| Question creation | `src/web/questions.rs` | Refuse with `409` and code `autonomous`. |
| Waiting questions at switch-on | `src/web/questions.rs`, `src/web/store.rs` | Mark `released` and wake the long-poll. |
| Plan submission and waiting reviews | `src/web/reviews.rs` | Store the review, approve it with the note "Approved by autonomous mode", record a `plan` decision. |
| Pi `interview` tool | `pi/interview.ts` | Return a normal result for `autonomous` and `released`, with the instruction text. Not an error. |
| Change of a pane's state | `src/autonomy.rs`, called from serve | Send the inbox message described in "When a switch changes". |

The plan review path needs no Pi change: an approval already reaches the session through the decision stream (`pi/plan-mode.ts`).
The word "decision" already names a review's approval there. New code and routes use `autonomy` and `decide` to keep the two apart.

### Commands and tools

- `dogsled decide` and `dogsled decisions` in `src/cli.rs`.
- MCP tool `decide` in `src/mcp.rs`, exposed directly in Pi (`pi/index.ts` keeps the default exposure).
- The command reference in `plugins/dogsled/skills/dogsled/references/commands.md` and the README gain both.

### Portal API and wire types

- `portal-types/src/autonomy.rs` holds the types. The snapshot gains `autonomy` (the fleet switch and the hosts without support).
  `AgentView` gains `autonomous` (the resolved cause) and `decisions` (a count and the newest time).
- `GET /api/agents/{host}/{pane}/decisions` returns a `DecisionList`: the agent's own decisions and those of the agents it owns.
  The portal server answers from its own publication. It starts no SSH command and no stream.
  The portal reads the list again when the agent's `decisions` mark changes.
- `POST /api/autonomy/fleet` and `POST /api/agents/{host}/{pane}/autonomy`, each with `{ "on": bool }`, behind the Origin guard.
- A remote agent's switch goes through a new `PeerOperation`, as model settings do (`src/web/settings.rs`).
- An older peer answers "unknown variant". The portal reports "host X: autonomous unsupported", as it does for files.

### Portal UI

- Add the official Switch with `dx components add switch` (the rule in `plans/dioxus-components.md`).
- Top bar (`web/src/app.rs`): the fleet switch after Metrics.
- Lead header (`web/src/agent_view.rs`): the agent switch before the icon buttons.
- Agents sidebar: an `Auto` badge on each autonomous lead.
- Right column: a fourth tab, `Decisions n`, after Files. Below 1400px it is the fifth header tab.
- Decisions panel (new `web/src/decisions.rs`): accordion rows, newest first, the lead's own and its workers'.
  A closed row shows the title, then time, who, and source. An open row shows Question, Decision, Why, and Other options.
  A `plan` row links to the stored review.
- Conversation: a `decide` call renders as a "Decision recorded" event card (`web/src/conversation_events.rs`).
- `DESIGN.md`: update the Right column and Layout sections.

Layout changes, from owner decision 6:

- Right column: 300px becomes 400px (`console.css:589`).
- Thread: `.conversation-turns` and `.conversation-footer` lose the centred 704px limit. Side padding goes from 28px to 16px.

I measured the tab row with the portal's font and padding in the isolated browser.
Four tabs need 266px with one-digit counts and 302px with two-digit counts. The 300px column has 275px for them.
At 400px the row fits, and the "N working" text beside the tabs still fits.

## Alternatives

| Choice | Selected | Rejected, and why |
| --- | --- | --- |
| Who enforces the rule | The host's service refuses or releases the request | A prompt rule alone: an agent that asks anyway would still block. |
| How a worker learns its lead's state | Each host resolves it from published domains | Push at switch time and at spawn: two more paths that can miss a worker. |
| Where the fleet switch lives | One host stores it; every host reads it | Write to every host: a partial write leaves hosts that disagree. |
| How the portal reads decisions | Hosts exchange them; the browser reads one lead's list on demand | In every snapshot frame: each frame would carry every decision of the fleet. One request per worker host: an SSH command for each read. |
| Where a decision is attributed | Owner pane and host stored with the decision | Resolve from the board: a closed worker's row leaves the board. |

## Units

Unit 1 fixes the interface. Units 2 and 4 can then run at the same time, because they own different files.
Unit 2 of the reviewed plan is now units 2 and 3, so that each fits one session. Unit 3 needs unit 2 merged.

1. **Wire types.** `portal-types`: the autonomy and decision types, with tests. Owner: lead.
2. **State.** `src/`: store and schema 9, `src/autonomy.rs`, the `host` extension and projection, the switch and decisions routes, the prompt context block, `decide` and `decisions`, command docs. Owner: writer.
3. **Enforcement.** `src/` and `pi/`: refused and released questions, approved plans with their decision, the Pi `interview` result, the switch-change messages. Owner: writer.
4. **Portal.** `web/` and `DESIGN.md`: Switch, the two switches, the `Auto` badge, the Decisions tab and panel, the event card, the 400px column, the full-width thread. Owner: writer.

## Checks

Cheap checks only, on `netcup`. The full suite and hosted CI run before the release.

- Unit 1: `cargo test -p portal-types`.
- Unit 2: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test autonomy`, and the Pi `interview` tests.
  The tests cover: the three causes and the fleet-wins order; a refused question; a released question; an approved plan with its decision; a stored and projected `decide`.
- Unit 3: `moon run web-lint`, the component tests of the Decisions panel, and a browser check at 1600, 1399, 1100, and 800px.
- Real run before landing: switch a real lead on in the portal, have it call `interview`, and see no inbox item and one decision in the tab.

## Not in the first iteration

- Automatic Autonomous when the owner is away.
- A command-line switch.
- Unseen markers or notifications for new decisions.
- Overriding or reverting a decision from the portal.
- One page for the decisions of every lead.

## Evidence

Read in `origin/main` at `78346fd`:

- One formatter builds the prompt context of every harness: `src/context.rs`, called by `dogsled hook context` (`src/main.rs:666`, `pi/index.ts:374`).
- Pi owns `interview` and `submit_plan` (`pi/interview.ts:520`, `pi/plan-mode.ts:291`). Both call the host's service.
- A lead cannot signal `ask` or `blocked` (`src/leads.rs:390`). A child's signal goes to its owner (`src/done.rs:497`).
- Items record `owner_pane`; launches record `owner_host` (`src/items.rs`, `src/work.rs:82`).
- Peers exchange only host-owned domains (`src/observation/peers.rs:105`). A stale peer source exists (`peers_runtime.rs:197`).
- The right column holds Todos, Workers, and Files (`web/src/agent_view.rs:1169`). The top bar holds New and Metrics (`web/src/app.rs:663`).
- The official catalogue has `switch` ([DioxusLabs/components](https://github.com/DioxusLabs/components/tree/main/preview/src/components)).

## Settled after approval

Read in `origin/main` at `0951ae6`:

- A new exported domain would break older hosts. `Export::validate` rejects an export whose `local` holds a domain it does not know (`src/observation/peers.rs:192`).
  So the data travels inside the `host` domain, in a versioned extension that older hosts ignore, as `owned_work` does.
  Decisions travel with it, so the bounds are 100 decisions for each host and 2000 bytes for each text field.
- A host keeps a peer's last usable evidence while the peer is stale (`Ownership::replace`, `src/observation/peers.rs:446`). The copy is in memory.
  After a service restart, the host has none until the peer answers. This is the limit that "Limits of the first iteration" already names.
- A launch from before `owner_host` has no resolvable owner (`owned_work::agent_executions` skips it). Such a worker follows only the fleet switch and its own state.
- The snapshot type refuses unknown fields, and the browser bundle ships inside the same binary (`src/web/assets.rs`).
  The new fields are optional and absent when empty, as `dogsled` is. `PORTAL_VERSION` stays 2.
- The owner approved a portal speed plan on the same day (`plans/portal-snappy.md`). It removes polls, extra streams, and SSH reads.
  This feature adds none of them: switches and decisions arrive in the existing publication and one on-demand read.

Settled in review of the units (merged in #451, #452, and #454):

- Only a host that publishes the extension enforces autonomy. A pane on an older host gets no cause, whatever the switches say.
  The portal names those hosts beside the fleet switch. A switch request for a pane there answers "host X: autonomous unsupported".
- When a host does not answer, the Decisions tab shows the decisions that the other hosts returned, with a notice that names the host.
  The wireframe's "Unavailable" state applies only when no decision could be read.
- A `plan` decision opens its approved plan in a new browser tab, in a read-only view of the stored review.
- The host meters in the top bar scroll inside the bar at any width where they do not fit. The rule applied only below 1150px before.
- A store write already wakes the service (`src/store.rs`), so the switch routes send no wake of their own.
- The writer profile cannot run a build. The lead builds the portal bundle in each service worktree before the writer runs `cargo test`.
- A binary from these branches must not open a live `state.db`: it upgrades the file to schema 9, and a 0.43.3 service then refuses it.
  Every review run used a temporary state directory.
