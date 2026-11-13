# Bounded observation transport

**Base:** `110e66895a353b1316a96c2b81eadddc6435c859` (`origin/main`, v0.36.0 plus release commit).

## Failure

On 2026-11-13 `dogsled serve` on the Mac exited with code 2 on every start. launchd restarted it more than 7,900 times. Each run logged `dogsled: observation publication too large`.

The portal showed `observation unavailable` and `Herdr is not reachable`. `dogsled status` and the MCP tools failed with `observation unavailable: dogsled serve is not running`. No worker could start, because the action lanes run in the same process.

## Cause

Three defects are in the chain. Each one has an owner.

1. **One logical snapshot must fit in one physical message.** `Publisher::replace` (`src/observation.rs:133`) serializes the whole `Published` value into one line. It returns `observation publication too large` when the line is over `ipc::MAX_FRAME_BYTES` (16 MiB, `src/observation/ipc.rs:20`). Every reader receives that whole line, whatever it needs.
2. **A publication failure stops the service.** `service::run` (`src/service.rs:557`) propagates the refresh error with `?`, before `workers.start`. One oversized projection stops admission, handoff, watchdog and sweep.
3. **The live publication carries history, record bodies and copies.** Measured with `state project` on the Mac, local host only, 11.9 MB:

   | Content | Bytes |
   | --- | --- |
   | `local.status` and `views.status` (the same status twice) | 2.60 MB each |
   | `views.inventory` and `views.inventories` (items and queue again, then again for each owner) | 1.90 MB each |
   | `local.host` and `views.hosts` (and again inside each status) | 0.56 MB each |
   | `status.queue`: 482 tickets, of which 387 admitted, 64 cancelled, 31 failed | 1.36 MB |
   | of that, ticket `request` bodies (the task briefs) and `result` bodies | 0.81 MB and 0.42 MB |
   | `status.items`: 641 items, of that `reply` bodies | 0.54 MB, 0.31 MB |

   Each peer adds its host report three times. With two peers the frame passes 16 MiB.

`plans/activity-startup-registration.md` (Decision 2) recorded the size risk at 15.6 MB and named the third defect. It did not name the first two. Removing copies alone moves the limit; it does not remove it.

## Target

- `serve` holds each revision in memory. Publication has no size failure.
- A reader names the domains that it needs. A reply carries only those domains.
- A reply is one or more frames. Each frame is one JSON line of at most 1 MiB. All frames of a reply come from one revision, because the connection holds that revision until the reply ends.
- A reader assembles the frames. It gets the complete selection or an explicit error. No record is dropped or cut to fit.
- A slow reader pins at most one revision. `MAX_CLIENTS` bounds the total.
- A publication failure is logged and published as unavailable. The lanes continue.
- The live views carry live records. History and record bodies are read in pages on request.

## Protocol version 2

`observation::VERSION` becomes `2`. A version 1 reader refuses a version 2 reply, and a version 2 server refuses a version 1 request. Mixed versions report `unsupported observation protocol version`; they do not read a partial publication as complete. All hosts must run the same release, as before.

**Request.** `current` and `subscribe` accept `select`:

```json
{"version":2,"operation":"current","select":{"local":["host"],"views":["status"]}}
```

An absent `select` means the whole publication. `sources` and `gaps` are always sent.

**Reply.** When the selection fits in one frame, the reply is the publication as one line, as in version 1. Otherwise:

```json
{"version":2,"epoch":"…","revision":7,"published_ms":…,"local_revision":3,"local":{},"views":{},"continues":true}
{"sequence":1,"ops":[{"set":{"at":"/views","key":"repos","value":{…}}},{"set":{"at":"/views/status/data","key":"queue","value":[]}},{"push":{"at":"/views/status/data/queue","value":{…}}}],"continues":true}
{"sequence":2,"ops":[{"concat":{"at":"/views/status/data/queue/481/request/task","text":"…"}}]}
```

The first frame holds the scalar members and an empty container for each member that follows. Each later frame holds operations that build the rest: `set` a key of an object, `push` an element of an array, `concat` text to a string. `at` is a JSON pointer (RFC 6901). A value that is larger than a quarter of a frame is opened as an empty container and filled by later operations, so one large record body cannot exceed a frame. The last frame has no `continues`. The reader checks the sequence.

The peer export (`peer` operation) uses the same frames.

Detail and activity streams keep their own codecs and limits. They are already separate from the main publication.

## Units

### T1. Selected, framed replies (this branch)

Owner: lead, in session. No worker can start while `serve` exits.

| Path | Change |
| --- | --- |
| `src/observation/pages.rs` (new) | Split a JSON value into frames of at most `MAX_FRAME_BYTES`; assemble frames into the value. No I/O. |
| `src/observation.rs` | `VERSION = 2`. `Frame` holds each member serialized once instead of one line. `Publisher::replace` has no size check and reuses the bytes of an unchanged domain. |
| `src/observation/ipc.rs` | `MAX_FRAME_BYTES = 1 MiB`. `Request.select`. `current`, `subscribe` and `peer` write frames, each with its own write timeout. The client reads frames and assembles. |
| Readers: `src/main.rs`, `src/hosts.rs`, `src/node.rs`, `src/mcp.rs`, `src/runtime.rs`, `src/sidebar/mod.rs`, `src/web/mod.rs`, `src/observation/peers.rs` | Each call names its selection. Readers of one domain select that domain. Readers that pass the whole publication on (`ActionFleet`, `context::format`, `gate::notices`, `runtime::published_inventory`, `observation current`) select everything; T3 narrows them. |
| `src/service.rs` | A refresh failure in continuous `serve` is logged and the loop continues. `--once` still returns it. |
| `web/src/app.rs` | The agent column shows the observation error. It says `Herdr is not reachable` only for a Herdr error. |
| `docs/design.md`, `README.md` | Protocol version 2. |

Acceptance, all through the real socket:

- A publication of more than 32 MiB is published. A reader that selects one small view receives one frame. A reader that selects everything receives the exact value. No frame is over 1 MiB.
- A record body of more than 1 MiB arrives complete.
- A reply that starts at revision N holds only revision N content when revision N+1 is published during the reply.
- A version 1 request gets `unsupported`.
- Continuous `serve` continues after a publication error and logs it once per change.

Commands: `cargo test --bin dogsled observation::`, `cargo test --test cli observation`, `cargo clippy --all-targets -- -D warnings`, `cargo fmt --check`, `cargo clippy -p web --target wasm32-unknown-unknown -- -D warnings`.

### T2. Live views carry live records

Depends on T1. Profile: writer.

`views.status`, `views.inventory` and the host report carry open items, pending and waiting tickets, running jobs and current holds. A closed item stays live while its branch waits to land. Terminal tickets, closed and landed items, finished jobs and handoffs leave the publication. Ticket `request` and `result` bodies and item `reply` bodies leave the list rows.

`serve` answers a new `history` operation from the store, read-only: kind, cursor, frame-sized page. `items`, `ps --all`, job history and the portal history read pages. Nothing is deleted from `state.db`.

Acceptance: with 10,000 terminal tickets and closed items in the store, the publication size equals the size with none, within the size of the counters. `dogsled items --all` returns all 10,000.

### T3. One copy of each fact, narrow readers

Depends on T2. Profile: writer.

- `views.status` is derived from `local.status` once; peers export `local`, readers read `views`. Drop the second copy from the publication.
- `views.inventory` and `views.inventories` are selections of status rows. Replace them with an owner filter in the request (`select.owner`), applied by `serve` to `boards`, `sidebars`, `job_history` and `findings` too.
- Host reports appear once.
- Each remaining whole-publication reader in T1 names its domains.

Acceptance: `rg "Select::all" src` matches only `observation current|subscribe`. A sidebar reply for one owner holds no other owner's rows.

### T4. Retention of validity evidence

Depends on T2. Profile: research first.

206 of 336 holds on the Mac are `immutable request validity evidence retained; age alone does not release`. They are true and must stay visible. The question for research: which event releases this evidence, and why has it not happened for these requests. No hold is hidden or released by age.

## Release

T1 restores the service only after a release is published and installed on all three hosts. Until then the Mac's `serve` stays down. Do not change dotfiles pins or installed versions before the GitHub release exists.
