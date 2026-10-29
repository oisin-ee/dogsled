# Brief: consolidation step 3b, direct calls between Dogsled and pi-todo

**Goal.** `pi/` and `pi/todo/src/` call each other directly. Three event channels go away: the seed path, the plan approval, and the work display. Dogsled stops using the task-provider event. pi-prewalk keeps using it. Behavior does not change.

**Spec.** `plans/oisinpi-consolidation.md`, section "Step 3", part "3b decisions". Its channel table and decisions are binding.

**Base.** `origin/main` at the commit that adds this brief. Branch `feat/todo-direct`.

**Before you start.** The lead has run `proto install`, `pnpm install --frozen-lockfile`, `moon run dogsled:web-build`, and `moon run pi-todo:generate` in your worktree. Do not commit `web/public/` or `pi/todo/src/generated/`.

**Edits.**

1. `pi/todo/src/index.ts`: add `export function createTodo(pi, options: { seedPath?: () => string | undefined } = {}): TodoParts`. It does what `piTodo` does now and returns:
   - `tasks(): TaskProviderV1 | undefined`, which gives `provider.capability()` when `ownsTodo(pi)` is true, else `undefined`.
   - `approve(plan: ApprovedPlan, ctx: PlanContext): void`. Export `ApprovedPlan` from `plan-approved.ts`.
   - `work: TodoWork`.

   The default export calls `createTodo(pi)` and returns nothing.
2. `pi/todo/src/handoff.ts`: `WorkerImport` takes the `seedPath` function. A non-empty string is a seed path. `undefined` is an ordinary launch. Any other value throws `IMPORT_FAILED: invalid seed path`. Delete `pi/todo/src/seed-transport.ts` and its tests. Change `runtime-fixture.ts` and `import-fixture.ts` to pass `seedPath`.
3. `pi/todo/src/plan-approved.ts`: replace the subscription with `approve(plan, ctx)`. Keep the cwd check, the `ownsTodo` check, their messages, and the follow-up message. Delete `isApprovedPlan`, `start`, and `stop`, and the calls to them in `index.ts`.
4. `pi/todo/src/work.ts`: replace the seven events with `attach(presenter: TodoPresenter): () => void`. `TodoDisplay` is the type in `pi/todo/src/display.ts`. `TodoPresenter` is `{ show(snapshot: { session: string; branch: string | null; revision: number; display: TodoDisplay }): boolean; open(): void; stop(): void }`.
   - `publish(ctx)` increments the revision and calls `show`. A `true` result claims: call `presentation.claim(ctx)`. A `false` result releases.
   - `start(ctx)` publishes if a presenter is attached. `attach` publishes if `start` has run.
   - `open()` calls `presenter.open()` when claimed.
   - `stop()` calls `presenter.stop()` and releases.
   - The detach function releases, as the old `oisin.work.release` did.

   Keep the `tui` and `ownsTodo` conditions of `start`.
5. `pi/work-todo.ts`: `WorkTodo` takes the `TodoWork` handle, not the event bus. It implements `TodoPresenter`:
   - `start(ctx)` attaches.
   - `stop()` detaches.
   - `reject()` detaches and sets the error.
   - `show()` returns `false` when it rejects the snapshot.

   Delete `snapshot()`, `isDisplay`, `isSections`, the `isView` checks, and the request, presenter, publisher, and retired fields. Keep `preflight`, `HostedTodo`, the session and branch checks, and the revision check. `TodoSnapshot` keeps `revision`, `session`, `branch`, and `display` only. Keep the `TodoDisplay` and `TodoView` interfaces. tsc checks that pi-todo's value fits them.
6. `pi/plan-mode.ts`: `PlanMode` and `registerPlanMode` take `approved: (plan: PlanApproved, ctx: ExtensionContext) => void`. Call it where the code emits `dogsled:plan-approved` now.
7. `pi/unit-bridge.ts` and `pi/unit-worker.ts`: take `tasks: () => TaskProvider | undefined`, and call it where they call `acquire(this.pi.events)` now. `UnitWorker` still registers the `todo-unit-file` flag. Delete its `seed-source` listener.
8. `pi/unit-protocol.ts`: delete `acquire` and `provider`. Replace its `TaskProvider` interface with a type import of pi-todo's `TaskProviderV1`. Keep every other validator.
9. `pi/index.ts`: `const todo = createTodo(pi, { seedPath: () => { const value = pi.getFlag("todo-unit-file"); return typeof value === "string" ? value : undefined; } })`. Pass `todo.tasks` to `UnitBridge`, `todo.approve` to `registerPlanMode`, and `todo.work` to `WorkTodo`. `createTodo` stays the first statement of `dogsled()`.
10. Tests:
    - Change the `pi/*.test.ts` and `pi/todo/src/*.test.ts` cases that drive these channels through a bus to drive the direct calls. Keep each case's assertion.
    - Delete only the cases that test envelope validation of a removed event, such as a wrong version, presenter, or request. Name each deleted case in the PR body.
    - `pi/todo-registration.test.ts` keeps its `oisin.tasks.acquire.v1` assertion. That event is pi-prewalk's interface.
11. Docs: as the spec's "Docs" decision says.

**Owned paths.** `pi/`, `docs/pi-work.md`, `docs/todo-workflows.md`, `docs/unit-requests.md`.

**Not owned.** `src/`, `crates/`, `packages/`, `plans/`, and the manifests. If a change there seems necessary, stop and report.

**Stop and report** in these cases:
- A test needs a behavior change, not only a change from events to calls.
- pi-todo's `TaskProviderV1` does not fit Dogsled's use without a cast.

**Your checks.** `moon run pi-todo:lint pi-todo:test pi-prewalk:test dogsled:typecheck dogsled:test-shim dogsled:format`, and the two `rg` checks of the spec's acceptance. Do not run the full suite, a release, an install, or a publish. The lead runs the full acceptance.

**Publication.** Commit with `refactor(pi): call pi-todo directly from Dogsled` through the normal hook. Push the branch and open a draft PR. Do not merge, tag, or publish.
