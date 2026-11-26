# Dogsled north star

Status: revision 3, 2026-11-27. The owner decided the direction (see Decisions). It changes no runtime code and does not authorize a layout change: that needs Squig wireframes and its own plan.

- Design book: [`docs/design/north-star/index.html`](../docs/design/north-star/index.html). Open the file in a browser. It needs no server and no network; it loads the portal fonts from `web/assets/fonts/`.
- Speed plan: [`plans/portal-snappy.md`](portal-snappy.md). It has the measurements behind the "Today" column and the units that reach the budgets.

## The owner's thesis

1. Dogsled is a layer on Herdr and is as fast as Herdr.
2. The owner works with leads.
3. The owner does not know and does not care where a lead or an agent runs. The leads use every machine as one.

## Direction

The owner talks to leads. The list holds only leads. Each row shows a short name and the last thing the lead said. Workers, jobs, branches and files live inside their lead. Home is the composer, with what needs the owner and what leads said since the last visit. A machine name appears only in one capacity panel.

The book has six chapters: direction and seven principles, speed budgets with today's measurements, identity, ten screens at 1440 × 900, what goes away, and the decisions.

## Decisions

The owner decided these on 2026-11-27.

1. Home is the composer, with Needs you and what leads said since the last visit.
2. A row shows the lead's short name and the last thing the lead said. Todo counts, model and effort move into the thread.
3. "Branches to review" becomes a "Landed" receipt in the lead's thread, and one decision per repository for branches that have commits and no lead. Change by the owner: the right panel gets a PRs tab that lists the pull requests of the lead and its workers.
4. A machine name appears only in the capacity panel. Change by the owner: each thread header also names its machine, on the control that opens the raw session in Herdr.
5. The speed budgets of chapter 02 are the acceptance test of the speed work. Each unit of `plans/portal-snappy.md` ends with the measurement of its row.

What the decisions need that does not exist yet:

- The service must remove the record of a branch that is gone from GitHub and from its machine (decision 3).
- The service must know the pull requests of a lead's branches and of its workers' branches, with their state (decision 3).
- `PRODUCT.md` still lists "see branches still to land" and "check host capacity on `local`, `netcup` and `momokaya-2`" as jobs of the first screen. Revise it with the layout plan.

## Limits

- The mark is a sketch, not final artwork.
- The work shown in the screens is invented. The controls do nothing.
- The screens are drawn for 1440 × 900. Narrow widths are not drawn.
- The book has no Squig canvas. Revision 1 had a canvas of seven frames for screens that this revision replaced; it was removed so that no canvas contradicts the book. Draw the Squig wireframes from the approved decisions before any layout is implemented.
- The "Today" numbers come from one Mac on one evening (see the speed plan).

## History

- Revision 1 (commit `28d02d9`): a writer's book that kept the current layout, with seven screens and budgets without a baseline.
- Revision 2: rewritten from the owner's thesis, with measured baselines, ten screens, the removals and the decisions.
- Revision 3: the owner's decisions, with a PRs tab and a Herdr control that names the machine.
