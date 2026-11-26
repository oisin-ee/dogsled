# Portal: the two Autonomous switches and the Decisions tab

Plan: `plans/autonomous-toggle.md` (approved), section "Portal UI". Wireframe: `plans/autonomous-toggle.squig.json`, render `.impeccable/mocks/autonomous-toggle/wireframe.png`. Base: `origin/main` c55c4ff.

## Outcome

The portal shows and flips the fleet switch and each lead's switch, marks autonomous leads, and lists a lead's decisions in a fourth right-column tab, in a 400px right column beside a full-width thread.

## Contract with the service (built in parallel; do not wait for it)

Types are in `portal-types/src/autonomy.rs`. The snapshot has `autonomy: Option<AutonomyView>`. Each `AgentView` has `autonomous: Option<AutonomyCause>` and `decisions: Option<DecisionMark>`.

- `POST /api/autonomy/fleet` and `POST /api/agents/{host}/{pane}/autonomy`, body `SwitchBody`. An error body is `{"error": "..."}`.
- `GET /api/agents/{host}/{pane}/decisions` returns `DecisionList`, newest first, with `gaps`.

## Edits

1. Add the official Switch: `dx components add switch` in `web/`, as `plans/dioxus-components.md` requires. Keep the generated files unchanged.
2. `web/src/api.rs`: `switch_fleet(on)`, `switch_agent(address, on)`, and `decisions(address)`, in the style of `change_settings` (:113) and `focus_agent` (:192).
3. `web/src/app.rs`: the fleet switch after Metrics in `.console-actions` (:670), label "Autonomous". It is on when `autonomy.fleet` is not empty and disabled when `autonomy` is `None`.
   `LeadPanel` (:79) gains `Decisions` with the value `decisions`, in both `match` sites (:659).
4. `web/src/agent_view.rs`:
   - The agent switch in `.lead-header`, before `.lead-actions` (:1090). States, from the wireframe: off; on (`Agent`); on and disabled with the label "Autonomous · fleet" (`Fleet`); on and disabled with "Autonomous · from lead" (`Owner`). In a worker view the switch is always disabled.
   - A fourth detail tab `Decisions {count}` after Files (:1183), and a fifth header tab below 1400px (:1086). The count is `decisions.count`, or 0.
   - Read the list when the tab is open and the agent's `DecisionMark` or address changes. Do not poll and do not open a stream.
   - A failed switch request sets `action_error`, as the other header actions do.
5. `web/src/decisions.rs` (new): the panel, with the official Accordion as the Todos panel uses it.
   A closed row shows the title, then `time · who · source`. `who` is "lead" for the page's own agent, else "worker <name>".
   An open row shows Question, Decision, Why, and Other options; absent fields are not shown. A `plan` row has a link that opens its review.
   States: empty with the switch off, empty with the switch on, and gaps ("Decisions are unavailable. Host X did not answer."), with the texts of the wireframe.
6. `web/src/board.rs` (:647, the agent row): an outline `Auto` badge on each row whose agent has `autonomous`.
7. `web/src/conversation_events.rs`: a `decide` call renders as a "Decision recorded" card with its title. Follow `operation` (:11) and `CallPanel` (:78).
8. `web/assets/console.css`: the right column is 400px (:589). `.conversation-turns` (:701) and `.conversation-footer` (:939) use 16px side padding; their content loses the 704px limit (:710, :944) and fills the column. Keep the rules under `@media (max-width: 1399px)` (:1474) working.
9. `DESIGN.md`: the Layout line (:208) and the Right column section (:266) state the new widths and the fourth tab. Add one short Components entry for the switch.

## Paths

Own: `web/`, `DESIGN.md`. Read: `portal-types/`, `plans/`, `.impeccable/mocks/autonomous-toggle/`. Do not change `portal-types/` or `src/`. If a wire type is wrong, reply `blocked` with the reason.

## Acceptance

- `cargo test -p web decisions` passes, with component tests in `web/src/component_tests.rs` (style of `model_picker_scrolls_and_selects…`, :141): the panel renders a lead decision and a worker decision in order with their labels; the three empty and gap states render their texts; the switch is disabled for `Fleet` and `Owner`.
- `moon run web-lint` passes.
- In the isolated browser (`mise run agent:browser`), with the built portal and fixture data, at 1600, 1399, 1100, and 800px: no clipped tab, switch, or title; the page does not scroll as a document. Attach one screenshot per width to the reply, and close the browser.

## Checks

`cargo fmt --check`, `moon run web-lint`, `cargo test -p web decisions`. The browser check needs one `moon run web-build`; run it once. Run no other test.
