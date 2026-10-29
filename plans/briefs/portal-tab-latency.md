# Portal tab latency repair

## Outcome

Returning to a viewed agent shows its retained conversation immediately.
The portal refreshes that conversation without waiting for the full agent list.
The installed portal remains unchanged during development and verification.

## Cause and baseline

`web/src/app.rs:319-325` and `web/src/agent_view.rs:83-96` replace the selected view and discard its conversation.
`web/src/agent_view.rs:117-150` waits for `api::fetch_agent` before it requests the conversation.
`web/src/api.rs:71` implements that lookup with the full agent list.

The running portal is Dogsled v0.27.0, commit `4485f3bb6253ddcd33d6849cee76d889a6a4148a`.
The relevant files match the diagnosed checkout, `ca82f8d272418be60fd37a5ccec0c370f8791867`.
Three initial browser switches took 3014, 1064, and 1724 milliseconds.
Their agent-list requests took 2922, 1048, and 1666 milliseconds.
Their conversation requests took 78, 4, and 47 milliseconds.

A second baseline used `mise run agent:browser` with a fresh signed-out session at 1280 × 633, DPR 1.
Headless Chrome 156 switched between panes `w83:pCZ` and `wWH:p2G`.
Four switches took 804, 918, 4648, and 1987 milliseconds. Every switch showed the loading placeholder.
The last three switches revisited previously loaded conversations.
A DOM observer measured click-to-conversation time. A HAR recorded request timing without response bodies.
The local HAR is `/tmp/dogsled-portal-tab-baseline-wWHp2G.har`. The browser closed after capture.
These samples describe interaction latency under variable fleet load, not a production percentile.

## Implementation contract

The writer unit is `u-b08054e8-7df8-4aee-a138-bcf0cf3599cd-2` on branch `fix/portal-tab-latency`.
Its base is `f208ce3b3210789093009d6fdf9f1ff55eb82c99`. Its native unit description contains the full dispatch contract.
The writer owns `web/src/{conversations.rs,main.rs,app.rs,agent_view.rs,api.rs}` only.

App owns one conversation store for its lifetime. The store retains at most 16 least-recently-used pane snapshots.
Successful empty responses count as retained snapshots. Refresh errors retain successful text and show the error.
A missing-agent response invalidates that pane. A mismatched response cannot update another pane.
Only the selected pane polls and subscribes. Cached text does not authorize actions.
Conversation requests and metadata requests have independent loading and error state.
The implementation preserves blocked prompts, pending-request priority, action validation, and the existing refresh interval.

Keeping hidden views mounted was rejected because their polling and subscriptions would survive selection changes.
Parallel requests alone were rejected because revisits would still discard successful text.

Dioxus 0.7.10 documents shared signal contexts in [use_context_provider](https://docs.rs/dioxus-hooks/0.7.10/dioxus_hooks/fn.use_context_provider.html).
Its [use_resource documentation](https://docs.rs/dioxus-hooks/0.7.10/dioxus_hooks/fn.use_resource.html) describes signal dependencies and cancellation on restart.
The conversation resource must not subscribe to the same cache it updates.

## Acceptance and delivery

The writer runs `cargo test --locked -p web`, `cargo fmt --check -p web`, and `cargo clippy --locked -p web --target wasm32-unknown-unknown -- -D warnings`.
Regression tests cover retention, replacement, empty responses, pane isolation, invalidation, refresh errors, and eviction.
The lead reviews the exact commit, builds the web bundle, and repeats browser measurements on a separate development server.
Browser proof covers immediate revisits, independent first loads, fresh replacement, visible errors, and unavailable-agent handling.
The writer can commit but cannot push, open a PR, merge, publish, or install a release.

Status: implementation in progress. Candidate measurements and integration evidence remain pending.
