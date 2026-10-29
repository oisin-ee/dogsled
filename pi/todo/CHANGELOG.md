# @oisin-ee/pi-todo

## 0.12.1

No changes in this release.

## 0.12.0

### Minor Changes

- [#40](https://github.com/oisin-ee/oisinpi/pull/40) [`7fcdc45`](https://github.com/oisin-ee/oisinpi/commit/7fcdc45f08c065faf38c3841c0b2d2133ad9322f) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - Continue plans approved in Dogsled plan mode through the existing todo tool in the same session. Listen for `dogsled:plan-approved` without additional configuration.

  Document Dogsled plan mode as the owner of planning restrictions.

## 0.11.1

No changes in this release.

## 0.11.0

### Patch Changes

- [`0e37d82`](https://github.com/oisin-ee/oisinpi/commit/0e37d8259d4a7cb4cffbeccfce2a36caa0274b6e) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Skip the todo activation check for the planning selections `tier/plan-claude` and `tier/plan-gpt`. Every other selection still needs an active local unit.

## 0.10.2

### Patch Changes

- [#35](https://github.com/oisin-ee/oisinpi/pull/35) [`a4554dc`](https://github.com/oisin-ee/oisinpi/commit/a4554dc2e64ba6e82fe5731992ba26c0c31c59b2) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - A worker can call `dogsled done` after it closes its seeded todo leaf. Before, the todo gate blocked `mcp__dogsled__done`, so a finished worker could not report done.

## 0.10.1

### Patch Changes

- [#33](https://github.com/oisin-ee/oisinpi/pull/33) [`ac2e931`](https://github.com/oisin-ee/oisinpi/commit/ac2e931bf8d907eced3d075ebae2e9707713ea6f) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - The owning lead can close any open leaf with `done` and a short note, or with `drop`. Closing a leaf that a worker holds clears the binding, records the execution as released, and frees its paths. A late settlement for a released execution is a no-op.

## 0.10.0

### Minor Changes

- [#30](https://github.com/oisin-ee/oisinpi/pull/30) [`5ab2bcc`](https://github.com/oisin-ee/oisinpi/commit/5ab2bcc44215d4f22e319d93dc57e8af12499b28) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - Publish todo status groups as typed data. The work protocol is now version 3: the version 2 snapshot plus `display.sections`, four sections (`in_progress`, `pending`, `completed`, `abandoned`) that each carry a count and items with text, review flag, contract summary, and subtasks. Pi-todo no longer answers version 2 requests, so an older presenter falls back to standalone todos. Todos also gain an optional `description` on outcomes and subtasks: `content` is now a short headline and the description holds the detail. `update` sets or clears it, `view` prints it under the headline, and `display.sections` carries it for presenters; the Pi widget still shows headlines only.

## 0.9.3

No changes in this release.

## 0.9.2

No changes in this release.

## 0.9.1

No changes in this release.

## 0.9.0

No changes in this release.

## 0.8.0

### Minor Changes

- [`50877ec`](https://github.com/oisin-ee/oisinpi/commit/50877ec0f9bba71b7f70d8bf8d3754191ac73364) Thanks [@oisincoveney](https://github.com/oisincoveney)! - Add executable task contracts, local activation gates, and profile-assigned worker handoff. Import exact worker contracts before their first task request. Attach returned evidence without accepting the lead task automatically.

  Use the executor-owned seed-source capability with compatible Dogsled tooling. Require Pi 1.3.0 for the loaded startup guard. Prewalk recognizes validated activation and preserves its post-edit model switch.

## 0.7.2

No changes in this release.

## 0.7.1

### Patch Changes

- [`b2ebd5a`](https://github.com/oisin-ee/oisinpi/commit/b2ebd5aefa0a52d84e7685c014fef5203e30e3a3) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Show completion reminders in the conversation. Reminder limits and continuation behavior remain unchanged.

## 0.7.0

No changes in this release.

## 0.6.0

### Minor Changes

- [`eb67440`](https://github.com/oisin-ee/oisinpi/commit/eb674408bf1dfbdfec7fd319c3863ad05b3d730d) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Add two-level outcomes and bounded subtasks with exact parent-scoped targets. Creation stays pending, and explicit starts preserve parallel active work. Automatic activation is removed.

  Keep parents open for explicit integration review after children close. Reject premature completion atomically. Preserve completed children when dropping a parent.

  Show hierarchy and separate progress in shared rendering, model output, and reminders. Add work-unit guidance to tool results and approved-plan continuation. Old flat snapshots remain readable.

  Pi-todo now supplies the renderer hosted by Wrangle's inline widget through display protocol version 2. Shared presentation requires a matching Wrangle release. Older integrations retain standalone todos instead of flattening subtasks.

## 0.5.1

No changes in this release.

## 0.5.0

No changes in this release.

## 0.4.0

### Minor Changes

- [`0a1434e`](https://github.com/oisin-ee/oisinpi/commit/0a1434eedb156c21abd36ae611a124b06f0ad1d8) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Continue approved Plannotator plans with the existing todo tool when external execution is enabled. Keep one branch-backed task list instead of a separate Plannotator checklist.

- [`74a563f`](https://github.com/oisin-ee/oisinpi/commit/74a563f9fa806902d9c0cf7d1d2e01edea255dec) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Publish validated todo snapshots for Wrangle's shared work display. Keep task state on the Pi session branch and restore the standalone widget when shared presentation ends.

## 0.3.0

### Minor Changes

- [`7f33497`](https://github.com/oisin-ee/oisinpi/commit/7f3349706861ab943f0be931ab3cff49e74d5f37) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Show only in-progress tasks in a themed, width-aware widget by default. Alt+T and /todos toggle grouped upcoming, done, and dropped tasks inline. Use /todos list for the full text summary.

## 0.2.0

### Minor Changes

- [#10](https://github.com/oisin-ee/oisinpi/pull/10) [`1c8612b`](https://github.com/oisin-ee/oisinpi/commit/1c8612b7dd1ce00e948d6fd6d3a421f956629e1a) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - `@oisin-ee/oisinpi` replaces `@oisin-ee/pi-prewalk-kit`. It bundles pi-prewalk, pi-todo, pi-guardrails, and
  pi-wrangle 0.11.0. `@oisin-ee/pi-guardrails` is new: the agent hook policies for pi, Claude Code (`agent-hook`), and
  Codex, ported from the dotfiles hooks runtime.

## 0.1.1

### Patch Changes

- [#8](https://github.com/oisin-ee/pi-prewalk/pull/8) [`785734f`](https://github.com/oisin-ee/pi-prewalk/commit/785734ff387908f263c9031d8b16392ece7198f5) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - `/prewalk on` arms a handoff, the same as `/prewalk`. The todo widget is hidden while the list has no tasks.

## 0.1.0

### Minor Changes

- [#7](https://github.com/oisin-ee/pi-prewalk/pull/7) [`f528eab`](https://github.com/oisin-ee/pi-prewalk/commit/f528eab26e4d2de300e1fdb9a3d5ec52e06ca9b6) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - First release: the `todo` tool, the prewalk planner-to-executor handoff, and the kit that installs both.
