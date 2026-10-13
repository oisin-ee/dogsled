---
name: wrangle
description: Read host status and reserve capacity before spawning a coding agent with Shepherdr.
---

# Wrangle: host status and placement

Use the CLI for placement. Use Shepherdr's `agents` tool for spawning and parent/child messages.
Wrangle registers no Pi tool or widget in 0.5.

## Read status

```sh
wrangle status
wrangle status --json
wrangle status --brief
```

Every enabled Herdr host stays visible. `service: down` means the snapshot is missing, unavailable, or older than 30 seconds.
A down host is not eligible. `status --line` uses `▲` for ineligible hosts and `?` for failed host reads.

The Pi extension adds brief status before each agent turn. It does not spawn or monitor children.

## Place and spawn

1. Reserve capacity for the child:

   ```sh
   wrangle place --json
   ```

   For a writer, prepare its worktree on the selected host:

   ```sh
   wrangle place --branch feat/unit-a --repo /path/to/repo --json
   ```

2. Read `host.id`, `host.label`, and any returned `path` and `pane_id`.
3. Spawn through `agents`, using that machine and the prepared pane or path. Omit `machine` for `local`.
4. Use `agents` for messages, follow-up, and completion. Do not call a removed `wrangle spawn` or ticket API.

`--host H` pins placement by id or label. Without it, wrangle selects the host with the most headroom.
Every reservation expires only at its TTL, 90 seconds by default. This covers the lag of the one-minute load average.

If `place` exits 1, it returns `{"full": true, "hosts": [...]}`.
Inspect the host reasons and report the capacity limit. There is no queue or automatic retry in 0.5.
Exit 2 means an error. Read the error before another attempt.

## Prepare hook

The hook runs in the repository's main checkout on the selected host.
Only host policy sets `hooks.prepare`. Repository setup belongs to Worktrunk configuration.
Use `{branch}` and `{base}` as argument placeholders. Pass `--base main` to select a base.
Without `--base`, wrangle omits a word that is only `{base}`. Other placeholders are errors.
The hook returns JSON with `path`, `pane_id`, or both.

## Service

Each host needs `wrangle serve`. Its default interval is ten seconds.
Dotfiles owns service installation. Do not install, replace, or restart host services without permission.
For an authorized one-shot measurement, run `wrangle serve --once`.
