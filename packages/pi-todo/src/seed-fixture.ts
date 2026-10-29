import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const version = 1;

interface Query {
  readonly version: number;
  readonly reply: (value: unknown) => void;
}

/**
 * Checks the transport request before a fixture responds.
 * @param {unknown} request Event payload.
 * @returns {boolean} Whether the response callback exists.
 */
const query = (request: unknown): request is Query =>
    typeof request === "object" &&
    request !== null &&
    "version" in request &&
    request.version === version &&
    "reply" in request &&
    typeof request.reply === "function",
  /**
   * Supplies an executor response without loading execution machinery.
   * @param {Readonly<ExtensionAPI["events"]>} events Synchronous runtime bus.
   * @param {unknown} response Test response.
   */
  registerSeedSource = (events: Readonly<ExtensionAPI["events"]>, response: unknown): void => {
    events.on("oisin.tasks.seed-source.v1", (request: unknown): void => {
      if (query(request)) {
        request.reply(response);
      }
    });
  };

export { registerSeedSource };
