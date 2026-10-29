# @oisin-ee/oisinpi

## 0.12.1

### Patch Changes

- Updated dependencies [[`2d9ac2b`](https://github.com/oisin-ee/oisinpi/commit/2d9ac2b60d36a1da10a8700e11ae19a4c3455436)]:
  - @oisin-ee/pi-prewalk@0.12.1
  - @oisin-ee/pi-guardrails@0.12.1
  - @oisin-ee/pi-tiers@0.12.1
  - @oisin-ee/pi-todo@0.12.1

## 0.12.0

### Patch Changes

- Updated dependencies [[`7fcdc45`](https://github.com/oisin-ee/oisinpi/commit/7fcdc45f08c065faf38c3841c0b2d2133ad9322f)]:
  - @oisin-ee/pi-todo@0.12.0
  - @oisin-ee/pi-guardrails@0.12.0
  - @oisin-ee/pi-prewalk@0.12.0
  - @oisin-ee/pi-tiers@0.12.0

## 0.11.1

### Patch Changes

- Updated dependencies [[`f1837f2`](https://github.com/oisin-ee/oisinpi/commit/f1837f28bc25fb00feebf7c47509db9bbd3b897c)]:
  - @oisin-ee/pi-guardrails@0.11.1
  - @oisin-ee/pi-prewalk@0.11.1
  - @oisin-ee/pi-tiers@0.11.1
  - @oisin-ee/pi-todo@0.11.1

## 0.11.0

### Patch Changes

- Updated dependencies [[`49b00f1`](https://github.com/oisin-ee/oisinpi/commit/49b00f117d4d64a51cdb2b502ac505a99ffe83dc), [`0e37d82`](https://github.com/oisin-ee/oisinpi/commit/0e37d8259d4a7cb4cffbeccfce2a36caa0274b6e)]:
  - @oisin-ee/pi-guardrails@0.11.0
  - @oisin-ee/pi-todo@0.11.0
  - @oisin-ee/pi-prewalk@0.11.0
  - @oisin-ee/pi-tiers@0.11.0

## 0.10.2

### Patch Changes

- [#35](https://github.com/oisin-ee/oisinpi/pull/35) [`a4554dc`](https://github.com/oisin-ee/oisinpi/commit/a4554dc2e64ba6e82fe5731992ba26c0c31c59b2) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - A worker can call `dogsled done` after it closes its seeded todo leaf. Before, the todo gate blocked `mcp__dogsled__done`, so a finished worker could not report done.
- Updated dependencies [[`a4554dc`](https://github.com/oisin-ee/oisinpi/commit/a4554dc2e64ba6e82fe5731992ba26c0c31c59b2)]:
  - @oisin-ee/pi-todo@0.10.2
  - @oisin-ee/pi-guardrails@0.10.2
  - @oisin-ee/pi-prewalk@0.10.2
  - @oisin-ee/pi-tiers@0.10.2

## 0.10.1

### Patch Changes

- Updated dependencies [[`ac2e931`](https://github.com/oisin-ee/oisinpi/commit/ac2e931bf8d907eced3d075ebae2e9707713ea6f)]:
  - @oisin-ee/pi-todo@0.10.1
  - @oisin-ee/pi-guardrails@0.10.1
  - @oisin-ee/pi-prewalk@0.10.1
  - @oisin-ee/pi-tiers@0.10.1

## 0.10.0

### Minor Changes

- [#30](https://github.com/oisin-ee/oisinpi/pull/30) [`5ab2bcc`](https://github.com/oisin-ee/oisinpi/commit/5ab2bcc44215d4f22e319d93dc57e8af12499b28) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - Publish todo status groups as typed data. The work protocol is now version 3: the version 2 snapshot plus `display.sections`, four sections (`in_progress`, `pending`, `completed`, `abandoned`) that each carry a count and items with text, review flag, contract summary, and subtasks. Pi-todo no longer answers version 2 requests, so an older presenter falls back to standalone todos. Todos also gain an optional `description` on outcomes and subtasks: `content` is now a short headline and the description holds the detail. `update` sets or clears it, `view` prints it under the headline, and `display.sections` carries it for presenters; the Pi widget still shows headlines only.

### Patch Changes

- Updated dependencies [[`5ab2bcc`](https://github.com/oisin-ee/oisinpi/commit/5ab2bcc44215d4f22e319d93dc57e8af12499b28)]:
  - @oisin-ee/pi-todo@0.10.0
  - @oisin-ee/pi-guardrails@0.10.0
  - @oisin-ee/pi-prewalk@0.10.0
  - @oisin-ee/pi-tiers@0.10.0

## 0.9.3

### Patch Changes

- Updated dependencies [[`6f6e309`](https://github.com/oisin-ee/oisinpi/commit/6f6e3092eaec85e4f47c52ec652db359d7ddd78f)]:
  - @oisin-ee/pi-tiers@0.9.3
  - @oisin-ee/pi-guardrails@0.9.3
  - @oisin-ee/pi-prewalk@0.9.3
  - @oisin-ee/pi-todo@0.9.3

## 0.9.2

### Patch Changes

- Updated dependencies [[`3e750ac`](https://github.com/oisin-ee/oisinpi/commit/3e750ace481cfcf55296da1df034473d86d74347)]:
  - @oisin-ee/pi-tiers@0.9.2
  - @oisin-ee/pi-guardrails@0.9.2
  - @oisin-ee/pi-prewalk@0.9.2
  - @oisin-ee/pi-todo@0.9.2

## 0.9.1

### Patch Changes

- Updated dependencies [[`bcba2d4`](https://github.com/oisin-ee/oisinpi/commit/bcba2d42673c2cf12c23b62079c2a96c0b098e30)]:
  - @oisin-ee/pi-tiers@0.9.1
  - @oisin-ee/pi-guardrails@0.9.1
  - @oisin-ee/pi-prewalk@0.9.1
  - @oisin-ee/pi-todo@0.9.1

## 0.9.0

### Minor Changes

- [#21](https://github.com/oisin-ee/oisinpi/pull/21) [`5e80e12`](https://github.com/oisin-ee/oisinpi/commit/5e80e1209af813427b16049c77de3f068d928fd1) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - Add sticky tier virtual models with bounded recovery, shared gateway circuits, and quota-aware pool selection.

### Patch Changes

- Updated dependencies [[`5e80e12`](https://github.com/oisin-ee/oisinpi/commit/5e80e1209af813427b16049c77de3f068d928fd1)]:
  - @oisin-ee/pi-tiers@0.9.0
  - @oisin-ee/pi-guardrails@0.9.0
  - @oisin-ee/pi-prewalk@0.9.0
  - @oisin-ee/pi-todo@0.9.0

## 0.8.0

### Patch Changes

- Updated dependencies [[`50877ec`](https://github.com/oisin-ee/oisinpi/commit/50877ec0f9bba71b7f70d8bf8d3754191ac73364)]:
  - @oisin-ee/pi-todo@0.8.0
  - @oisin-ee/pi-prewalk@0.8.0
  - @oisin-ee/pi-guardrails@0.8.0

## 0.7.2

### Patch Changes

- Updated dependencies [[`f31e9a6`](https://github.com/oisin-ee/oisinpi/commit/f31e9a6043d5b7cbe351540b51f87eadf85c9af3)]:
  - @oisin-ee/pi-guardrails@0.7.2
  - @oisin-ee/pi-prewalk@0.7.2
  - @oisin-ee/pi-todo@0.7.2

## 0.7.1

### Patch Changes

- Updated dependencies [[`b2ebd5a`](https://github.com/oisin-ee/oisinpi/commit/b2ebd5aefa0a52d84e7685c014fef5203e30e3a3)]:
  - @oisin-ee/pi-todo@0.7.1
  - @oisin-ee/pi-guardrails@0.7.1
  - @oisin-ee/pi-prewalk@0.7.1

## 0.7.0

### Minor Changes

- [`5a49c73`](https://github.com/oisin-ee/oisinpi/commit/5a49c7387a1be43057bb881c34794347de3769b1) Thanks [@oisincoveney](https://github.com/oisincoveney)! - Remove pi-wrangle from the collection. Install `npm:@oisin-ee/pi-wrangle` on its own, at the same version as the `wrangle` binary, so a wrangle release no longer needs an oisinpi release. The collection now bundles pi-prewalk, pi-todo, and pi-guardrails.

### Patch Changes

- Updated dependencies []:
  - @oisin-ee/pi-guardrails@0.7.0
  - @oisin-ee/pi-prewalk@0.7.0
  - @oisin-ee/pi-todo@0.7.0

## 0.6.0

### Patch Changes

- [`16abba6`](https://github.com/oisin-ee/oisinpi/commit/16abba6b96e8ca0106a011e0fe4629396839b08c) Thanks [@oisincoveney](https://github.com/oisincoveney)! - Bundle pi-wrangle 0.12.3. The widget above the editor uses the wrangle board design again: host meters, the current task, and agents and jobs grouped TO LAND, ATTENTION, RUNNING, and WAITING with aligned columns. Alt+W and `/work` still expand it in place. Wrangle hosts pi-todo's display protocol version 2 view inline.
- Updated dependencies [[`eb67440`](https://github.com/oisin-ee/oisinpi/commit/eb674408bf1dfbdfec7fd319c3863ad05b3d730d)]:
  - @oisin-ee/pi-todo@0.6.0
  - @oisin-ee/pi-guardrails@0.6.0
  - @oisin-ee/pi-prewalk@0.6.0

## 0.5.1

### Patch Changes

- [`0170caa`](https://github.com/oisin-ee/oisinpi/commit/0170caa9c59c4e3d71409627b14925cefcc68679) Thanks [@momo-momokaya[bot]](https://github.com/momo-momokaya%5Bbot%5D)! - Bundle pi-wrangle 0.12.2. Show the current task and named agents and jobs above the editor. Alt+W and `/work` expand the same widget in place, without a popup or editor replacement.
- Updated dependencies []:
  - @oisin-ee/pi-guardrails@0.5.1
  - @oisin-ee/pi-prewalk@0.5.1
  - @oisin-ee/pi-todo@0.5.1

## 0.5.0

### Minor Changes

- Bundle the released shared work strip and panel. Use Wrangle for background jobs and keep todo state on its Pi branch. Include updated guardrails that direct long work to Wrangle without relaxing timeout or wait checks.

### Patch Changes

- Updated dependencies [`1f4a622`]:
  - @oisin-ee/pi-guardrails@0.5.0
  - @oisin-ee/pi-prewalk@0.5.0
  - @oisin-ee/pi-todo@0.5.0

## 0.4.0

### Patch Changes

- Updated dependencies [[`0a1434e`](https://github.com/oisin-ee/oisinpi/commit/0a1434eedb156c21abd36ae611a124b06f0ad1d8), [`74a563f`](https://github.com/oisin-ee/oisinpi/commit/74a563f9fa806902d9c0cf7d1d2e01edea255dec)]:
  - @oisin-ee/pi-todo@0.4.0
  - @oisin-ee/pi-guardrails@0.4.0
  - @oisin-ee/pi-prewalk@0.4.0

## 0.3.0

### Patch Changes

- Updated dependencies [[`7f33497`](https://github.com/oisin-ee/oisinpi/commit/7f3349706861ab943f0be931ab3cff49e74d5f37)]:
  - @oisin-ee/pi-todo@0.3.0
  - @oisin-ee/pi-guardrails@0.3.0
  - @oisin-ee/pi-prewalk@0.3.0

## 0.2.0

### Minor Changes

- [#10](https://github.com/oisin-ee/oisinpi/pull/10) [`1c8612b`](https://github.com/oisin-ee/oisinpi/commit/1c8612b7dd1ce00e948d6fd6d3a421f956629e1a) Thanks [@momo-momokaya](https://github.com/apps/momo-momokaya)! - `@oisin-ee/oisinpi` replaces `@oisin-ee/pi-prewalk-kit`. It bundles pi-prewalk, pi-todo, pi-guardrails, and
  pi-wrangle 0.11.0. `@oisin-ee/pi-guardrails` is new: the agent hook policies for pi, Claude Code (`agent-hook`), and
  Codex, ported from the dotfiles hooks runtime.

### Patch Changes

- Updated dependencies [[`1c8612b`](https://github.com/oisin-ee/oisinpi/commit/1c8612b7dd1ce00e948d6fd6d3a421f956629e1a)]:
  - @oisin-ee/pi-guardrails@0.2.0
  - @oisin-ee/pi-prewalk@0.2.0
  - @oisin-ee/pi-todo@0.2.0
