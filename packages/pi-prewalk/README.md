# @oisin-ee/pi-prewalk

Prewalk starts with the active planner model. After the planning nudge, validated local task activation opens the handoff gate.
Without the task-provider capability, a successful `todo` call opens the gate instead. Imported workers need no bookkeeping call.
The first subsequent `edit` or `write` result switches the session to the target model, even if that result failed.
Nested calls through codemode also count. Without a callable `todo` tool, the gate is open.

## Config

Put `prewalk.json` in the pi agent directory:

```json
{
  "enabled": true,
  "target": { "model": "alias/luna", "thinking": "medium" }
}
```

A trusted project's `.pi/prewalk.json` replaces matching user keys. Prewalk reads both files again at each input.
The target needs a provider-qualified model ID and configured credentials. Alias models need the alias extension.

## Controls

Use `/prewalk` or `/prewalk on` to arm from the active model. Use `/prewalk off` to cancel without changing the model or config.
Use `/prewalk restart` to select the saved default and arm again. Use `/prewalk status` to inspect branch state and config errors.

The startup flags are `--prewalk`, `--no-prewalk`, and `--prewalk-into provider/model`.
Do not combine `--no-prewalk` with either arming flag.
Flags affect only the startup session. New sessions use `enabled` from config. Resumed sessions do not arm automatically.

For a dogsled child, add `"pi_args": ["--prewalk-into", "alias/luna"]` to its profile.
To suppress configured prewalk for that child, use `"pi_args": ["--no-prewalk"]`.

## State and context

The active branch stores arm, handoff, and off entries. The extension does not keep a separate in-memory protocol state.
After a handoff, `/new` restores the planner unless a later model or thinking selection replaced the handoff target.

pi 1.3.0 permits persistent context edits only at actionable boundaries.
Cancellation writes an off entry immediately. A context handler removes canceled planning steering before the next model request.
The next turn or settlement boundary records the persistent removal.

## Source

The protocol and prompts adapt [oh-my-pi prewalk](https://github.com/can1357/oh-my-pi/tree/e7fe318/packages/coding-agent/src),
including `session/prewalk.ts` and `prompts/system/prewalk-{plan,continue,checklist}.md`.
The original project uses the MIT license. This package keeps the one-shot handoff semantics and adapts them to pi's branch entries.
