# Brief: consolidation step 2, todo-core owns the contract types

**Goal.** The Dogsled crate uses the todo-core contract types. `src/unit_transport.rs` keeps only Dogsled-only types. Wire JSON does not change.

**Spec.** `plans/oisinpi-consolidation.md`, section "Step 2". Its type table and decisions are binding.

**Base.** `origin/main` at the commit that adds this brief. Branch `feat/contract-owner`.

**Edits.**

1. `Cargo.toml`: add `schemars = "1.2.3"` to `[workspace.dependencies]`. Add `todo-core = { workspace = true, features = ["schema"] }` to the `dogsled` dependencies.
2. `crates/todo-core/Cargo.toml`: add feature `schema = ["dep:schemars"]` and an optional `schemars` dependency.
3. `crates/todo-core/src/{contract,binding,import}.rs`: add `#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]` next to each `Tsify` derive on the types of the table.
4. `src/unit_transport.rs`: delete the copies in the table. Re-export the todo-core types under the Dogsled names (`pub use todo_core::{TaskBinding as Binding, …}`) only where that keeps the change small. Move `Export::validate` and `UnitRef::validate` to free functions or an extension trait in this file. Compute the digest with `todo_core::export_digest`.
5. Callers in `src/` and `tests/`: change `u64` to `u32` for the fields in the table only. Change the test that uses `revision: u64::MAX` to `u32::MAX`.
6. Delete `tests/fixtures/unit-exports.json`. Point every `include_str!` of it at `crates/todo-core/tests/fixtures/unit-exports.json`.

**Owned paths.** `Cargo.toml`, `Cargo.lock`, `crates/todo-core/`, `src/`, `tests/`.

**Not owned.** `pi/`, `packages/`, `plans/`. If a change there seems necessary, stop and report.

**Stop and report** if a type of the table has a different serialized shape on the two sides, other than the width and `Stopped` cases the spec decides.

**Your checks.** `cargo fmt --all --check`, `cargo clippy -p todo-core --all-targets --all-features -- -D warnings`, `cargo clippy -p dogsled --all-targets`, `cargo test -p todo-core`, and `cargo test -p dogsled --test cli unit_requests`. Do not run the full test suite, a release, or an install. The lead runs the full acceptance.

**Publication.** Commit with `refactor(units): use the todo-core contract types` through the normal hook. Push the branch and open a draft PR. Do not merge, tag, or publish.
