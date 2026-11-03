# Brief: fix the release web bundle step and release 0.25.1

**Outcome.** Tag `v0.25.1` builds and publishes. Tag `v0.25.0` built nothing and published nothing (release run 40474162483).

**Cause, confirmed by the lead.**
- In `.github/build-setup.yml`, the step "Bundle the web portal UI" has `working-directory: web`.
- The generated `.github/workflows/release.yml` (line ~136) dropped that key. So `dx bundle --web --release` ran at the workspace root, and dx built the root `dogsled` package for wasm32.
- That package has tokio with the `net` feature, so mio failed with "This wasm target is unsupported by mio".
- In `web/`, the build is correct: `cargo tree -p web --target wasm32-unknown-unknown -i mio` finds nothing. CI and moon run `cd web && dx bundle --web --release`.

**Edits.**
1. **`.github/build-setup.yml`:** make the bundle step work without `working-directory`. Use `run: dx bundle --web --release --package web` if dx 0.7.10 supports `--package` (check `dx bundle --help`). Otherwise use `run: cd web && dx bundle --web --release`.
2. **Regenerate the workflow:**
   - Run `dist generate` with cargo-dist 0.32.0 (`cargo binstall --no-confirm cargo-dist@0.32.0` if it's missing), so `release.yml` carries the new step.
   - Check that `dist plan` passes.
   - Do not hand-edit `release.yml`.
3. **`moon.yml` `web-build`:** if you chose `--package web`, use the same command there. That way CI exercises the exact command the release runs.
4. **Version bump to 0.25.1,** in a separate commit `chore(release): 0.25.1`:
   - Change the same nine files as release commit `cac4b07` (`git show cac4b07`), replacing 0.25.0 with 0.25.1. The CHANGELOG is the exception; see the next point.
   - CHANGELOG: add `## 0.25.1 - 2026-10-27` with one line: "Release builds bundle the web portal from the `web` package. The 0.25.0 tag built no archives." Keep the 0.25.0 entry.

**Owned paths:** .github/build-setup.yml, .github/workflows/release.yml, moon.yml, CHANGELOG.md, Cargo.toml, Cargo.lock, README.md, herdr-plugin.toml, package.json, package-lock.json, plugins/dogsled/.claude-plugin/plugin.json, plugins/dogsled/.codex-plugin/plugin.json.

**Acceptance.**
1. In a clean checkout, from the repository root, run the exact `run:` line from the regenerated `release.yml`. It exits 0 and writes `web/public/index.html`. Report the output tail.
2. `dist plan` passes.
3. `cargo metadata --format-version 1 --no-deps | jq -r '.packages[]|select(.name=="dogsled").version'` prints 0.25.1.
4. Push branch `release/0.25.1`, open a PR, and report its number and SHA. Do not create or push a tag.
