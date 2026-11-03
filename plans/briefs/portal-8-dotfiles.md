# Brief: switch the dotfiles from Plannotator and pi-interview to Dogsled (web portal step 8)

**Outcome.** On every host, Pi loads Dogsled's own `interview` tool and plan mode. Plannotator and pi-interview are no longer installed. On macOS, `dogsled web` runs as a login service.

**Precondition.** Dispatch this unit only after these two releases exist. The lead fills in the versions:
- `@oisin-ee/pi-dogsled` <DOGSLED_VERSION>, with `interview`, `/plan` and `submit_plan`.
- `@oisin-ee/oisinpi` <OISINPI_VERSION>, with pi-todo listening for `dogsled:plan-approved`.

**Design.**
- Services in this repository are `[bootstrap.*]` entries in `conf.d/agent-ide/mise.toml`. Copy `[bootstrap.macos.launchd.agents.dogsled]` (line ~155), and read the comment above it about PATH.
- The portal is local-only, so it gets a macOS launchd agent only. It gets no Linux systemd unit.

**Edits.**
- **`conf.d/harness/settings/pi/settings.json`:**
  - Remove `npm:@plannotator/pi-extension@0.28.19` and `npm:pi-interview@0.13.0`.
  - Bump `npm:@oisin-ee/pi-dogsled` to <DOGSLED_VERSION>, and `npm:@oisin-ee/oisinpi` to <OISINPI_VERSION>.
  - Keep the `"interview": {"timeout": 1200}` block. Dogsled's fallback reads it.
- **`conf.d/harness/mise.toml:336`:** remove the `~/.pi/agent/plannotator.json` copy entry. Delete `conf.d/harness/settings/pi/plannotator.json`.
- **`conf.d/agent-ide/mise.toml`:**
  - Bump the pinned dogsled version, if one is pinned there.
  - Add `[bootstrap.macos.launchd.agents.dogsled-web]`. It has the same program, working_directory, run_at_load, keep_alive and environment as `dogsled`, with `args = ["x", "--", "dogsled", "web"]`.
  - Add a two-line comment: it serves the local portal on 127.0.0.1:7465.
- **Docs:** update any README or harness docs that mention Plannotator or pi-interview.

**Owned paths:** conf.d/harness/settings/pi/settings.json, conf.d/harness/settings/pi/plannotator.json, conf.d/harness/mise.toml, conf.d/agent-ide/mise.toml, conf.d/harness/README.md.

**Acceptance.**
1. `rg -n "plannotator|pi-interview" conf.d` prints nothing.
2. `jq . conf.d/harness/settings/pi/settings.json` parses.
3. The repository's own checks pass, using its pre-commit hook or check task.
4. Open a PR and report its number and SHA.
