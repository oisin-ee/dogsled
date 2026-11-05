# Observation fixtures

`context_pre_handoff.json` captures two open, finished children before handoff. Each child has a different owner. A real Git worktree is two commits ahead.

The producer test checks the complete normalized publication byte-for-byte. The CLI test loads the same file without changes. Normal tests never rewrite it.

To regenerate this fixture explicitly:

```sh
DOGSLED_UPDATE_CONTEXT_PUBLICATION=1 cargo test --bin dogsled observation::tests::context_fixture::context_pre_handoff_snapshot_matches_real_capture_and_compose
```

Review the JSON diff after generation. The fixture builder normalizes the temporary root, epoch, clock, and CPU capacity.
