import { describe, expect, it } from "vitest";
import { mkdtemp, rm, symlink } from "node:fs/promises";
import { isOwnSource } from "./source.ts";
import path from "node:path";
import { tmpdir } from "node:os";

const SOURCE = decodeURIComponent(new URL(".", import.meta.url).pathname);

describe("source", () => {
  it("matches the entry through a symlinked directory", { timeout: 5000 }, async () => {
    expect.hasAssertions();
    const dir = await mkdtemp(path.join(tmpdir(), "pi-todo-")),
      link = path.join(dir, "src");
    await symlink(SOURCE, link, "dir");
    try {
      expect(isOwnSource(path.join(link, "index.ts"))).toBe(true);
    } finally {
      await rm(dir, { force: true, recursive: true });
    }
  });

  it("rejects another file and a path that does not exist", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(isOwnSource(path.join(SOURCE, "tool.ts"))).toBe(false);
    expect(isOwnSource("/another/todo.ts")).toBe(false);
  });
});
