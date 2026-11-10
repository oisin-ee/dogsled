import { DELTA_ENTRY, STATE_ENTRY, stateChange } from "./state.ts";
import type { ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import { entry, initial, taskAt } from "./delta-fixture.ts";
import { TaskProvider } from "./old-provider-fixture.ts";
import { leafTask } from "./delta.ts";
import { runtimeFixture } from "./runtime-fixture.ts";

const count = 100,
  changes = 20,
  first = 0,
  step = 1,
  replayBranch = (): SessionEntry[] => {
    const list = initial(count),
      branch = [entry("base", STATE_ENTRY, list)];
    let current = list;
    for (let index = first; index < changes; index += step) {
      const next = structuredClone(current);
      taskAt(next).description = `Change ${index}`;
      const change = stateChange(current, next);
      branch.push(entry(`change-${index}`, change.customType, change.data));
      current = next;
    }
    return branch;
  },
  intermediateBranch = (changed: unknown): SessionEntry[] => {
    const list = initial(count),
      task = leafTask(taskAt(list)),
      id = task.unit?.reference.unitId,
      delta = (value: unknown): unknown => ({
        changed: [{ id, task: value }],
        orders: [],
        removed: [],
        version: step,
      });
    return [
      entry("base", STATE_ENTRY, list),
      entry("bad", DELTA_ENTRY, delta(changed)),
      entry("fixed", DELTA_ENTRY, delta(task)),
    ];
  },
  oldRuntime = async (): Promise<{
    runtime: Awaited<ReturnType<typeof runtimeFixture>>;
    provider: TaskProvider;
    context: ExtensionContext;
  }> => {
    const captured: { provider?: TaskProvider; context?: ExtensionContext } = {},
      runtime = await runtimeFixture((pi) => {
        captured.provider = new TaskProvider(pi);
        pi.on("session_start", (_event, ctx) => {
          captured.context = ctx;
          captured.provider?.context(ctx);
        });
      }),
      { provider, context } = captured;
    if (provider === undefined || context === undefined) {
      runtime.session.dispose();
      throw new Error("Missing retained provider context");
    }
    return { context, provider, runtime };
  };

export { count, changes, intermediateBranch, oldRuntime, replayBranch };
export { show as oldShow } from "./old-lifecycle-fixture.ts";
