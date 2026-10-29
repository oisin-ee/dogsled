import { describe, expect, it } from "vitest";
import { first } from "./contract-fixture.ts";
import { leased } from "./lease-fixture.ts";
import { runtimeFixture } from "./runtime-fixture.ts";

describe("provider shutdown", () => {
  it(
    "invalidates acquired capabilities and clears abandoned invocation leases",
    { timeout: 20_000 },
    async () => {
      expect.hasAssertions();
      const lease = await leased(false, false),
        old = lease.test.provider();
      try {
        await lease.test.session.extensionRunner.emit({
          reason: "reload",
          type: "session_shutdown",
        });
        expect(old.active()).toMatchObject({ error: { code: "PROVIDER_UNAVAILABLE" }, ok: false });
        await lease.test.todo({ evidence: "Reviewed after shutdown", op: "done", task: lease.id });
        expect(old.imported()).toMatchObject({
          error: { code: "PROVIDER_UNAVAILABLE" },
          ok: false,
        });
        expect(lease.test.provider().active()).toStrictEqual({ ok: true, value: null });
        await lease.finish();
      } finally {
        lease.close();
      }
    },
  );
});

describe("replacement session authorization", () => {
  it("does not inherit the closed runtime's activation", { timeout: 20_000 }, async () => {
    expect.hasAssertions();
    const original = await runtimeFixture(),
      replacement = await runtimeFixture();
    try {
      await original.create();
      await original.session.extensionRunner.emit({ reason: "new", type: "session_shutdown" });
      await replacement.call("bash", { command: "printf forbidden" });
      expect(replacement.provider().active()).toStrictEqual({ ok: true, value: null });
      expect(
        replacement.session.messages.filter((message) => message.role === "toolResult").at(first),
      ).toMatchObject({ isError: true });
    } finally {
      original.session.dispose();
      replacement.session.dispose();
    }
  });
});
