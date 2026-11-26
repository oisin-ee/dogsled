# Dogsled north star: one place to run the work

**Status:** Design proposal. No runtime changes or approval to implement them.

- [Design book](../docs/design/north-star/index.html) — open the file in a browser. It needs no server or network.
- [Editable Squig canvas](north-star.squig.json) — one Desktop group with seven frames at 1440 × 900. From a built Squig checkout, run `pnpm squig serve <absolute-path-to-north-star.squig.json>`.

## Goal

A returning owner can tell what needs a decision, act in place, and leave. Dogsled feels like one machine: the owner selects the work and the repository. Dogsled selects where the work runs.

## Direction

Keep the Quiet Console layout from `DESIGN.md` and `.impeccable/surfaces/web.md`. Keep the repository sidebar, Needs you, and the lead’s Now and Todos panels. Use the proposed Work overview as the default when no lead or request is selected. Open deep links at the selected item.

A host-first dashboard helps diagnose placement, but it makes the owner map machines to work on each visit. A chat-first home is faster for one lead, but it hides requests from other leads. Use the work-first layout: Needs you stays visible, and a decision opens inside the relevant lead. Machine names, pane IDs, and raw placement evidence are reserved for diagnostics outside the primary views. Capacity shows aggregate eligibility and the reason a run cannot proceed. It never implies that each executor is interchangeable.

## Screen sequence

| Frame | Owner task | Design check |
| --- | --- | --- |
| 01 Work overview | Return and decide what to open | Two open requests, live work, and aggregate capacity are distinct. Branch cleanup is not an inbox request. |
| 02 New lead | Set a goal and pick a repository | No host picker. Keep the draft if the repository catalog is stale. |
| 03 Lead workspace | Follow evidence and steer | Keep the composer pinned. Put Now above Todos, Workers, and Files. Nest workers and jobs under the lead. |
| 04 Question | Answer a request without leaving the lead | Show the question, a recommendation, and explicit Submit / Decline actions. Keep the conversation available. |
| 05 Plan review | Read, annotate, and decide | The review replaces the lead workspace, not the sidebar. Plan and Changes remain separate. Pin the decision actions. |
| 06 Job detail | See what is queued and why | Keep queue admission distinct from execution and completion. Cancel admission, not execution, while queued. No fake chat. |
| 07 Recovery | Resume from a known state | Show the last known state and what cannot be verified. Offer a saved copy without claiming task completion. |

## Speed and verification

The book gives proposed owner-task and p95 system budgets. They are targets, not measurements of the current portal. Later implementation must record a baseline and separate immediate UI feedback from durable acknowledgment and from agent execution. Measure stalled or disconnected fixtures as well as a healthy session.

Review the book from `file://`, offline. At 1440 × 900, open each scene at native size and check the persistent sidebar, bounded content, and pinned controls. Check keyboard travel and the return from full-size view. Check all primary scenes for machine names, host tooltips, and location selectors. Read the wireframes in the same order as the book.

## Scope

This proposal does not change scheduling, persistence, Herdr, or the live portal. It does not add phone access, remote sign-in, or a new diagnostics surface. The book uses illustrative work and status data, not fleet snapshots.
