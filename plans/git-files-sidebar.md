# Portal Git files sidebar

Status: mockups ready for review. No production implementation is authorized.

## Scope

The portal gets a Files tab beside Todos in the existing right sidebar.
The list shows the selected agent's checkout on its host, not combined worker checkouts.
Rows show repository-relative paths and Git status only. They do not open files or diffs.
Staging, discard, committed task history, and the Herdr sidebar remain outside this scope.

The design question is how to show file paths and two Git status fields within the existing 300px sidebar.
The mockups preserve the console palette, typography, inbox, conversation, and composer.
They extend the current `web/src/agent_view.rs` layout without changing production files.

## Artifacts

- Interactive mockups: [index.html](../.impeccable/mocks/git-files-sidebar/index.html).
- Side-by-side comparison: [comparison.png](../.impeccable/mocks/git-files-sidebar/comparison.png).
- Full portal: [desktop.png](../.impeccable/mocks/git-files-sidebar/desktop.png).
- Compact portal: [compact.png](../.impeccable/mocks/git-files-sidebar/compact.png).
- Editable wireframes: [git-files-sidebar.squig.json](git-files-sidebar.squig.json).

The HTML file works locally without a server, network connection, or build.
Its review toolbar is not part of the proposed product UI.
All file statuses, branch names, conversations, and task counts are synthetic examples.
The screenshots come from this HTML, not image generation.

## Alternatives

| Option | Benefit | Trade-off |
| --- | --- | --- |
| A: Flat list | One row per file, with separate index and worktree status | Readers must recognize Git status letters |
| B: By stage | Explicit Staged, Unstaged, Untracked, and Conflicts groups | A file with both staged and unstaged changes appears twice |
| C: By folder | Shared path prefixes appear once | Folder headings take more room for small changes |

A is the recommendation, not an approved selection.
The dot in options A and C makes an unchanged status field visible. It is a display convention, not raw Git output.
The Files count is the number of unique paths. Option B has seven paths across eight rows in the default example.
Renames show the previous path below the current path.
Conflicts have a text label, not color alone.
Git status markers stay neutral to preserve the console's existing color meanings.

## Behavior and responsive layout

At 1400px and wider, Todos and Files share the 300px right sidebar.
Below 1400px, Files joins Chat and Todos in the conversation header.
Below 1100px, Agents also becomes a header tab. The inbox stays visible.
The minimum product width remains 800px. Phone layouts remain outside scope.

The selected checkout, branch, and host appear above the files.
File lists scroll within their panel. The composer stays at the bottom of the viewport.
Rows are not links or buttons. Paths wrap instead of losing required text.
Todos remains accessible from the same tab strip.

The state selector demonstrates changes, clean, loading, stale, unavailable, non-repository, conflict, and long-list states.
Stale data retains its last snapshot with an explicit label.
An unavailable status never appears as a clean checkout.
The mockup does not fetch Git data or send messages.

## Evidence

Impeccable's Operate, new-work, and craft-floor guidance informed this extension of the existing console.
Visual references were `DESIGN.md`, current portal code, and the running portal on 2026-11-06.
The HTML uses plain controls for design review only. Production controls must use the existing Dioxus components.

Browser inspection covered the comparison at 1440×1020 and the portal at 1600×1000, 1000×900, and 800×900.
The compact portal fits the viewport without document overflow. Its inbox and composer remain visible.
All eight fixture states showed distinct content without undefined labels.
Default row counts were 7, 8, and 7 for A, B, and C.
The long-list fixture has 37 unique paths and scrolls inside the panel.
Squig measured all text nodes without overflow or missing glyphs. The canvas has 63 editable nodes across four variations.

These checks prove the design artifact only. They do not prove Git collection, remote delivery, or production accessibility.

## Resume the editable canvas

From the existing Squig checkout:

```sh
pnpm --dir /Users/oisin/dev/squig squig serve /Users/oisin/dev/dogsled/plans/git-files-sidebar.squig.json
```

The companion prints a new local editor link. Its token is not stored in this plan.
The companion used during review stops after the saved artifacts are verified.

## Next decision

Choose A, B, or C, or request a revision. Mockup approval does not authorize production implementation.
