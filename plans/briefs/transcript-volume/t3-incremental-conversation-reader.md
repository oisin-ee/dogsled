# T3: serve reads only appended transcript bytes

**Outcome:** when a transcript grows, the per-pane detail worker parses only the bytes appended since its last read. The resulting conversation is exactly what a fresh read returns. It rescans from the end only when the file is replaced or truncated. Lines that cannot be turns cost no JSON tree.

**Cause (measured 10 November 2026):**
- `Capture::conversation` caches by file metadata: dev, inode, length and mtime (`src/observation/details.rs:930–968`, `same_file` at `:971`). Every appended line invalidates the cache.
- A miss then calls `activity::try_conversation`. That reads newest-first in 1 MB chunks up to `CONVERSATION_BYTES` = 32 MB (`src/activity.rs:338–343`) and `serde_json::from_str::<Value>`s every line (`:251`) until it has `CONVERSATION_TURNS` (200) text turns.
- Pi sessions carry `todo-state` custom entries of 1–2 MB each, so 200 text turns can span tens of MB. A separate unit, T2, shrinks those entries; old sessions keep them.

**Edits**
1. `src/activity.rs`:
   - Add an incremental history owner, for example `Incremental { identity: (dev, ino), offset: u64, history: History, window counts }`.
   - Give it `advance(agent, path) -> io::Result<&History>`. If identity and length are unchanged, return as is. If the identity changed or the length shrank, rescan with today's backward read and record the offset. If the file only grew, read forward from `offset` to the last complete line, take records with today's `records(...)`, and merge them into the window.
   - Merge rules: tool call/result pairing by `call_id`, Claude's adjacent-block joining, Dogsled delivery de-duplication, and the `CONVERSATION_TURNS` / `CONVERSATION_TOOLS` / `CONVERSATION_THOUGHTS` caps with `truncated`. Apply the same rules the backward reader applies (`conversation_with_limit`, lines ~240–335), evicting the oldest entries as caps are reached.
   - Keep any partial last line for the next advance.
   - Before a full parse, classify each line cheaply: a borrowed `type`/kind-only deserialize, or a bounded prefix check. Skip kinds that `records` never turns into a record for that agent. Never skip a line that `records` would use.
2. `src/observation/details.rs`: `Capture` keeps an `Incremental` in place of the `(Metadata, Content<Conversation>)` cache, and builds `Conversation` from it. Error handling stays unchanged: an I/O error is still `Content::Failed`.
3. **Tests in `src/activity.rs`:**
   - **Equivalence (the core test):** for Pi, Claude and Codex fixture transcripts, append the lines in random chunk boundaries, including mid-line splits. After every append, `Incremental::advance` must equal a fresh `conversation_with_limit` on the same file, for turns and for `truncated`. Cover tool results arriving after their calls, Claude multi-block messages, delivery duplicates, and cap eviction.
   - Truncation and replacement trigger a rescan with the correct result.
   - A 2 MB `todo-state` line is skipped without being parsed into a `Value`. Prove it with a counter or by timing; the method is your choice.
4. **Tests in `src/observation/tests/details.rs`:** the existing detail tests pass unchanged.

**Owned paths:** `src/activity.rs` and `src/observation/details.rs` (the conversation cache only). Read everything else. Do not change the wire format, `web/`, or `pi/`.

**Acceptance:** `cargo test --bin dogsled activity` and `cargo test --bin dogsled observation::tests::details` pass, including the equivalence test.

**Checks (only these):** the two filters above, `cargo clippy --all-targets -- -D warnings`, and `cargo fmt --check`. `web/public` must exist. If it does not, ask the lead. Commit hooks run as configured. Do not bypass them.

**Publication:** commit with `perf(activity): read only appended transcript bytes for conversation detail`. Push your branch. Do not open a PR, merge, release or install.

**Base:** given by the lead at dispatch. It comes after T1 lands, because both edit the details tests.

**Reply:** branch, head SHA, changed paths, each command with its result, one before/after timing of an append to a 30 MB fixture transcript, and anything you did not verify.
