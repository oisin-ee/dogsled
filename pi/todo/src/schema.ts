import type { Static } from "typebox";
import { StringEnum } from "@earendil-works/pi-ai";
import { Type } from "typebox";

// $defs keys are wire names: each reference repeats one, and the declaration budget is 2000 chars.
const description = Type.String(),
  optionalString = Type.Optional(Type.String()),
  strings = Type.Immutable(Type.Array(Type.String())),
  stringsRef = Type.Unsafe<Static<typeof strings>>(Type.Ref("#/$defs/sa")),
  scope = Type.Immutable(
    Type.Object(
      {
        generates: stringsRef,
        owns: stringsRef,
        reads: stringsRef,
        repositoryId: Type.String(),
      },
      { description: "Repo paths: src/" },
    ),
  ),
  self = Type.Object({ kind: Type.Literal("self") }),
  worker = Type.Object({ kind: Type.Literal("worker"), profile: Type.String() }),
  assignment = Type.Union([self, worker]),
  command = Type.Object({
    argv: stringsRef,
    cwd: Type.String(),
    expected: Type.String(),
    kind: Type.Literal("command"),
  }),
  review = Type.Object({
    artifact: Type.String(),
    criterion: Type.String(),
    kind: Type.Literal("review"),
  }),
  acceptance = Type.Union([command, review]),
  contract = Type.Immutable(
    Type.Object({
      acceptance: Type.Optional(acceptance),
      assignment: Type.Optional(assignment),
      deliverable: optionalString,
      dependsOn: Type.Optional(stringsRef),
      scope: Type.Optional(scope),
    }),
  ),
  contractRef = Type.Unsafe<Static<typeof contract>>(Type.Ref("#/$defs/ct")),
  draft = Type.Immutable(
    Type.Object(
      {
        content: Type.String(),
        contract: Type.Optional(contractRef),
        description: Type.Optional(description),
        key: optionalString,
      },
      { additionalProperties: false },
    ),
  ),
  leaf = Type.Union([Type.String(), draft]),
  leafRef = Type.Unsafe<Static<typeof leaf>>(Type.Ref("#/$defs/lf")),
  subtasks = Type.Immutable(Type.Array(leafRef)),
  outcome = Type.Immutable(
    Type.Object(
      { content: Type.String(), description: Type.Optional(description), subtasks },
      { additionalProperties: false },
    ),
  ),
  item = Type.Union([leafRef, outcome]),
  items = Type.Immutable(Type.Array(item)),
  parameters = Type.Immutable(
    Type.Object(
      {
        contract: Type.Optional(contractRef),
        description: Type.Optional(description),
        evidence: optionalString,
        items: Type.Optional(items),
        op: StringEnum(["init", "start", "done", "drop", "append", "view", "update"] as const),
        parent: optionalString,
        start: optionalString,
        task: optionalString,
      },
      { $defs: { ct: contract, lf: leaf, sa: strings }, additionalProperties: false },
    ),
  );

export { parameters };
