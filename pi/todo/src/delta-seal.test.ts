import { describe, expect, it } from "vitest";
import { sealReference } from "./seal-reference-fixture.ts";

describe("delta seal provenance", () => {
  it("preserves lead references and the first sealing entry", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const comparison = await sealReference();
    expect(comparison.actual).toStrictEqual(comparison.expected);
    expect(comparison.seal).toMatchObject({ entryId: "seal" });
  });
});
