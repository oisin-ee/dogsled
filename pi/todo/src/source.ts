/*
 * Which file registered a tool. pi reports a tool's `sourceInfo.path` as the
 * path it was configured with, but `import.meta.url` names the module's real
 * path. The two differ when the path has a symlink, such as macOS `/tmp` or a
 * linked agent directory, so both are compared after resolving symlinks.
 */
import {
  closeSync,
  constants,
  existsSync,
  fstatSync,
  openSync,
  readFileSync,
  realpathSync,
} from "node:fs";

const OWN = new Set([
    realpathSync(new URL("../../index.ts", import.meta.url)),
    realpathSync(new URL("index.ts", import.meta.url)),
  ]),
  /**
   * Whether `path` is this package's extension entry. A path that does not
   * exist, such as a synthetic source, cannot be this file.
   * @param {string} path A tool's `sourceInfo.path`.
   * @returns {boolean} Whether the file is this package's `index.ts`.
   */
  isOwnSource = (path: string): boolean => existsSync(path) && OWN.has(realpathSync(path)),
  /**
   * Reads only a bounded regular transport file owned by the executor.
   * @param {string} path Seed artifact path.
   * @returns {string} Untrusted JSON for Rust validation.
   */
  seedSource = (path: string): string => {
    const descriptor = openSync(path, constants.O_RDONLY | constants.O_NONBLOCK),
      maximum = 1_048_576;
    try {
      const metadata = fstatSync(descriptor);
      if (!metadata.isFile() || metadata.size > maximum) {
        throw new Error("IMPORT_FAILED: seed must be a regular file of at most 1 MiB");
      }
      const bytes = readFileSync(descriptor);
      if (bytes.byteLength > maximum) {
        throw new Error("IMPORT_FAILED: seed exceeds 1 MiB");
      }
      return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    } finally {
      closeSync(descriptor);
    }
  };

export { isOwnSource, seedSource };
