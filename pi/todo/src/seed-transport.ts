import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const version = 1,
  empty = 0,
  /**
   * Validates one executor response without interpreting task state.
   * @param {unknown} source Untrusted transport response.
   * @returns {string | undefined} Source path or ordinary launch.
   */
  sourcePath = (source: unknown): string | undefined => {
    if (
      typeof source !== "object" ||
      source === null ||
      !("version" in source) ||
      source.version !== version ||
      !("path" in source) ||
      Object.keys(source).some((key) => key !== "version" && key !== "path")
    ) {
      throw new Error("IMPORT_FAILED: invalid seed source");
    }
    if (source.path === null) {
      return undefined;
    }
    if (typeof source.path !== "string" || source.path.trim().length === empty) {
      throw new Error("IMPORT_FAILED: invalid seed path");
    }
    return source.path;
  },
  /**
   * Reads executor-owned launch transport without registering an executor flag.
   * @param {Readonly<ExtensionAPI["events"]>} events Synchronous extension event bus.
   * @returns {string | undefined} Validated source path, or no managed launch.
   */
  seedPath = (events: Readonly<ExtensionAPI["events"]>): string | undefined => {
    const replies: unknown[] = [];
    events.emit("oisin.tasks.seed-source.v1", {
      reply: (value: unknown): void => {
        replies.push(value);
      },
      version,
    });
    if (replies.length === empty) {
      return undefined;
    }
    if (replies.length !== version) {
      throw new Error("IMPORT_FAILED: ambiguous seed source");
    }
    const [source] = replies;
    return sourcePath(source);
  };

export { seedPath };
