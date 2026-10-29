import type {
  ContextEvent,
  ContextEventResult,
  SessionEntry,
} from "@earendil-works/pi-coding-agent";
import { branchRecords } from "./branch.ts";
import { project } from "./wasm.ts";

const LAST = -1,
  /**
   * Removes steering outside the branch's live arm before the next request.
   * @param {readonly SessionEntry[]} branch Current branch.
   * @param {readonly AgentMessage[]} messages Proposed context.
   * @returns {ContextEventResult} Context without canceled or consumed nudges.
   */
  projectContext = (
    branch: readonly SessionEntry[],
    messages: Readonly<ContextEvent["messages"]>,
  ): ContextEventResult => {
    const projection = project({ records: branchRecords(branch) }),
      nudges = messages.filter(
        (message) => message.role === "custom" && message.customType === "prewalk-plan",
      ),
      latest = nudges.at(LAST);
    return {
      messages: messages.filter(
        (message) =>
          message.role !== "custom" ||
          message.customType !== "prewalk-plan" ||
          (projection.keep_plan && message === latest),
      ),
    };
  };

export { projectContext };
