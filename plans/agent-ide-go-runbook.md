# Go-runbook: agent-ide cutover

Branches. They are not merged or deployed:

| Repo | Branch | Head | Content |
| --- | --- | --- | --- |
| oisin-ee/infra | `feat/agent-ide-nix-access` | `734b57aa` | Stage 1: netcup `allowed-users` gets `dev`. Nothing else changes. |
| oisin-ee/infra | `feat/agent-ide-nixos-floor` | `e3562436` | Stage 3: removes system Herdr, agent-config-sync, `agent:sync`, `peers.nix`, and dev packages. Includes stage 1. |
| oisin-ee/dotfiles | `feat/agent-ide-cutover` | `204b106` | Stage 2: authorized_keys block, Linux tool pins, `nix:` packages, user `herdr.service`, sync timer, `boot --local --restart-herdr`. |

The order prevents SSH lockout and a missing package window. The infra floor
removes keys and packages. Dotfiles must replace them before that deploy.

## Prerequisites (before go)

1. Make sure that momokaya-2 has `ORCA_PACKAGES_READ_TOKEN`. Run `secret login` (dotfiles). Then run `ssh momokaya-2-dev 'grep -c ORCA_PACKAGES_READ_TOKEN ~/.config/zsh/secrets.zsh'`. The result must be 1. Without it, the new Herdr server exits 78.
2. Find the cause of the failed `deploy-hosts.yml` runs on `main` (the last five, 2026-10-14 to 2026-10-19). Both infra stages deploy through that workflow. Note: `nix flake check --no-build` fails with `path '…-openclaw' is not valid`. This is not a blocker. nix-openclaw reads its `package-lock.json` during evaluation, which needs a build. With builds allowed, `netcup-openclaw-host-contract` evaluates on x86_64-linux (momokaya-2, `origin/main`, 2026-10-20).
3. Decide the dogsled restart on a version change. The open question is below.

## Stage 1: Nix access (infra, no service impact)

1. Merge `feat/agent-ide-nix-access`.
2. Deploy netcup through `deploy-hosts.yml`. momokaya-2 already allows every user.
3. Make sure that `ssh netcup-dev 'nix path-info nixpkgs#hello'` succeeds.

## Stage 2: dotfiles (additive, Herdr keeps running)

1. Merge `feat/agent-ide-cutover` into dotfiles `main`.
2. Run `boot` on the Mac. It boots the Mac, then each peer with `boot --local --pull`.
3. On each Linux host, make sure that these are true:
   - `~/.ssh/authorized_keys` has the `# >>> mise:agent-ide >>>` block with the two other keys.
   - `~/.nix-profile/bin/chromium` and `tmux` exist.
   - `systemctl --user is-enabled herdr.service` prints `enabled`. The unit is not started yet.
   - `systemctl --user list-timers dev.mise.agent-ide-sync.timer` shows the timer.
4. From each host, open a new SSH session to each other host on port 2222. All six paths must work.

The old system Herdr still runs. The new user unit is enabled, but it is not started.

## Stage 3: the floor (one host at a time, disruptive)

CAUTION: Do not deploy the floor while agents run on that host. The deploy removes the system `herdr.service` and stops every pane on it.

For netcup first, then momokaya-2:

1. Stop all agents on the host.
2. Deploy `feat/agent-ide-nixos-floor` to that host through `deploy-hosts.yml`. Merge it before the first host deploy.
3. Outside Herdr, run `ssh <host>-dev '~/dev/dotfiles/bin/boot --local --restart-herdr'`.
4. Run `scripts/verify-agent-services.sh` (infra) on the host.
5. Run `systemctl --user status herdr.service` as `dev`. Make sure that `$XDG_RUNTIME_DIR/herdr/npmrc` exists.
6. From the Mac, attach to the host with Herdr. Make sure that the protocol matches.

## Rollback

- Stage 1 and 2 are additive. Revert the merge commit and run `boot`.
- Stage 3: revert the floor merge and deploy. Run `systemctl --user disable --now herdr.service` as `dev` first. Two servers on one socket conflict.

## Open question: dogsled restart on a version change

mise `postinstall` runs before `mise upgrade --bump` saves the new pin. The
service runs `mise x -- dogsled serve`, so a restart from `postinstall` can start
the old version. Calling mise inside `postinstall` hangs on its lock. I found
no mise 2026.10.11 setting that restarts a user service on a tool version
change. The service-contract review returned no evidence.
Today, an operator restarts `dev.mise.dogsled` after a dogsled bump.
