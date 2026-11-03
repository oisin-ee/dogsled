# Managed Dogsled 0.23.3 rollout

## Outcome
Every configured host uses released Dogsled 0.23.3 for its binary, managed harness packages, Herdr plugin, and running host service. Existing agents and user work remain intact.

## Verified owners and baseline
- Dogsled publication: version-bump PR must merge before a v0.23.3 tag is pushed. .github/workflows/release.yml produces macOS ARM64 and Linux x64 archives; publish-npm.yml publishes @oisin-ee/pi-dogsled. Confirm both before updating fleet pins.
- Canonical managed configuration: /Users/oisin/dev/dotfiles at 94e6998e5198ae89d1c728243be3230d8f6301a1, origin https://github.com/oisin-ee/dotfiles.git. Read that repository's AGENTS.md before changing it.
- conf.d/agent-ide/mise.toml line 19 owns the github:oisin-ee/dogsled v0.23.2 tool pin.
- conf.d/harness/settings/pi/settings.json owns the explicit npm:@oisin-ee/pi-dogsled@0.23.2 package pin. Do not bump @oisin-ee/oisinpi or other packages.
- mise.lock owns platform release archive URLs/checksums. Regenerate through the pinned mise, only for Dogsled; preserve unrelated tool entries. Do not hand-edit generated lock data.
- conf.d/agent-ide/mise.toml agent:plugins derives Herdr, Claude, and Codex plugin tags from dogsled --version. No additional hard-coded plugin tag edit is needed.
- All three hosts currently use 0.23.2. Their dotfiles checkout revision is 94e6998 and tracked trees were clean at this preflight.

## Required delegation boundary
The configuration edit must be delegated to a writer operating in the dotfiles repository, not performed by this lead. Current Dogsled unit binding validates the invoking checkout origin against the unit repositoryId and forbids a cwd override. Do not forge a prepared envelope, weaken identity checks, or hide cross-repository edits inside a Dogsled-scoped unit. A repository-local dotfiles lead/session must own its writer and acceptance.

## Managed installation
Use the repository's supported bootstrap path: ~/dev/dotfiles/bin/boot --local --pull, which fast-forwards the managed checkout and calls its pinned bin/mise bootstrap --yes. For local already-integrated main, boot --local suffices. Apply the exact committed pins on each host.
Do not run an unqualified full boot: it upgrades all global tools with --bump, which is unrelated to this repair. Do not use mise use -g or manually install binaries or npm packages to override canonical managed pins.
Never pass --restart-herdr. Herdr hosts active agents; restarting it would end those agents. Managed bootstrap installs integrations without a Herdr server restart.

## Fleet and service proof
Host list comes from /Users/oisin/dev/dotfiles/bin/peers: local oisin-mac, netcup-dev, momokaya-2-dev. Plain momokaya-2 is not the configured SSH user target.
Linux service: dev.mise.dogsled.service, ExecStart=~/.local/bin/mise x -- dogsled serve, KillMode=control-group. macOS service: gui/$(id -u)/dev.mise.dogsled, program ~/.local/bin/mise with x -- dogsled serve.
Inspect active preparations/launches before a Dogsled-only service restart. Preserve their receipts and queued work; do not replay failed historical launches. Herdr agents are not to be restarted.
After each host installs, verify dogsled --version from home, installed Pi package version, Herdr plugin requested ref and embedded bin/dogsled version, and Claude/Codex plugin versions. Verify the service process's executable, not only PATH: Linux readlink /proc/$MainPID/exe and /proc/$MainPID/exe --version; macOS launchctl PID and process executable path.
If bootstrap leaves an old Dogsled service process, a service-manager Dogsled-only restart is part of activation after the preparation safety check. Never restart Herdr or unrelated services.
Confirm fresh post-upgrade host snapshots and service liveness with dogsled status. Exercise only read-only diagnostic commands or isolated scratch fixtures; no user launch replay.

## Acceptance and reporting
Name the published release/tag/commit, successful release workflow and npm version, committed managed pin revision, and each host's actual binary/package/plugin/service version. Report any incomplete surface or stale current-session extension explicitly. Do not claim an already-running Pi session reloaded its extension unless it did.
