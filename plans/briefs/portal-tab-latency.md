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

## Candidate evidence

The writer committed `29087add937fb9de9c237e201c01ed7751c04fd7` on base `497f133c49b81885956fa67bd0dc2e821f2657d5`.
The actual base has no scoped source, configuration, or lockfile differences from the specified base.
The three writer checks passed on the committed inputs. Six new tests cover the production cache transitions.
An independent reviewer accepted the exact commit without actionable findings.
The lead integrated the fix locally as `34e008572f706b07eb0a1ee8f8d42a9efa243fa0`.
The integrated web inputs match the tested candidate exactly.

The candidate ran with `proto run dx -- serve --web --release --locked --package web --port 7466 --open false --watch false --interactive false`.
Its existing development proxy used the same live API on port 7465 as the baseline.
The isolated browser used the same viewport and the same two live panes.
The first switch took 305 milliseconds. Three revisits took 7, 22, and 8 milliseconds without the loading placeholder.
The initial transcript request started at 65 milliseconds and completed at 88 milliseconds.
The full agent-list request completed at 1277 milliseconds. Transcript display no longer waits for that request.
The candidate HAR is `/tmp/dogsled-portal-tab-candidate-29087add.har`.

Managed browser interception also verified the response states without changing agent data:

- An aborted refresh retained seven turns and showed a fetch error.
- A successful refresh replaced the retained text and cleared the error.
- A mismatched response retained the correct text and showed an identity error.
- A successful empty response showed the empty state, without a loading placeholder.
- A real missing pane showed the unavailable error and disabled the composer, without a false empty state.

The browser closed, and the development-server job stopped after verification.
The design detector reported only the existing Inter font warning in `web/src/app.rs`.
The installed v0.27.0 portal remains unchanged. No release, installation, push, or PR occurred.
The final scoped check passed: `proto run moon -- run dogsled:web-build --affected --base af28f11b0dbe4f5352b90f2cb36f84c39f887a34 --head 34e008572f706b07eb0a1ee8f8d42a9efa243fa0`.
Moon completed the release web bundle in 31 seconds. The later unrelated documentation commit did not change its tested inputs.
Status: implemented, reviewed, integrated locally, and verified. Publication and installation remain outside this change.
