import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

interface ActiveProvider {
  readonly active: () => unknown;
}

/**
 * Validates only the capability method this consumer needs.
 * @param {unknown} value Capability response.
 * @returns {boolean} Whether the active query is callable.
 */
const isProvider = (value: unknown): value is ActiveProvider =>
    typeof value === "object" &&
    value !== null &&
    "active" in value &&
    typeof value.active === "function",
  /**
   * Queries current activation without loading todo or interpreting its snapshots.
   * @param {Readonly<ExtensionAPI["events"]>} events Extension capability bus.
   * @returns {boolean | undefined} Activation, or unavailable legacy capability.
   */
  activeTask = (events: Readonly<ExtensionAPI["events"]>): boolean | undefined => {
    const replies: unknown[] = [];
    events.emit("oisin.tasks.acquire.v1", {
      reply: (provider: unknown): void => {
        replies.push(provider);
      },
      version: 1,
    });
    const [provider] = replies,
      one = 1;
    if (replies.length !== one || !isProvider(provider)) {
      return undefined;
    }
    const result: unknown = provider.active();
    if (
      typeof result !== "object" ||
      result === null ||
      !("ok" in result) ||
      result.ok !== true ||
      !("value" in result)
    ) {
      return undefined;
    }
    return result.value !== null && result.value !== undefined;
  };

export { activeTask };
