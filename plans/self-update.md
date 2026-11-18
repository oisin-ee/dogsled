# Dogsled updates itself

Status: proposed. No part of this plan is approved or implemented.

## Goal

1. An installed Dogsled finds a newer release and installs it. The user runs no command.
2. When no newer release exists, Dogsled does nothing.
3. Dogsled updates its harness plugins to the same release.
4. After an update, the running Dogsled processes restart on the new version.
5. A user can turn the automatic update off.

Release creation does not change. A maintainer merges the release PR to make a release.

## How other apps do it

Each app checks its publisher's release feed. It compares the newest release with its own version. It installs at a safe
moment. The publisher only publishes. It does not know who installed the app.

| App | Check | Install |
| --- | --- | --- |
| [Sparkle](https://sparkle-project.org/documentation/customization/) | The app reads its feed every `SUScheduledCheckInterval`, default 1 day | `SUAutomaticallyUpdate` downloads in the background and installs when the app quits |
| [Electron](https://www.electronjs.org/docs/latest/tutorial/updates) | `autoUpdater.checkForUpdates()` on an interval, 60 s in the example | `quitAndInstall()` |
| [Tailscale](https://tailscale.com/kb/1067/update) | The client | The same installer that installed it, for example the Linux package manager |
| [cargo-dist / axoupdater](https://axodotdev.github.io/cargo-dist/book/installers/updater.html) | Compares GitHub Releases with the install receipt | Runs the release installer again |

Dogsled follows the same pattern. `dogsled serve` runs on every host, so it does the check.

## Evidence

Checked on 2026-11-18 at Dogsled `bd8192b` (`v0.38.0`).

| Finding | Evidence |
| --- | --- |
| Dogsled has five install channels: the binary, Pi packages on npm, a Claude marketplace, a Codex marketplace, and a Herdr plugin. The README pins each one to a tag. | `README.md` "Install" |
| dist generates no installer for Dogsled. | `dist-workspace.toml`: `installers = []` |
| The dist shell installer writes an install receipt. axoupdater reads the receipt and updates the binary. | [axoupdater README](https://github.com/axodotdev/axoupdater) |
| A binary from a package manager must be updated by that package manager. | [dist `install-updater`](https://axodotdev.github.io/cargo-dist/book/reference/config.html#install-updater) |
| The GitHub release is public before the npm packages publish. A check in that gap finds an incomplete release. | `.github/workflows/release.yml`: `host` runs `gh release create`, before `custom-publish-npm` |
| dist can create the GitHub release in the `announce` job, after every publish job. The race that dist warns of affects only npm or Homebrew installers that download release assets. Dogsled's npm packages contain only JavaScript. | [dist `github-release`](https://axodotdev.github.io/cargo-dist/book/reference/config.html#github-release) |
| Pi checks an unpinned npm package for a newer version. It does not check an exact version. `pi update <source>` updates one package. | Pi 1.5.2 `docs/packages.md`, `docs/cli.md` |
| Claude Code refreshes a marketplace with `autoUpdate` on and updates its plugins. `claude plugin marketplace update` and `claude plugin update` do the same on request. | [Claude Code plugin loading](https://code.claude.com/docs/en/plugins/loading#when-auto-update-runs); `claude plugin --help` 2.1.322 |
| Codex refreshes Git marketplaces with `codex plugin marketplace upgrade`. | `codex plugin marketplace --help` 0.165.0 |
| Herdr updates GitHub-installed plugins with `herdr plugin update`. | `herdr plugin --help` 0.9.5 |
| A Git tag does not move, so an update command on a tag-pinned install finds nothing new. | Pi `docs/packages.md`: "Git tags and commits are also pinned" |
| The service templates restart only after a failure. A clean exit stops the service. | `plugin/services/dogsled.service` `Restart=on-failure`; `dev.dogsled.plist` `KeepAlive.SuccessfulExit=false` |
| The service templates start Dogsled through `~/.local/bin/mise`. | `plugin/services/*` |
| The Pi extension warns when its version differs from `dogsled --version`. | `pi/index.ts` `versionMismatch` |
| Three Dogsled files name dotfiles, one consumer. | `dist-workspace.toml` line 25, `plugin/services/dev.dogsled.plist` line 3, `README.md` "Host services" |

## Design

### A release is complete before anyone can see it

dist creates the GitHub release in `announce`, after the publish jobs: npm, and the move of the `release` branch.
"Latest release" on GitHub then always means "every artifact and every channel is available". No consumer needs to check
npm or the branch.

### One release channel for the plugins

Four channels install from Git or npm. Each has a native update command. The command must find a ref that moves only on a
release:

- npm: the `latest` dist-tag moves only on a release. Install docs drop the exact version.
- Git (Claude, Codex, Herdr): a new branch `release`. A publish job moves `release` to the new tag before `announce`. A
  plain push only fast-forwards. Install docs use `release` as the ref. Tags stay fixed.
- Git cannot have `release` next to `release/<version>` branches. The owner deleted the nine old release-PR branches
  `release/0.23.1` to `release/0.31.1` on 2026-11-18. Their PRs were merged or closed.

### `dogsled update`

1. Read `https://api.github.com/repos/oisin-ee/dogsled/releases/latest`. When its version is not newer than
   `CARGO_PKG_VERSION`, skip step 2.
2. Update the binary through the installer that installed it:
   - A dist receipt exists: run axoupdater.
   - `[update] command` is set in `dogsled.toml`: run it with `{version}` replaced. This follows Pi's
     `selfUpdateCommand`. A mise user can set `["mise", "use", "-g", "github:oisin-ee/dogsled@{version}"]`.
   - Neither: print `dogsled <version> is available; update it with the tool that installed it.` and stop. Do not update
     plugins, so plugins and binary keep one version.
3. Update each harness that has a Dogsled plugin from the `release` channel:
   - Pi: `pi update npm:@oisin-ee/<package>` for each installed `@oisin-ee` package from this repository.
   - Claude: `claude plugin marketplace update dogsled`, then `claude plugin update dogsled@dogsled`.
   - Codex: `codex plugin marketplace upgrade dogsled`.
   - Herdr: `herdr plugin update dogsled --yes`.
   A harness that is not installed is skipped. A pinned plugin stays on its pin.
   Step 3 runs once for each binary version: after step 2 installs a release, or when the binary is the newest release
   and the version in `update-plugins` in the ledger directory differs. A failed step leaves that file unchanged, so the
   next check repeats step 3.
4. Print one line for each part that changed.

### Automatic check and restart

- `dogsled serve` runs step 1 at start and then every `[update] interval`. When step 1 finds a newer release and
  `[update] mode = "auto"`, `serve` runs `dogsled update`, then exits.
- The service templates restart after every exit (`Restart=always`, `KeepAlive = true`). The new process runs the new
  binary.
- `serve` starts no new lane after the install. It exits when the running lanes finish. Queued work is in the ledger.
- `dogsled web` compares its executable file with the file it started from, every 30 seconds. When the file changed, it
  exits, and its supervisor starts it again.
- Herdr does not start a plugin pane again, and `herdr plugin update` installs into a new directory. So the sidebar
  replaces its own process (`exec`) with the newer build: the file at its start path, or `bin/dogsled` in the plugin root
  that `herdr plugin list` names. The pane stays open.
- Running agent sessions keep their loaded plugins until they restart. The Pi extension already warns about the version
  difference.

### Settings

```toml
[update]
mode = "auto"          # "auto" installs, "notify" only logs the new version, "off" does nothing
interval_minutes = 15  # the other config durations are integers too
# command = ["mise", "use", "-g", "github:oisin-ee/dogsled@{version}"]  # optional
```

## Units

Each unit is one PR and one session. The order follows the dependencies.

### R1. Complete releases and a release channel

No dependencies.

- Owner files: `dist-workspace.toml`, `.github/workflows/release.yml` (generated by `dist generate`, never edited by hand),
  a new `.github/workflows/advance-release-branch.yml`, `README.md`.
- `dist-workspace.toml`:
  - `github-release = "announce"`.
  - `installers = ["shell"]` and `install-path = ["$DOGSLED_INSTALL_DIR", "~/.local/bin"]`.
  - `publish-jobs = ["./publish-npm", "./advance-release-branch"]`. The job moves `release` to the tag with a plain,
    fast-forward-only push. It never moves a tag.
- Remove the dotfiles comment on line 25.
- `README.md` "Install": the shell installer for the binary, the npm packages with no version, and `release` as the ref for
  Claude, Codex, and Herdr.
- Acceptance, at the next release: `releases/latest` changes only after npm has the version. `dogsled-installer.sh` is a
  release asset. On a clean host it installs `~/.local/bin/dogsled` and a receipt. `git rev-parse origin/release` equals the
  tag commit.

### R2. `dogsled update`

Depends on R1. axoupdater needs a receipt from the R1 installer.

- Owner files: `src/cli.rs`, a new `src/update.rs`, `src/config.rs` (`[update]`), `Cargo.toml` (`axoupdater` with the
  `blocking` feature), `README.md`.
- Behavior: steps 1 to 4 of the design.
- First check: confirm that `pi update npm:<package>` on an unpinned package installs the newest version, and that
  `codex plugin marketplace upgrade` also updates an installed plugin. If one of them does not, stop and report.
- Acceptance, on a host that runs an older release from the R1 installer: `dogsled update` installs the latest binary and
  moves each installed plugin to the same version. A second run prints nothing. On a mise install with no `command`, it
  prints the "available" line and changes nothing.
- Tests: one behavior test for the version comparison, and one for the choice of updater (receipt, command, neither).

### R3. Automatic check and restart

Depends on R2.

- Owner files: the `serve` loop in `src/main.rs` or the module that owns it, `src/web/mod.rs`, `src/sidebar/`,
  `plugin/services/dogsled.service`, `plugin/services/dev.dogsled.plist`, `plugin/services/dev.dogsled.web.plist`,
  `README.md`.
- Behavior: the "Automatic check and restart" section.
- Templates: restart after every exit. Run the binary from the install path, not through mise. Remove the dotfiles comment
  from `dev.dogsled.plist`. Remove the sentences in `README.md` that name dotfiles.
- First check: confirm that a `serve` restart loses no queued or running work. If it can lose work, `serve` must wait until
  no admission is in progress before it exits.
- First check: confirm that Herdr starts a plugin pane again after its process exits. If not, the sidebar keeps running the
  old version until the pane is opened again. Write that in the README.
- Acceptance: on a host with `mode = "auto"` and an older release, within one interval and with no command, `serve`, `web`,
  and the sidebar run the new version. With `mode = "off"`, nothing changes.

## Decisions

Decided by the owner on 2026-11-18.

| # | Decision | Result |
| --- | --- | --- |
| D1 | Default `mode` for a new install | `"auto"` |
| D2 | Default `interval` | 15 minutes. That is 4 unauthenticated GitHub API calls per hour per host. The limit is 60 per hour per address. |
| D3 | Name of the release channel branch | `release` |

## Out of scope

- dotfiles. It decides how its hosts install Dogsled. That plan is in the dotfiles repository.
- Herdr, Pi, Claude, and Codex updates themselves.
- Release creation and its CI gate.

## Risks

- Hosts update independently, within one interval. For that time, peers run different versions. Peer messages already
  accept older fields during upgrades.
- An update that fails halfway leaves the binary new and some plugins old. The next check repeats step 3, because
  `update-plugins` still names the old version. Each update command does nothing when its plugin is current.
- If `announce` fails after the publish jobs, `release` and npm are ahead of the latest GitHub release. Hosts do not
  update until a retry announces the release. A new plugin install can be one version ahead of the binary until then.
- A bad release reaches every automatic host within one interval. The fix is a new release. Tags never move back.
