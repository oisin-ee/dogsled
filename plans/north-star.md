# Dogsled north star

Status: design proposal, revision 2, 2026-11-27. Not approved. It changes no runtime code.

- Design book: [`docs/design/north-star/index.html`](../docs/design/north-star/index.html). Open the file in a browser. It needs no server and no network; it loads the portal fonts from `web/assets/fonts/`.
- Speed plan: [`plans/portal-snappy.md`](portal-snappy.md). It has the measurements behind the "Today" column and the units that reach the budgets.

## The owner's thesis

1. Dogsled is a layer on Herdr and is as fast as Herdr.
2. The owner works with leads.
3. The owner does not know and does not care where a lead or an agent runs. The leads use every machine as one.

## Direction

The owner talks to leads. The list holds only leads. Each row shows a short name and the last thing the lead said. Workers, jobs, branches and files live inside their lead. Home is the composer, with what needs the owner and what leads said since the last visit. A machine name appears only in one capacity panel.

The book has six chapters: direction and seven principles, speed budgets with today's measurements, identity, ten screens at 1440 × 900, what goes away, and the decisions.

## Decisions for the owner

1. Home is the composer. Recommended: yes.
2. A row shows the lead's name and its last words. Recommended: yes.
3. "Branches to review" becomes a "Landed" receipt, or one decision for branches with no lead. Recommended: yes.
4. Machine names appear only in the capacity panel. Recommended: yes.
5. The speed budgets of chapter 02 become the acceptance test of the speed work. Recommended: yes.

## Limits

- The mark is a sketch, not final artwork.
- The work shown in the screens is invented. The controls do nothing.
- The screens are drawn for 1440 × 900. Narrow widths are not drawn.
- The book has no Squig canvas. Revision 1 had a canvas of seven frames for screens that this revision replaced; it was removed so that no canvas contradicts the book. Draw the Squig wireframes from the approved decisions before any layout is implemented.
- The budgets are proposals. The "Today" numbers come from one Mac on one evening (see the speed plan).

## History

- Revision 1 (commit `28d02d9`): a writer's book that kept the current layout, with seven screens and budgets without a baseline.
- Revision 2: rewritten from the owner's thesis, with measured baselines, ten screens, the removals and the decisions.
