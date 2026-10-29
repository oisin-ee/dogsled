# @oisin-ee/pi-prewalk

## 0.12.1

### Patch Changes

- [#42](https://github.com/oisin-ee/oisinpi/pull/42) [`2d9ac2b`](https://github.com/oisin-ee/oisinpi/commit/2d9ac2b60d36a1da10a8700e11ae19a4c3455436) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - Size the prewalk checklist to the task. The plan prompt no longer asks for 5–9 todo items; it reuses the task's verified steps, or one item per distinct change or check.

## 0.12.0

No changes in this release.

## 0.11.1

No changes in this release.

## 0.11.0

No changes in this release.

## 0.10.2

No changes in this release.

## 0.10.1

No changes in this release.

## 0.10.0

No changes in this release.

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

No changes in this release.

## 0.7.0

No changes in this release.

## 0.6.0

No changes in this release.

## 0.5.1

No changes in this release.

## 0.5.0

No changes in this release.

## 0.4.0

No changes in this release.

## 0.3.0

No changes in this release.

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
