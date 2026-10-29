import { describe, expect, it, vi } from "vitest";
import { repositoryIdentity } from "./repository.ts";

vi.setConfig({ testTimeout: 5000 });

describe("canonical repository identity", () => {
  it.each([
    "git@github.com:oisin-ee/oisinpi.git",
    "ssh://git@github.com/oisin-ee/oisinpi.git",
    "https://github.com/oisin-ee/oisinpi.git",
    "https://GITHUB.com/oisin-ee/oisinpi/",
  ])("normalizes clone transport: %s", (origin) => {
    expect.hasAssertions();
    expect(repositoryIdentity(origin)).toBe("github.com/oisin-ee/oisinpi");
  });

  it.each([
    "/tmp/repo",
    "file:///tmp/repo",
    "https://github.com/",
    "https://github.com/owner/repo?query=1",
  ])("rejects non-repository origins: %s", (origin) => {
    expect.hasAssertions();
    expect(() => repositoryIdentity(origin)).toThrow(/origin|Invalid URL/u);
  });

  it("retains owner and path case rather than matching a short repository name", () => {
    expect.hasAssertions();
    expect(repositoryIdentity("git@github.com:Other/oisinpi.git")).toBe("github.com/Other/oisinpi");
  });
});
