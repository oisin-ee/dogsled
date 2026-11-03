# Plan: the agent-ide distribution (dotfiles owns the agent stack, Nix owns the bare machine)

Status: revision 2, after the feedback "I want one place where my stack exists."
Repositories touched: `~/dev/dotfiles` (all the stack config) and `~/dev/infra` (deletions and one import). wrangle and oisinpi stay as code repositories. This folder pins their releases.

Coordination: pane wAH:pX owns the wrangle board-widget restore. Its release chain ends with a dotfiles wrangle pin bump. Step 2 below moves that pin from `conf.d/harness/mise.toml:71` to `conf.d/agent-ide/mise.toml`. Tell wAH:pX before step 2 lands.

## Context

Goal: one place defines the whole Herdr + wrangle + Pi orchestration stack. A new device gets the stack from one `boot`.

Today the stack is spread over seven places:

| Piece                            | Lives in today                                                                                           |
| -------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Herdr pin                        | dotfiles `conf.d/host/mise.toml:15` (0.9.5) **and** infra `modules/agent-host/herdr.nix:13` (0.9.4)      |
| Herdr server                     | infra `systemd.services.herdr` (Linux), the client autostart (Mac)                                       |
| wrangle pin and wrangle profiles | dotfiles `conf.d/harness/mise.toml:71, 324`                                                              |
| wrangle service and plugins      | dotfiles `conf.d/host/mise.toml:90, 195`                                                                 |
| Herdr and wrangle config         | dotfiles `home/.config/herdr`, `home/.config/wrangle`                                                    |
| Host list                        | infra `modules/agent-host/peers.nix`, `home/.ssh/config.tera`, the `case` in `conf.d/host/mise.toml:137` |
| Config sync                      | infra `agent-config-sync` timer and the `agent:sync` task (`infra/mise.toml:14`)                         |

Live evidence (netcup, 2026-10-19): panes resolve `herdr` to mise 0.9.5. The server runs `/nix/store/...herdr-0.9.4`. The client and the server already differ.

## Approach

### One folder: `dotfiles/conf.d/agent-ide/`

```
conf.d/agent-ide/
├── mise.toml        pins (herdr, wrangle), services, sync timer, plugin task, packages
├── hosts.toml       the host inventory: [bootstrap.remote.hosts.<name>]
├── herdr/           config.toml (moved from home/.config/herdr)
├── wrangle/         config.toml + profiles/ (moved from home/.config/wrangle, harness settings)
└── ssh/             peers.conf.tera (SSH host blocks generated from hosts.toml)
```

Everything that defines the stack, and changes when the stack changes, is in this folder. Nothing that defines the stack stays in infra, `home/` or `conf.d/harness`.

- **Inventory.** `hosts.toml` uses mise's own `[bootstrap.remote.hosts]` schema (`host`, `user`, `port`, `tags`). Each host also records its public key. Tested: `mise config get -f hosts.toml bootstrap.remote.hosts` returns the table, and `builtins.fromTOML` reads the same file in Nix. `bin/peers` reads it with `mise config get` instead of awk on `peers.nix`.
- **Herdr.** One pin. On Linux, `[bootstrap.linux.systemd.units.herdr]` runs `~/.local/share/mise/shims/herdr server`. The unit file names no version, so a pin bump does not restart it. mise restarts a unit only when its file differs ([mise systemd apply](https://mise.jdx.dev/bootstrap/systemd.html#preview-and-apply)). The new version starts at the next deliberate `boot --restart-herdr`. A `[bootstrap.files]` drop-in sets the Herdr-only `TasksMax`, because mise units have no `TasksMax` key. The npm token comes from a `[bootstrap.secrets]` input, the same pattern as `conf.d/harness/mise.toml:365`.
- **wrangle.** The pin, the `serve` service, the Herdr, Claude and Codex plugins, the config and the profiles all move here.
- **Sync.** A systemd user timer (`on_calendar = "*:0/5"`, `persistent = true`) runs `boot --local --pull`. It replaces infra's timer and `agent:sync`.
- **Packages.** mise `[tools]` pins first. `nix:` entries only for library-heavy packages: `ungoogled-chromium`, `gcc`, `gnumake`, `openssl`.

Pi and `@oisin-ee/oisinpi` stay in `conf.d/harness`. One agent on any machine needs Pi without Herdr. The layer split from `plans/repo-boundary.md` stays: harness = one agent anywhere, agent-ide = many agents across hosts.

### Why Nix stays at all

Nix stays only because netcup and momokaya-2 run NixOS. Checked on netcup: `/etc/systemd/system` and `/etc/ssh/sshd_config` are links into the Nix store, and `/lib64/ld-linux-x86-64.so.2` is the nix-ld shim. NixOS rebuilds `/etc` on each switch, so mise cannot own system files there.

On those hosts, infra keeps a **generic Linux floor**. It holds nothing specific to the stack:

- the `dev` account with `linger`, its login shell, and authorized keys read from `hosts.toml`
- sshd on tailnet port 2222, nix-ld, and `MISE_ALL_COMPILE=false`
- the cgroup limits on the user slice, the firewall, and k3s

You plan clean Linux boxes with Nix installed, not NixOS. On those, mise bootstrap owns the floor too: accounts, packages, sshd file and firewall. Only `loginctl enable-linger` stays a one-time manual step, because mise does not enable it ([mise systemd troubleshooting](https://mise.jdx.dev/bootstrap/systemd.html#troubleshooting)). A Nix install on a non-NixOS host leaves `/etc` writable, so no infra module is needed.

### How infra reads `hosts.toml` (decided)

infra adds dotfiles as a `git+ssh` flake input. infra reads `conf.d/agent-ide/hosts.toml` with `builtins.fromTOML`. Deploy CI gets a read-only deploy key for `oisin-ee/dotfiles`.

## Decisions (2026-10-19)

1. Layer name: `conf.d/agent-ide`.
2. Packages: `[tools]` pins first. `nix:` only for library-heavy packages.
3. Herdr restart: only through `boot --restart-herdr`.
4. One place: `conf.d/agent-ide` holds every pin, service, config file and the inventory. NixOS hosts keep a generic floor in infra.
5. infra reads the inventory through a `git+ssh` flake input, with a read-only key in deploy CI.

## Risks

- Cutover kills the running agents once, when the Nix Herdr unit stops and the user unit starts. Do step 5 when no agents run.
- Order on each host: the user unit must exist before infra deletes the system unit. Otherwise a reboot between the two leaves no Herdr server.
- The Herdr unit `PATH` must name only stable directories. Copy the list from `herdr.nix`.
- `AGENT_BROWSER_EXECUTABLE_PATH` moves from `/etc/profiles/per-user/dev/bin/chromium` to `~/.nix-profile/bin/chromium`.

## Files

- dotfiles: `conf.d/host/` → `conf.d/agent-ide/` (plus the new `hosts.toml`, `herdr/`, `wrangle/`, `ssh/`), `conf.d/harness/mise.toml` (the wrangle pin, profiles and Herdr comment go), `home/.config/herdr`, `home/.config/wrangle`, `home/.ssh/config.tera` (includes `peers.conf`), `bin/peers`, `bin/boot`, `config.toml`, `README.md`, `AGENTS.md`, `.github/workflows/ci.yml`, `plans/repo-boundary.md`.
- infra: `flake.nix` (dotfiles input), `modules/agent-host.nix` (keys from `hosts.toml`, the package list goes), `modules/agent-host/herdr.nix` (deleted), `modules/agent-host/peers.nix` (deleted), `mise.toml` (`agent:sync` goes), `hosts/momokaya-2/configuration.nix` (the guards go), `scripts/smoke-agent-services.sh`, `.github/workflows/deploy-hosts.yml` (deploy key).

## Reuse

- `bin/boot`: this host, then every peer, under one `flock`.
- `bin/peers`: finds this host in the list by its SSH key. Only its reader changes.
- `[bootstrap.services.wrangle]` and `agent:plugins`: they move as they are.
- `[bootstrap.secrets]` with the 0600 file pattern (`conf.d/harness/mise.toml:365-407`).
- mise's `[bootstrap.remote.hosts]` schema for the inventory.

## Steps

Each step ships on its own.

- [ ] 1. **Rename.** `git mv conf.d/host conf.d/agent-ide` and update every reference. Run `boot`.
- [ ] 2. **Gather.** Move the wrangle pin, profiles, and Herdr and wrangle config into the folder. Tell wAH:pX first.
- [ ] 3. **Inventory.** Add `hosts.toml`. Generate SSH host blocks, Herdr machines and Mac authorized keys from it. Change `bin/peers` to read it. Add the infra flake input, the deploy key, and read keys with `fromTOML`. Delete `peers.nix` and the hard-coded `case`.
- [ ] 4. **Packages.** Add the `[tools]` pins and the four `nix:` entries. Run `boot`. Then remove `users.users.dev.packages` from infra and deploy.
- [ ] 5. **Herdr server and sync.** Add the unit with `start = false`, the TasksMax drop-in, the secret npmrc, the timer and `boot --restart-herdr`. Then, one host at a time with no agents running: delete `herdr.nix`, `agent-config-sync` and `agent:sync` from infra, deploy, and run `boot --restart-herdr`.

## Verification

- After each step: `mise bootstrap --dry-run` and `mise bootstrap status --missing` on each host.
- `rg -l 'herdr|wrangle' ~/dev/infra/modules` finds nothing stack-specific after step 5.
- On every host, `herdr --version` in a pane equals `readlink /proc/$(pgrep -f 'herdr server')/exe`.
- Run `boot` twice with a changed Herdr pin: `systemctl --user show dev.mise.herdr -p ActiveEnterTimestamp` does not change.
- `wrangle status` lists every host in `hosts.toml` as up.
