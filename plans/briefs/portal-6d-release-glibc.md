# Brief: make the Linux release job install a dx that runs, and release 0.25.2

**Outcome.** Tag `v0.25.2` builds both archives and publishes the GitHub release and `@oisin-ee/pi-dogsled`.

**Cause, confirmed by the lead.** In release run 40476248819 (tag v0.25.1):
- The macOS build passed.
- `build-local-artifacts (x86_64-unknown-linux-gnu)` failed at "Bundle the web portal UI" with `dx: /lib/x86_64-linux-gnu/libc.so.6: version 'GLIBC_2.39' not found (required by dx)`.
- cargo-dist builds Linux artifacts on `ubuntu-22.04` (glibc 2.35), so the `dogsled` binary runs on older hosts. Keep that runner.
- The prebuilt `dx-x86_64-unknown-linux-gnu` for dioxus-cli 0.7.10, which `cargo binstall` downloads, needs glibc 2.39. CI passes only because it runs on ubuntu-24.04.

**Edits.**
1. **`.github/build-setup.yml`, the "Install dx" step:**
   - On Linux, build dx from source with `cargo install --locked dioxus-cli@0.7.10`. On macOS, keep `cargo binstall --no-confirm --locked dioxus-cli@0.7.10`.
   - Do this in the step's single `run:` script, branching on `uname -s`. cargo-dist dropped `working-directory` from build-setup steps before, so do not rely on step-level keys such as `if:`.
   - Add a short YAML comment that gives the glibc reason.
   - If the cargo-binstall action step is then needed only on macOS, keep it anyway. It is harmless on Linux.
2. **Regenerate the workflow:** run `dist generate` (cargo-dist 0.32.0), check that `dist plan` passes, and confirm that `release.yml` carries the new script exactly. Do not hand-edit `release.yml`.
3. **Version bump to 0.25.2,** in a separate commit `chore(release): 0.25.2`:
   - Change the same nine version files as `git show b9eff87`, replacing 0.25.1 with 0.25.2.
   - CHANGELOG: add `## 0.25.2 - 2026-10-27` with one line: "The Linux release build compiles dx from source, because the prebuilt dx needs glibc 2.39 and the release builds on Ubuntu 22.04. The 0.25.1 tag built no Linux archive and published nothing."

**Owned paths:** .github/build-setup.yml, .github/workflows/release.yml, CHANGELOG.md, Cargo.toml, Cargo.lock, README.md, herdr-plugin.toml, package.json, package-lock.json, plugins/dogsled/.claude-plugin/plugin.json, plugins/dogsled/.codex-plugin/plugin.json.

**Acceptance.**
1. **Prove the Linux path on glibc 2.35.** On this Mac, OrbStack provides `docker`. Run the exact "Install dx" script from the regenerated `release.yml` in `docker run --rm --platform linux/amd64 rust:1.99.1-bookworm` or an `ubuntu:22.04` image with rustup. Then run `dx --version`, and report the output tail.
   - If the container build is too slow to finish, report how far it got.
2. `dist plan` passes, and `cargo metadata` reports dogsled 0.25.2.
3. Let the pre-commit hook run, and do not bypass it.
4. Push branch `release/0.25.2`, open a PR, and report its number and SHA. Do not create or push a tag.
