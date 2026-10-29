import { StringEnum } from "@earendil-works/pi-ai";
import { Type } from "typebox";

const description = Type.String({
    description:
      "Optional detail behind the short content headline: context, approach, or acceptance in one or two sentences. Blank is none.",
  }),
  strings = Type.Immutable(Type.Array(Type.String())),
  scope = Type.Immutable(
    Type.Object(
      {
        generates: strings,
        owns: strings,
        reads: strings,
        repositoryId: Type.String(),
      },
      { additionalProperties: false },
    ),
  ),
  self = Type.Object({ kind: Type.Literal("self") }, { additionalProperties: false }),
  worker = Type.Object(
    { kind: Type.Literal("worker"), profile: Type.String() },
    { additionalProperties: false },
  ),
  assignment = Type.Union([self, worker]),
  command = Type.Object(
    { argv: strings, cwd: Type.String(), expected: Type.String(), kind: Type.Literal("command") },
    { additionalProperties: false },
  ),
  review = Type.Object(
    { artifact: Type.String(), criterion: Type.String(), kind: Type.Literal("review") },
    { additionalProperties: false },
  ),
  acceptance = Type.Union([command, review]),
  fields = Type.Object(
    {
      acceptance: Type.Optional(acceptance),
      assignment: Type.Optional(assignment),
      deliverable: Type.Optional(Type.String()),
      dependsOn: Type.Optional(strings),
      scope: Type.Optional(scope),
    },
    { additionalProperties: false },
  ),
  contract = Type.Immutable(fields),
  draftFields = Type.Object(
    {
      content: Type.String(),
      contract: Type.Optional(contract),
      description: Type.Optional(description),
      key: Type.Optional(Type.String()),
    },
    { additionalProperties: false },
  ),
  draft = Type.Immutable(draftFields),
  leaf = Type.Union([Type.String(), draft]),
  subtasks = Type.Immutable(Type.Array(leaf)),
  outcome = Type.Immutable(
    Type.Object(
      { content: Type.String(), description: Type.Optional(description), subtasks },
      { additionalProperties: false },
    ),
  ),
  item = Type.Union([leaf, outcome]),
  items = Type.Immutable(Type.Array(item)),
  evidence = Type.String({
    description: "Concrete acceptance result or integration-review evidence. Required for done.",
  }),
  parent = Type.String({
    description: "Parent ID. Exact content is accepted only for unmigrated legacy records.",
  }),
  start = Type.String({
    description: "Atomically start this ID or creation key with init/append/update.",
  }),
  task = Type.String({
    description:
      "Unit ID, or exact legacy content before migration. Children can use their ID without parent.",
  }),
  change = Type.String({
    description:
      "update only: set the open task or subtask description; blank clears it; omit to keep it. Never changes content.",
  }),
  parameters = Type.Immutable(
    Type.Object(
      {
        contract: Type.Optional(contract),
        description: Type.Optional(change),
        evidence: Type.Optional(evidence),
        items: Type.Optional(items),
        op: StringEnum(["init", "start", "done", "drop", "append", "view", "update"] as const),
        parent: Type.Optional(parent),
        start: Type.Optional(start),
        task: Type.Optional(task),
      },
      { additionalProperties: false },
    ),
  );

export { parameters };
