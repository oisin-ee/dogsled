# Portal Close publication repair

## Contract

One click must close the selected native session and remove its sidebar entry through authoritative observation.
The repair must preserve siblings, checkout contents, task state, and native refusals.
It must not hide rows optimistically, add polling, delete history, or change release policy.

## Cause and repair

Native Close succeeded while the portal continued to show an older publication.
`holds::local` repeatedly called settlement predicates during synchronous host collection.
Those predicates decoded every ticket or item to find one request.

`tables::ticket_by_request` and `tables::item` use the existing unique indexes and typed decoders.
`work::references_released` and `work::admission_completion` retain their existing state and provenance predicates.
The matched record still fails on corrupt data or a storage error.
Unrelated corrupt rows no longer enter these per-request reads.

The repair changes no UI code, schema, dependency, close adapter, or safety rule.

## Automated evidence

The production-path fixture contains 779 admissions, 506 tickets, and 665 items.
It includes 757 finished admissions and 156 legacy admissions without ordinary-release certificates.
Both revisions produced 461 settled requests and 480 holds across three samples.
The complete-result SHA256 fingerprint remained:

```text
9ad0cf402b5895c638e0757cb903d099ca3da573a42065664bd2047b9786a71c
```

| Optimized build measurement | Baseline | Repair |
| --- | --- | --- |
| Settlement pass | 4.305–4.502s | 93–110ms |
| Holds pass | 4.103–5.790s | 183–218ms |

Baseline source: `1321e0dc7d401e92936355447d63a26781afffd0`, with the same workload test.
Repair source: `8c0cc4dccc5bdb5fb2f8b418dcccbc835c471edf`.
These measurements are diagnostic evidence, not wall-clock test assertions.
The fixture does not measure preservation-operation cost.

`tests/cli/observation_portal.rs` adds two integration tests through real web and serve processes.
A stateful socket transport changes native pane evidence and emits `pane.closed`.
An already-open portal stream and `/api/agents` must remove only the selected session.
A 3600-second fallback interval prevents polling from satisfying the test.
The refusal test preserves both sessions after a fresh observer publication and checks the existing 502 response.
Both tests preserve tracked, dirty, transcript, and Git sentinel bytes.
Native Herdr is simulated only in these integration tests.

## Real browser and native-session evidence

The lead used isolated local and remote native sessions, private Dogsled state, and port 17465.
All browser commands used the repository-scoped `mise run agent:browser --` task.
A click listener measured elapsed time until a DOM observer saw the sidebar button disappear.

The live history fixture used synthetic records, not production data.
It kept all tickets terminal, all admissions finished, all items closed, and timestamps recent.
This prevented execution of queued fixture work and age-based retention removal.
The database retained 779 admissions, 506 tickets, and 665 items after both runs.

| Browser Close measurement | Sidebar removal |
| --- | --- |
| Installed 0.36.1, empty state, local | 358ms |
| Installed 0.36.1, empty state, remote | 2077ms |
| Installed 0.36.1, synthetic history, local | 16,712ms |
| Repair, same synthetic history, local | 676ms |
| Repair portal, same local history, remote | 1385ms |

The affected production files are identical between installed baseline source `4eb230d` and test baseline `1321e0d`.
The local and remote targets reported `working` before Close.
Both repaired requests returned 202, and subsequent native reads returned `pane_not_found`.
The sibling remained in native state, `/api/agents`, and the sidebar.
The remote observer used its installed version and private empty state.
Thus, remote evidence covers the repaired portal, local publication, and native remote close path, not remote history scaling.

The repair executable came from the writer's optimized build at `8c0cc4d`.
Its SHA256 was:

```text
fa03fa96f1b2bab8912f344edbe8e4969a2703c224ee42f55d2bf63787b6aa77
```

The lead ran this executable only in the isolated test environment.
Installed binaries and version pins remained unchanged.

## Checks

Both writers passed their scoped checks and normal commit hooks.
The repair writer also passed the full Rust suite through its commit hook.
The lead owns the combined affected gate and publication evidence.
Release and installation remain separate delivery steps.

```sh
proto run moon -- run :#precommit --affected --base origin/main
cargo test --release -p dogsled close_publication_workload_preserves_settlement_and_holds -- --nocapture
cargo test --test cli observation_portal
```
