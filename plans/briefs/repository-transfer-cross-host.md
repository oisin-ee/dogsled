# Cross-host transfer delivery

Complete without the plan. Base: branch `feat/transfer-commands` on origin, at the commit "feat(transfer): apply received changes to working files". Work on netcup.

## Outcome
`dogsled transfer send` reaches an agent on another fleet host. That host stores the transfer once and puts one notice in the recipient's inbox. `list`, `show`, `apply`, and `drop` then work there unchanged.

## Cause
`src/transfer/delivery.rs:309-313` refuses a recipient on another host. A transfer is a directory `transfers/<id>/` in one host's state directory, and no command carries it to a peer.

## Edits
- `src/node.rs:146`: add `#[serde(default, skip_serializing_if = "Option::is_none")] pub transfer: Option<u32>` to `SnapshotReport`. Add `pub const TRANSFER_VERSION: u32 = 1;` beside `PLACEMENT_VERSION`. Set it in `src/observation/collect.rs:31`.
- `src/transfer/peer.rs` (new, `pub(crate) mod peer;` in `src/transfer.rs`): `push` for the sender and `receive` for the peer.
- Envelope on stdin: one JSON header line, a newline, then the bytes of each part in header order. Header: `{version: 1, delivery, parts: [{name, bytes, sha256}]}`, `deny_unknown_fields`. A part name is `manifest.json`, `payload.bundle`, or `files/<name>` with one normal path component. No bytes go in arguments, errors, or logs.
- `push(fleet, host, id)`: read `host snapshot` (exemplar `src/hosts.rs:773-780`) and refuse when `transfer != Some(TRANSFER_VERSION)`, before anything is sent. Then send with `Shell::run_input_bounded` over `fleet.remote_argv(host, ["transfer-put", "--input", "-"])` and `SETUP_LIMIT` (exemplar `src/leads/routing.rs:88-110`). Reply: `{"stored": {id, notice}}` or `{"refused": {reason}}` with a fixed reason code. Neither end forwards free text from the other.
- `receive(shell, ledger, input: &[u8])`: validate the id (`transfer::validate_id`), the header, every part name, size, and digest, and 10 MiB per `files/` part, before any write. Under `transfer::locked`: if `transfers/<id>/received.json` exists and holds the same header digest, return its saved reply; another digest refuses `conflict`. Else write the parts and `delivery.json` to `.{id}.tmp`, validate the manifest with `transfer::load` rules, rename into place, put the inbox record, then write `received.json`.
- Inbox record on the peer: id `transfer-<id>`, kind `transfer`, terminal and pane from `delivery.to`, text from `delivery::notice` with the peer's directory. Add `pub(crate) fn deliver(ledger, shell, record)` in `src/inbox.rs` that calls `put_checked` (`:269`). If it refuses (no reader, terminal gone), remove the stored directory and reply `refused`.
- `src/transfer/delivery.rs` `send`: replace the refusal at `:309`. Store locally as now. For a remote recipient, call `peer::push` in place of `inbox::send`. `refused`: `discard` and return the reason. No reply or an invalid reply: send once more with the same id. Still none: keep the local transfer and return "delivery to <host> is uncertain; transfer <id> is kept". Make `notice` `pub(super)`.
- `src/cli.rs:768` `HostCommand`: hidden `TransferPut { #[arg(long, value_parser = ["-"])] input: String }`. `src/main.rs`: dispatch it before host routing, as `HostCommand::Lead` at `:233`. Read stdin as bytes.

Do not add retention, an outbox entry, or streaming. The envelope is held in memory at both ends; say so in a comment.

## Tests in `tests/cli/transfer.rs`
Write your own ssh fixture there: a script that runs `"$DOGSLED_BIN"` with `HOME="$RIG/peer-home"` and `DOGSLED_STATE="$RIG/peer-state"` and passes stdin through (model: `peer_transport` at `tests/cli/lead.rs:407`). Do not edit `tests/cli/lead.rs`.
1. A file transfer and a change transfer to a peer agent: stored under `peer-state/transfers/<id>/`, one inbox record `transfer-<id>` on the peer, `transfer show` and `transfer apply` succeed there with `DOGSLED_STATE` set to the peer state.
2. A lost first reply: the second send returns the saved reply; one stored transfer, one inbox record.
3. A peer snapshot without `transfer`: refused, and `ssh.log` has no `transfer-put` call.
4. One changed payload byte, an oversized attachment part, a part named `../x`, and a second envelope with the same id and other bytes: each refuses and stores nothing new.
5. A peer recipient without an inbox reader: `refused`, nothing stored on the peer, and the sender's local transfer and ref are gone.
6. Two failed replies: the sender keeps its local transfer and reports "uncertain".

## Paths
Owned: `src/transfer.rs`, `src/transfer/`, `src/node.rs`, `src/observation/collect.rs`, `src/inbox.rs`, `src/cli.rs`, `src/main.rs`, `tests/cli/transfer.rs`. Generated: `web/public/`, never committed. If another file must change, stop and reply `blocked`.

## Acceptance
`cargo test --locked --test cli transfer::` and `cargo test --locked --bin dogsled -- transfer:: inbox::` pass with nonzero tests. Report the tested SHA.

## Checks and authority
Run `moon run dogsled:web-build` once if `web/public/index.html` is missing. Allowed: `cargo fmt --all --check`, `cargo clippy --locked --all-targets -- -D warnings`, and the acceptance commands.
Run each long command as a direct `bash` call with `timeout: 600`, never inside `codemode`. A call that ends at the limit can run again, up to three times.
The commit hook stays enabled; on netcup it fits in one call. If it fails on a test outside your paths, reply `blocked` with the test name and panic line.
Commit as `feat(transfer): deliver transfers to another host`. Push branch `feat/transfer-cross-host` to origin. No force, PR, merge, release, or install.
Reply: full head SHA, changed paths, commands and results, blockers, unverified scope.
