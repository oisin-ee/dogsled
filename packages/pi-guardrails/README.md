# @oisin-ee/pi-guardrails

Agent guardrails for [pi](https://pi.dev), Claude Code, and Codex. One set of policies checks every shell and edit call:

- **Git safety:** no `--no-verify`, no discarding uncommitted work, no stash push and pop in one command, worktrees only
  under `.worktrees/`, no on-the-fly identity.
- **Wait gate:** no `sleep`, `watch`, follow flags, `--wait`/`--watch`, or wait subcommands. Start long work in the
  background instead.
- **Agent typing:** no `herdr agent prompt` or `pane send-*` into another agent's terminal. Use `dogsled send`. For prompt answers, only `herdr agent send-keys` accepts `# dogsled: answer-prompt`; each use is recorded.
- **GitOps:** no imperative `kubectl`, `helm`, `argocd`, or `nix` mutations, and no `fsck`/`mkfs`.
- **Generated paths:** no hand-edits of lockfiles or generated files.

In pi, it also fills a missing shell timeout (120 s) and blocks one above 10 minutes, naming each tool's unit.
Shell text is parsed with `shfmt`, which must be on `PATH`. A command that cannot be parsed is blocked.

## Planning models

The guardrails do not restrict a model by its identity. The planning selections `tier/plan-claude` and
`tier/plan-gpt` keep the full policy set: a planning model may run Git commands, write and edit files,
and use the workflow tools.

Planning restrictions belong to Dogsled plan mode, which blocks writes outside markdown files while planning.

## pi

```bash
pi install npm:@oisin-ee/pi-guardrails
```

## Claude Code and Codex

Install the package globally, then run `agent-hook` as a `PreToolUse` hook for shell and edit tools. It reads the hook
payload on stdin and exits 2 with the reason on stderr to block.

```json
{ "type": "command", "command": "agent-hook", "timeout": 30 }
```
