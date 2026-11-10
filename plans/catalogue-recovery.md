# Live recovery for the repository catalogue

Status: approved in Dogsled plan review `f085da93e6777084126562623044e474`. Implementation is authorized. Release and rollout are not authorized.

Reviewed source: `9b0e914b5412483fdec4ce8e27c2c74abc96ff5a` on `main`.
The browser reproduction used installed Dogsled `0.34.2`.
This plan changes no installed version, credentials, service configuration, or production code.

## Outcome and limits

An open New conversation must show the latest published repository catalogue.
A successful refresh must clear the previous refresh error without navigation or a page reload.
Updates must preserve the owner's draft, attachments, search, and valid repository selection.

The owning defect is the disconnected read path in `web/src/new_lead.rs`.
The component fetches the catalogue once, outside the portal's existing publication stream.
The proposed repair connects that component to the same publication owner as the other portal views.

This repair does not prevent GitHub connection failures.
It does not shorten the existing one-hour interval between GitHub attempts.
A failed attempt can therefore remain the latest result until the next scheduled attempt succeeds.
The exact cause of the original connection failure remains unknown.

## Evidence

| Finding | Evidence |
| --- | --- |
| The original service error was real. | `dogsled repo list --json` initially returned 73 repositories and the screenshot's connection error. |
| The service later recovered without intervention. | Subsequent CLI and browser GET reads returned 73 repositories and `error: null`. |
| The mounted picker retains an obsolete error. | In the isolated browser, a mocked initial `/api/repos` response supplied the failure. After removing the mock, a real GET succeeded while the header retained the failure. |
| The picker has no update dependency. | `web/src/new_lead.rs:86` uses `use_resource(api::fetch_repos)`. That function reads no reactive signal. |
| The stream omits the catalogue. | `portal-types/src/lib.rs::PortalSnapshot` contains board, agents, inbox, and gaps, but no catalogue. |

The browser reproduction did not send a message, create a lead, interrupt a service, or change credentials.
The route mock affected only the isolated browser.
The browser was closed after the check.

The earlier diagnostic HTTP 403 came from the agent's Momo GitHub identity.
The installed agent wrapper selects that identity for these requests.
The service's launch configuration does not use that wrapper path.
That diagnostic was not evidence of broken service credentials.

> 🧠 **From Hindsight memory (One observer per host)** — `serve` owns observation. Readers consume immutable publications through the existing shared stream, without independent probes or fallback polling.

Current source confirms this boundary in `src/observation/collect.rs`, `src/observation/project.rs`, and `src/web/mod.rs`.

> 🧠 **From Hindsight memory** — The recorded catalogue policy is startup plus hourly refresh, retaining the last list and publishing failures. Memory establishes no compatibility rule for future portal schema versions.

Current source and `README.md:610-622` confirm the hourly policy.
`Catalogue::refresh` records the attempt time before the request, so failures also consume the interval.
The schema design below is an explicitly approved decision, not a recovered historical requirement.

## Alternatives considered

| Approach | Decision |
| --- | --- |
| Include the catalogue in the existing snapshot. | Recommended. One coherent revision and one browser stream, with no extra requests. |
| Stream a catalogue stamp, then refetch `/api/repos`. | Rejected. Adds a second request, ordering races, and another failure path for a small payload. |
| Poll or add a separate catalogue stream. | Rejected. Adds an independent lifecycle and contradicts the shared-observer design. |
| Restart services, reload pages, or retry GitHub more often. | Not the repair. Reloading only resets the component. Retry timing is a separate product policy. |

The current catalogue serializes to approximately 10 KB for 73 repositories.
That supports a full snapshot rather than a separate invalidation protocol.
This measurement is not a bound on future repository counts.
The existing 16 MiB observation-frame limit remains enforced, without truncation or a larger limit.

## Proposed design

### 1. Publish one typed catalogue in the existing snapshot

Add `repos: Option<RepoList>` to `PortalSnapshot`.
Use the existing `RepoList { repos, error }` type without a second catalogue model.

Decode the local `repos` domain once at the projection boundary.
Use the same decoded value for the existing `repos` view and the new snapshot field.
Both values must belong to the same immutable publication.
No GitHub, filesystem, store, or host read belongs in snapshot serialization or HTTP delivery.

`Some(list)` means the producer supplied a catalogue result.
An empty list with an error is a failed initial fetch, not a successful empty catalogue.
`None` means the catalogue domain is unavailable.
Missing or malformed input must not become `Some(RepoList::default())`.
An absent domain stays `None`, including partial test publications that intentionally omit GitHub data.
Malformed present data produces a scoped `repos` gap without discarding the other snapshot fields.
Keep an ordinary GitHub refresh error inside `RepoList.error`, rather than manufacturing a whole-stream outage.

`Publisher::replace` already compares view contents and assigns the publication revision.
Preserve that mechanism.
A catalogue-only error change must produce a new revision even when its repository list is unchanged.
Peer-only publications must reuse the local catalogue without another GitHub call.

Keep `GET /api/repos` and `dogsled repo list --json` compatible with their existing `{repos, error}` response.
Keep the REST endpoint's `Cache-Control: no-store` and unavailable-response behavior.
Do not change lead creation or its server-side authorization.

### 2. Version the changed portal contract explicitly

`PortalSnapshot` and `PortalStatus` reject unknown fields.
Adding `repos` is therefore not transparent to an already-open old browser or an old `dogsled web` process.

Introduce a shared portal-snapshot version constant in `portal-types`, with value `2`.
Use it in the projector and browser validation.
Keep observation IPC version `1`: its envelope and operations do not change.
Do not bump the fleet-wide IPC version for this portal-only payload change.

Preserve strict snapshot/status discrimination and epoch/revision validation.
Update every owned snapshot constructor and fixture.
Cover unsupported portal versions with an explicit error, without accepting an old schema or substituting an empty catalogue.

This is one coordinated release of the producer, web server, and embedded client.
After an authorized release rollout, restart both `serve` and `web`, then reload existing portal tabs.
An old tab can report a decoding error until that reload.
There is no compatibility adapter or silent polling fallback in this scope.

### 3. Use the existing browser reducer and subscription

Extend `PortalState` in `web/src/app.rs` to retain the catalogue and its observation error.
Keep GitHub refresh errors separate from transport or missing-domain errors.
On stream loss, retain the last list but mark it as last-known data.
On recovery, replace that state from the new snapshot and clear obsolete observation errors.

Expose a catalogue memo through the existing app context pattern.
Use value equality so unrelated board, inbox, and agent changes do not invalidate the picker.
Do not create a new EventSource, timer, observer, or fetch-on-every-publication effect.

Replace NewLead's resource fetch with that shared state.
Remove the unused browser `fetch_repos` function after a caller search.
Keep the server endpoint for other readers.

### 4. Preserve interaction state and truthful errors

Keep the existing combobox and composer mounted across catalogue updates.
Do not key their parent by publication revision or use a remount to reset state.
Keep repository rows keyed by slug, not array position.

| State or transition | Required behavior |
| --- | --- |
| No catalogue received yet | Show loading, not an empty result. Do not choose a repository. |
| Initial fetch fails without repositories | Show the failure. Do not also claim “No GitHub repositories found.” |
| Refresh fails with a retained list | Keep the list and valid selection usable. Show the refresh error once. |
| Refresh succeeds | Replace the list and clear the warning in the same mounted view. |
| Stream disconnects or the domain is unavailable | Retain last-known choices with an explicit observation warning. Do not claim they are current. |
| A successful list omits the selected repository | Keep the selected slug visible. Require another explicit selection before Send. Never silently switch targets. |
| Publication arrives during Send | Keep the request's captured target and message. Do not issue another request or change its destination. |

Initial selection continues to prefer the current repository hint, then the first available repository.
That default applies only before an initial selection, not after the owner clears a selection or a refresh removes it.
A valid explicit selection survives reordering, description changes, failure, recovery, and unrelated publications.
Draft text and attachments remain unchanged throughout these transitions.

Show the backend refresh message once, without adding another `GitHub catalogue:` prefix.
Use the current Alert and Status components and existing layout.
There is no visual redesign, new control, palette change, or new wireframe in this repair.

The selection-removal behavior is an approved safety rule for this implementation.
It prevents a background update from silently changing where the first message goes.

## Verification design

Existing tests prove static picker markup, sorting, failed-refresh retention, and shared portal subscriptions.
They do not prove catalogue recovery in a mounted NewLead component.
The regression must fail against the old behavior before the repair.

| Layer | Required regression |
| --- | --- |
| Collector and projector | Good → failed → recovered results at controlled timestamps. Retain the list on failure and clear the error on success. Assert the unchanged hourly request count. Compare snapshot and `repos` view values, including empty, missing, malformed, and peer-only cases. |
| Wire and publisher | Round-trip the new snapshot. Reject ambiguous status/snapshot shapes and unsupported versions. Prove that an error-only change advances the revision and unchanged content does not create an artificial change. |
| Reducer and mounted component | Apply failed and recovered snapshots to the same component instance. Assert warning removal, stable scope, draft and attachment preservation, valid selection, removed selection, empty results, and reconnect behavior. |
| Running HTTP and IPC | Extend `tests/cli/observation_portal.rs`. Keep multiple SSE clients open through catalogue-only transitions. Assert identical epoch/revision ordering, one shared IPC subscription, and zero reader-side `gh`, Git, Herdr, or SSH calls. Preserve REST/CLI output. |
| Browser | Use the managed isolated browser against a test portal. Verify search, keyboard navigation, focus, draft text, and warning recovery without navigation. Verify that catalogue changes do not create a lead or issue an extra Send. |

Use fake producer time for hourly boundaries. Do not wait an hour or shorten production intervals for tests.
Use the existing collector/projector, `Publisher`, `Portal`, and `Supplied` fixtures, not a replacement server.
If browser-driven transitions need a fixture-lifetime control, keep it inside the existing test harness.
No test-only HTTP endpoint belongs in production.

Native mounted-component tests follow the existing `VirtualDom`, `Signal`, and `render_immediate_to_vec` pattern in `web/src/component_tests.rs`.
Native tests cannot prove actual browser focus or keyboard behavior, so that browser check remains required.

## Implementation ownership

Use one bounded `writer` unit for the coherent recovery fix, followed by lead integration review.
Do not split shared wire types and their callers across independently shippable changes.
Do not dispatch the unit until this plan is approved.

The writer owns these path groups:

| Group | Paths |
| --- | --- |
| Portal contract | `portal-types/src/lib.rs`, `portal-types/src/tests.rs` |
| Projection and producer tests | `src/observation/project.rs`, `src/observation/tests/local.rs`, `src/observation/tests/projection.rs`, `src/observation/tests.rs` |
| Snapshot fixtures and transport tests | `src/observation/details_fixture.rs`, `src/web/test_support.rs`, `tests/cli/observation_portal.rs` |
| Browser state and component tests | `web/src/app.rs`, `web/src/api.rs`, `web/src/new_lead.rs`, `web/src/component_tests.rs` |
| User-facing contract | The catalogue section of `README.md` |

Read-only constraints include `src/observation/catalogue.rs`, `src/observation.rs`, `src/web/mod.rs`, `src/web/agents.rs`, and the component dependencies.
If current fixtures reveal another necessary owner, update the scope before editing it.
Do not change authentication, retry schedules, third-party code, dependency pins, global hooks, or service configuration.

### Required checks for implementation

Run the focused tests first:

```sh
cargo test --locked -p portal-types
cargo test --locked -p dogsled catalogue
cargo test --locked -p dogsled observation::tests
cargo test --locked -p dogsled --test cli observation::portal::
cargo test --locked -p web
```

Each filter must select at least one test.
The web and portal-types tests are explicit because the root package's test task is not proof of their native tests.

Run the repository gates on the integrated revision:

```sh
moon run dogsled:fmt-check dogsled:web-lint dogsled:web-build dogsled:lint dogsled:test
```

Run browser checks through `mise run agent:browser -- ...`, then close the browser.
Do not edit generated assets by hand.
The normal commit hook still applies.
Use existing CI gates during authorized delivery, without extra workflow dispatches merely to gather evidence.

## Planning evidence and remaining limits

`cargo test --locked -p portal-types` passed on the reviewed revision.
`cargo test --locked -p web new_lead::tests` also passed.
An earlier attempt at the second command reached its 180-second execution limit without a result. A later run completed successfully.
These are baseline tests, not proof of the proposed repair.

The isolated-browser reproduction proved the retained warning after a successful API read.
It did not establish the network cause of the original incident.
No production-code changes or new regression tests exist yet.

The plan review covered ownership, fixture updates, protocol compatibility, selection changes, request counts, and release boundaries.
The implementation must still prove dynamic combobox behavior against the pinned component version.
A dependency defect is a blocker, not permission to patch or replace that dependency with a workaround.

## Primary framework references

Dioxus hooks are pinned to `0.7.10` in `Cargo.lock`.
The version-matched [`use_resource` documentation](https://docs.rs/dioxus-hooks/0.7.10/dioxus_hooks/fn.use_resource.html#reactivity) states that resources rerun when signals read inside their future change.
The current catalogue future reads none.

The [`use_memo` documentation](https://docs.rs/dioxus-hooks/0.7.10/dioxus_hooks/fn.use_memo.html) specifies equality-based downstream invalidation.
That supports the existing app-context pattern without per-publication picker resets.

The pinned [Combobox source](https://github.com/DioxusLabs/dioxus-components/blob/9a758255ea26e2b20c8cecf4c1c946feb9e71da7/primitives/src/combobox/components/combobox.rs) exposes controlled value, query, and open state.
Use supported APIs only if the implementation needs explicit state control.
Do not patch dependencies or use remounts to conceal lost interaction state.

## Delivery boundary

Approval authorizes the bounded implementation, not a release or fleet rollout.
Deliver one reviewed change with fresh evidence for its exact revision.
A published GitHub release and separate rollout authorization are required before local adoption.
Until then, keep installed binaries, packages, and version pins unchanged.
