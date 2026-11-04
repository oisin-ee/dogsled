# Config-driven repo list with `dogsled repo add/remove/list`

Plan: `plans/fleet-leads.md`, stage 1a. Base: `origin/main` a0ce8764258c4a70d968a53522a4e6f1b9c5ef6c.

## Outcome

`config.toml` holds a `[[repos]]` list. `dogsled repo add|remove|list` edits and prints it, and keeps the file's comments. `dogsled doctor` checks those repos.

## Design

- No repo list exists. `cleanup.repositories` (`src/config.rs:71`) is a list of paths that only doctor reads (`src/cleanup.rs:453`).
- A lead on any host needs the origin and the home-relative path, the same pair `checkout::ensure` uses (`src/checkout.rs:90`). So each entry stores both.
- Schema, in `Policy` (`src/config.rs:14`, `deny_unknown_fields` stays):
  ```toml
  [[repos]]
  path = "~/dev/dogsled"                         # home-relative, `~/` prefix
  origin = "git@github.com:oisin-ee/dogsled.git" # as `git remote get-url origin` prints it
  ```
- Two entries are the same repo when `checkout::identity(origin)` (`src/checkout.rs:167`) is equal.
- `cleanup.repositories` stays as it is. Its removal is a later change after dotfiles migrate.

## Edits

1. `src/config.rs`: add `pub repos: Vec<Repo>` to `Policy` with `#[serde(default)]`, and `Repo { path, origin }`. Add a parse test.
2. New `src/repos.rs`:
   - `add(shell, home, config_path, dir)`: run `checkout::discover(shell, dir, home, true)` (`src/checkout.rs:19`). Store `~/` + `relative` and `origin`. Refuse a duplicate identity with `Error::Invalid`.
   - `remove(config_path, selector)`: the selector is an origin (match by identity) or a path. Refuse when nothing matches.
   - `list(policy)`.
   - Edit the file with the `toml_edit` crate so that comments and key order survive. Write through the path from `config::path` (`src/config.rs:116`); it may be a symlink, and the write must follow it. A missing file is created.
3. `src/cli.rs`: add `Repo(RepoCommand)` with `Add { path: Option<String> }`, `Remove { selector: String }`, `List`. `add` defaults to the current directory.
4. `src/main.rs` `run()` (line 200): hold the resolved config path in a local (`cli.config` or `config::default_path()`). Use it for `config::load`, then dispatch `Command::Repo` in the same `match` as `Command::Quota`, passing that path. `list` honours `--json` like the other commands (`crate::print`).
5. `src/node.rs:312`: add `pub repos: &'a [crate::config::Repo]` beside `cleanup`. Set it at every `Node {` literal; `rg -n 'Node \{' src` finds them (`src/main.rs`, `src/hosts.rs`, `src/service.rs`, `src/node.rs`, `src/cleanup.rs`, `src/work_tests.rs`, `src/loss_tests.rs`). Use `&policy.repos` where a policy exists, and `&[]` in tests that build a bare `Cleanup`.
6. `src/cleanup.rs:453`: doctor checks the `node.repos` paths with the same loop body as `cleanup.repositories`. Share that body; do not copy it. The detail text names `repos`, not `cleanup.repositories`.
7. `Cargo.toml`/`Cargo.lock`: add `toml_edit` (latest release compatible with the `toml = "1.1.6"` pin).
8. `README.md` "Configuration" (line 582): document `[[repos]]` and the three commands.
9. `tests/cli.rs`: a temp git repo with an origin, and `DOGSLED_CONFIG` pointing at a temp file that has a comment. `repo add` writes the entry and keeps the comment. `repo list --json` shows it. A second `add` fails. `repo remove` by origin deletes it.

## Owned paths

`src/config.rs`, `src/repos.rs`, `src/main.rs`, `src/cli.rs`, `src/node.rs`, `src/hosts.rs`, `src/service.rs`, `src/cleanup.rs`, `src/work_tests.rs`, `src/loss_tests.rs`, `Cargo.toml`, `Cargo.lock`, `README.md`, `tests/cli.rs`.

## Acceptance

`cargo test repo` and `cargo test config::` pass, and the new CLI test passes.

## Checks

`moon run :fmt-check :lint`, then `cargo test repo`, `cargo test config::`, and `cargo test cleanup::`.
