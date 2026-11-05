pub fn context_pre_handoff() -> serde_json::Value {
    serde_json::from_slice(
        &std::fs::read(context_path())
            .unwrap_or_else(|error| panic!("read context publication fixture: {error}")),
    )
    .unwrap_or_else(|error| panic!("decode context publication fixture: {error}"))
}

pub fn context_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/observation/context_pre_handoff.json")
}
