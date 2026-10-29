import assert from "node:assert/strict";
import { test } from "vitest";
import { shellCallGuards, shellCalls } from "./shell-parser.ts";

test("shfmt supplies quoted words, assignments, and real command boundaries", () => {
  assert.deepEqual(shellCalls("A='one two' printf '%s' \"a|b\"; echo done\nprintf ''"), [
    ["A=one two", "printf", "%s", "a|b"],
    ["echo", "done"],
    ["printf", ""],
  ]);
});

test("cached calls remain immutable to classifiers", () => {
  const command = "echo immutable";
  shellCalls(command)[0]?.shift();
  assert.deepEqual(shellCalls(command), [["echo", "immutable"]]);
});

test("each call carries the plain pipeline that must succeed before it runs", () => {
  assert.deepEqual(shellCallGuards("cd /r && a x | b && c; d || e"), [
    null,
    null,
    null,
    [["a", "x"], ["b"]],
    null,
    null,
  ]);
  for (const command of ["! a | b && c", "a | b > f && c", "(a | b) && c", "a |& b && c"]) {
    assert.equal(shellCallGuards(command).at(-1), null, command);
  }
  // `a &` is a separate statement; only `b` gates `c`.
  assert.deepEqual(shellCallGuards("a | x & b && c").at(-1), [["b"]]);
});

test("shfmt parse failure carries the boundary failure", () => {
  assert.throws(() => shellCalls("echo 'broken"), /^Error: shell-parser:/u);
});
