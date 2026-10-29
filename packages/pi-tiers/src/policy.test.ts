import { classify, target } from "./policy.ts";
import { describe, expect, it } from "vitest";
import { exhaustedUntil, parseQuota } from "./quota.ts";
import { Responses } from "./responses.ts";
import { parseConfig } from "./config.ts";

const ZERO = 0,
  UNAVAILABLE = 503,
  FIXTURES: readonly (readonly [string, string])[] = [
    ["HTTP 502", "gateway"],
    ["HTTP 503", "gateway"],
    ["no available server", "gateway"],
    ["Bad Gateway", "gateway"],
    ["Unable to connect", "gateway"],
    ["HTTP 429", "quota"],
    ["You have hit your ChatGPT usage limit", "quota"],
    ["usage limit", "quota"],
    ["rate_limit 429", "quota"],
    ["Codex error: Our servers are currently overloaded", "transient"],
    ["WebSocket closed 1006", "transient"],
    ["SSE headers timeout", "transient"],
    ["timeout", "transient"],
    ["Retry after 400ms", "transient"],
    ["Overloaded after processing 400 tokens", "transient"],
    ["Timeout after processing 401 tokens", "transient"],
    ["Retry after 403 milliseconds", "transient"],
    ["Timeout after processing 429 tokens", "transient"],
    ["Retry after 502 milliseconds", "transient"],
    ["Timeout after processing 503 tokens", "transient"],
    ["HTTP 500", "transient"],
    ["HTTP 504", "transient"],
    ["empty_stream: upstream stream closed before first payload", "transient"],
    ["internal_server_error", "transient"],
    ["unknown model", "request"],
    ["HTTP 400", "request"],
    ["HTTP 401", "request"],
    ["HTTP 403", "request"],
  ];

describe("error policy", () => {
  it.each(FIXTURES)("classifies %s as %s", { timeout: 5000 }, (message, expected) => {
    expect.hasAssertions();
    expect(classify(message)).toBe(expected);
  });

  it("uses correlated HTTP status but never the last unrelated response", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const responses = new Responses();
    responses.observe({ "x-request-id": "req-one" }, UNAVAILABLE);
    expect(classify("opaque error req-one", responses.status("opaque error req-one"))).toBe(
      "gateway",
    );
    expect(classify("opaque error", responses.status("opaque error"))).toBe("request");
    responses.clear();
    expect(responses.status("req-one")).toBeUndefined();
  });

  it("maps pools and rejects malformed config", { timeout: 5000 }, () => {
    expect.hasAssertions();
    expect(
      [
        "proxy/claude-opus",
        "proxy/gpt-worker",
        "proxy/minimax-m3",
        "proxy/MiniMax-M3.1-Flash",
        "other/model",
      ].map((reference) => target(reference).pool),
    ).toStrictEqual(["claude", "codex", "minimax", "minimax", "other"]);
    expect(() => parseConfig('{"fast":["unknown"]}')).toThrow("Invalid tier target");
  });
});

describe("quota policy", () => {
  it("requires an exhausted future window for every account", { timeout: 5000 }, () => {
    expect.hasAssertions();
    const windows = parseQuota(
      '{"version":1,"pools":[{"pool":"codex","account":"a","used_pct":100,"resets_ms":100},{"pool":"codex","account":"b","used_pct":50,"resets_ms":200}]}',
    );
    expect(exhaustedUntil(windows, "codex", ZERO)).toBeUndefined();
    expect(exhaustedUntil(windows, "missing", ZERO)).toBeUndefined();
    expect(parseQuota("not json")).toStrictEqual([]);
  });

  it(
    "uses the last blocked window per account and earliest available account",
    { timeout: 5000 },
    () => {
      expect.hasAssertions();
      const windows = parseQuota(
          '{"version":1,"pools":[{"pool":"codex","account":"a","used_pct":100,"resets_ms":100},{"pool":"codex","account":"a","used_pct":100,"resets_ms":300},{"pool":"codex","account":"b","used_pct":100,"resets_ms":200}]}',
        ),
        reset = 200;
      expect(exhaustedUntil(windows, "codex", ZERO)).toBe(reset);
      expect(exhaustedUntil(windows, "codex", reset)).toBeUndefined();
    },
  );
});
