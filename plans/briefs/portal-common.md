Everything you need is in this message. Base: origin/main at 570e7c121d24d426f2f99b93df0b95b0b6766e3f. It has the whole server API, the Pi tools and the Dioxus `web/` crate with empty slot components.

Other units run in parallel, so edit only your owned paths. If you need a change elsewhere, report it instead.

The Rust lints deny `unwrap`, `expect` and `panic`.

Checks: `proto install && moon run :web-lint :web-build :fmt-check :lint :test`.

Commit, push, and open a draft PR against main. You have publication authority for this branch. Report:
- The head SHA.
- The check results.
- The output of `gh pr checks <n>`.
