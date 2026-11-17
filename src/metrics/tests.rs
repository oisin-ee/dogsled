use std::io::Write as _;

use portal_types::metrics::{RunGroup, ToolGroup, WaitKind};
use rusqlite::params;

use super::{Options, harvest, index, report, store};
use crate::error::Result;
use crate::ledger::Ledger;

const START: u64 = 1_794_916_801_000;
const NOW: u64 = START + 10_000;
const PI: &str = include_str!("../../tests/fixtures/metrics/pi.jsonl");
const CLAUDE: &str = include_str!("../../tests/fixtures/metrics/claude.jsonl");
const CODEX: &str = include_str!("../../tests/fixtures/metrics/codex.jsonl");

fn file(ledger: &Ledger, name: &str, content: &str) -> Result<std::path::PathBuf> {
    let path = ledger.dir.join(name);
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(&path, content)?;
    Ok(path)
}

fn append_inputs(
    ledger: &Ledger,
    connection: &mut rusqlite::Connection,
    shell: &crate::shell::System,
) -> Result<()> {
    for (harness, content) in [
        (
            "pi",
            serde_json::json!({"type":"message","timestamp":"2026-11-17T12:00:03Z","message":{"role":"user","content":"answer"}}),
        ),
        (
            "claude",
            serde_json::json!({"type":"user","timestamp":"2026-11-17T12:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"claude-call","content":"complete"},{"type":"text","text":"answer"}]}}),
        ),
        (
            "codex",
            serde_json::json!({"type":"response_item","timestamp":"2026-11-17T12:00:03Z","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"answer"}]}}),
        ),
    ] {
        let path = ledger.dir.join(format!("{harness}.jsonl"));
        writeln!(
            std::fs::OpenOptions::new().append(true).open(&path)?,
            "{content}"
        )?;
        index::file(connection, &path, harness, NOW, shell)?;
    }
    Ok(())
}

#[test]
fn metrics_indexes_native_transcripts_and_reports_known_and_unknown_status() -> Result<()> {
    let ledger = Ledger::temporary();
    let mut connection = store::open(&ledger)?;
    let shell = crate::shell::System::default();
    for (harness, content, name, error) in [
        ("pi", PI, "read", Some(true)),
        ("claude", CLAUDE, "Read", Some(false)),
        ("codex", CODEX, "exec_command", None),
    ] {
        let path = file(&ledger, &format!("{harness}.jsonl"), content)?;
        index::file(&mut connection, &path, harness, NOW, &shell)?;
        let row = connection.query_row(
            "SELECT tool,started_ms,ended_ms,error,model FROM tool_calls WHERE path=?1",
            [path.to_str()],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Option<bool>>(3)?,
                    row.get::<_, String>(4)?,
                ))
            },
        )?;
        assert_eq!(
            row,
            (
                name.into(),
                crate::rows::integer(START)?,
                crate::rows::integer(START + 1000)?,
                error,
                format!("{harness}-model")
            )
        );
        index::file(&mut connection, &path, harness, NOW, &shell)?;
    }
    let inputs: i64 =
        connection.query_row("SELECT COUNT(*) FROM input_entries", [], |row| row.get(0))?;
    assert_eq!(inputs, 0);
    append_inputs(&ledger, &mut connection, &shell)?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM input_entries", [], |row| row
            .get::<_, i64>(0))?,
        3
    );
    let options = Options::default();
    let local = report::local(&mut connection, &options, NOW)?;
    assert_eq!(local.tools.len(), 3);
    assert_eq!(local.tools.iter().map(|row| row.calls).sum::<u64>(), 3);
    assert_eq!(local.tools.iter().map(|row| row.errors).sum::<u64>(), 1);
    assert_eq!(
        local
            .tools
            .iter()
            .map(|row| row.unknown_status)
            .sum::<u64>(),
        1
    );
    assert!(
        local
            .tools
            .iter()
            .all(|row| row.duration.quantile(0.95) == Some(1000))
    );
    let mut merged = local.clone();
    report::merge(&mut merged, local, "peer")?;
    assert_eq!(merged.hosts.len(), 2);
    assert!(
        merged
            .tools
            .iter()
            .all(|row| row.calls == 2 && row.duration.count() == 2)
    );
    let grouped = report::local(
        &mut connection,
        &Options {
            group: ToolGroup::Harness,
            ..Options::default()
        },
        NOW,
    )?;
    assert!(grouped.tools.iter().all(|row| row.group.is_some()));
    let filtered = report::local(
        &mut connection,
        &Options {
            tool: Some("Read".into()),
            ..Options::default()
        },
        NOW,
    )?;
    assert_eq!(filtered.tools.len(), 1);
    Ok(())
}

#[test]
fn metrics_pairs_appended_results_and_commits_only_complete_lines() -> Result<()> {
    let ledger = Ledger::temporary();
    let mut connection = store::open(&ledger)?;
    let shell = crate::shell::System::default();
    let lines = PI.lines().collect::<Vec<_>>();
    let calls = format!("{}\n{}\n", lines[0], lines[1]);
    let path = file(&ledger, "append.jsonl", &calls)?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    let initial = report::local(&mut connection, &Options::default(), NOW)?;
    assert_eq!(initial.tools[0].running, 1);
    assert_eq!(initial.tools[0].duration.count(), 0);
    let mut writer = std::fs::OpenOptions::new().append(true).open(&path)?;
    writer.write_all(lines[2].as_bytes())?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    let offset: i64 =
        connection.query_row("SELECT offset FROM transcripts", [], |row| row.get(0))?;
    assert_eq!(offset, crate::rows::integer(calls.len() as u64)?);
    writer.write_all(b"\n")?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    let complete = report::local(&mut connection, &Options::default(), NOW)?;
    assert_eq!(complete.tools[0].calls, 1);
    assert_eq!(complete.tools[0].errors, 1);
    assert_eq!(complete.tools[0].running, 0);
    assert_eq!(complete.tools[0].duration.count(), 1);
    Ok(())
}

#[test]
fn metrics_codex_model_cursor_survives_passes_and_resets_with_the_file() -> Result<()> {
    let ledger = Ledger::temporary();
    let mut connection = store::open(&ledger)?;
    let shell = crate::shell::System::default();
    let lines = CODEX.lines().collect::<Vec<_>>();
    let context = format!("{}\n{}\n", lines[0], lines[1]);
    let path = file(&ledger, "cursor.jsonl", &context)?;
    index::file(&mut connection, &path, "codex", NOW, &shell)?;
    assert_eq!(
        connection.query_row("SELECT model FROM transcripts", [], |row| row
            .get::<_, String>(0))?,
        "codex-model"
    );
    let calls = format!("{}\n{}\n", lines[2], lines[3]);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(calls.as_bytes())?;
    index::file(&mut connection, &path, "codex", NOW, &shell)?;
    assert_eq!(
        connection.query_row("SELECT model FROM tool_calls", [], |row| row
            .get::<_, String>(0))?,
        "codex-model"
    );
    // Truncation must not attach the previous context to a new call.
    std::fs::write(&path, &calls)?;
    index::file(&mut connection, &path, "codex", NOW, &shell)?;
    assert!(
        connection
            .query_row("SELECT model FROM tool_calls", [], |row| row
                .get::<_, Option<String>>(0))?
            .is_none()
    );
    let replacement = file(&ledger, "with-context.jsonl", CODEX)?;
    std::fs::rename(replacement, &path)?;
    index::file(&mut connection, &path, "codex", NOW, &shell)?;
    let replacement = file(&ledger, "without-context.jsonl", &calls)?;
    std::fs::rename(replacement, &path)?;
    index::file(&mut connection, &path, "codex", NOW, &shell)?;
    assert!(
        connection
            .query_row("SELECT model FROM tool_calls", [], |row| row
                .get::<_, Option<String>>(0))?
            .is_none()
    );
    Ok(())
}

#[test]
fn metrics_replacement_and_truncation_discard_old_calls_and_inputs() -> Result<()> {
    let ledger = Ledger::temporary();
    let mut connection = store::open(&ledger)?;
    let shell = crate::shell::System::default();
    let path = file(&ledger, "replace.jsonl", PI)?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    let replacement = file(&ledger, "new.jsonl", &PI.replace("pi-call", "new-call"))?;
    std::fs::rename(replacement, &path)?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    let ids = connection
        .prepare("SELECT call_id FROM tool_calls")?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    assert_eq!(ids, ["new-call"]);
    std::fs::write(&path, "{\"type\":\"session\",\"id\":\"empty\"}\n")?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM tool_calls", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    Ok(())
}

#[test]
fn metrics_wait_ends_at_first_input_even_when_a_pass_contains_two() -> Result<()> {
    let ledger = Ledger::temporary();
    let item = crate::items::Item {
        id: "item".into(),
        kind: "agent".into(),
        host: "local".into(),
        pane: "pane".into(),
        owner_pane: "owner".into(),
        created_ms: START,
        signal: Some(crate::items::ItemSignal {
            kind: crate::lifecycle::Signal::Ask,
            text: "question".into(),
            event_id: "signal".into(),
            recorded_ms: START + 1000,
            session: None,
            reported_ms: None,
        }),
        ..crate::items::Item::default()
    };
    crate::store::write(&ledger, |tx| {
        crate::tables::sync_items(tx, std::slice::from_ref(&item))
    })?;
    let mut connection = store::open(&ledger)?;
    let shell = crate::shell::System::default();
    let path = file(&ledger, "wait.jsonl", PI)?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    connection.execute("UPDATE transcripts SET item='item',pane='pane'", [])?;
    harvest::Source::read(&ledger)?.write(&mut connection, &shell)?;
    let open = report::local(&mut connection, &Options::default(), NOW)?;
    assert_eq!(
        open.owner_waits
            .iter()
            .find(|row| row.kind == WaitKind::Ask)
            .unwrap()
            .open,
        1
    );
    let input = "{\"type\":\"message\",\"timestamp\":\"2026-11-17T12:00:03.000Z\",\"message\":{\"role\":\"custom\",\"customType\":\"dogsled\",\"content\":[]}}\n{\"type\":\"message\",\"timestamp\":\"2026-11-17T12:00:04.000Z\",\"message\":{\"role\":\"user\",\"content\":[]}}\n";
    std::fs::OpenOptions::new()
        .append(true)
        .open(&path)?
        .write_all(input.as_bytes())?;
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    // The signal may have been cleared before the harvest sees the new inputs.
    let mut cleared = item;
    cleared.signal = None;
    crate::store::write(&ledger, |tx| crate::tables::sync_items(tx, &[cleared]))?;
    harvest::Source::read(&ledger)?.write(&mut connection, &shell)?;
    let ended: i64 = connection.query_row(
        "SELECT ended_ms FROM owner_waits WHERE id='item:signal'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(ended, crate::rows::integer(START + 2000)?);
    let report = report::local(&mut connection, &Options::default(), NOW)?;
    let wait = report
        .owner_waits
        .iter()
        .find(|row| row.kind == WaitKind::Ask)
        .unwrap();
    assert_eq!((wait.answered, wait.open, wait.wait.max_ms), (1, 0, 1000));
    Ok(())
}

#[test]
fn metrics_run_group_is_independent_of_tool_group() -> Result<()> {
    let ledger = Ledger::temporary();
    let items = [
        crate::items::Item {
            id: "first".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "first-pane".into(),
            created_ms: START,
            launch_confirmed_ms: Some(START + 100),
            profile: Some("writer".into()),
            model: Some("first-model".into()),
            harness: Some("pi".into()),
            ..crate::items::Item::default()
        },
        crate::items::Item {
            id: "second".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "second-pane".into(),
            created_ms: START,
            launch_confirmed_ms: Some(START + 200),
            profile: Some("writer".into()),
            model: Some("second-model".into()),
            harness: Some("claude".into()),
            ..crate::items::Item::default()
        },
    ];
    crate::store::write(&ledger, |tx| crate::tables::sync_items(tx, &items))?;
    let mut connection = store::open(&ledger)?;
    harvest::Source::read(&ledger)?.write(&mut connection, &crate::shell::System::default())?;
    for (run_group, expected) in [
        (RunGroup::Profile, vec!["writer"]),
        (RunGroup::Model, vec!["first-model", "second-model"]),
        (RunGroup::Harness, vec!["claude", "pi"]),
    ] {
        let options = Options {
            group: ToolGroup::Repository,
            run_group,
            ..Options::default()
        };
        let report = report::local(&mut connection, &options, NOW)?;
        assert_eq!(report.run_group, run_group);
        assert_eq!(
            report
                .runs
                .iter()
                .map(|row| row.group.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            report.runs.iter().map(|row| row.spawn.count()).sum::<u64>(),
            2
        );
        assert_eq!(
            report
                .runs
                .iter()
                .map(|row| row.outcomes.running)
                .sum::<u64>(),
            2
        );
    }
    Ok(())
}

#[test]
fn metrics_checkpoint_advances_only_after_a_whole_successful_pass() -> Result<()> {
    let ledger = Ledger::temporary();
    let path = file(&ledger, "launches/session.jsonl", PI)?;
    let home = ledger.dir.to_str().unwrap();
    super::pass(&ledger, home, &[])?;
    let connection = store::open(&ledger)?;
    let indexed = connection.query_row("SELECT indexed_ms FROM index_state", [], |row| {
        row.get::<_, i64>(0)
    })?;
    assert!(indexed > 0);
    assert_eq!(
        super::local(&ledger, &Options::default())?.hosts[0].indexed_ms,
        Some(u64::try_from(indexed).unwrap())
    );
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)?
        .write_all(b"not-json\n")?;
    assert!(super::pass(&ledger, home, &[]).is_err());
    assert_eq!(
        connection.query_row("SELECT indexed_ms FROM index_state", [], |row| row
            .get::<_, i64>(0))?,
        indexed
    );
    Ok(())
}

#[test]
fn metrics_old_schema_rebuilds_and_retention_expires_derived_rows() -> Result<()> {
    let ledger = Ledger::temporary();
    let mut connection = store::open(&ledger)?;
    let path = file(&ledger, "old.jsonl", PI)?;
    let shell = crate::shell::System::default();
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    connection.execute_batch("CREATE TABLE obsolete(value); PRAGMA user_version=0;")?;
    drop(connection);
    let mut connection = store::open(&ledger)?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM transcripts", [], |row| row
            .get::<_, i64>(0))?,
        0
    );
    assert_eq!(
        connection.query_row(
            "SELECT COUNT(*) FROM sqlite_schema WHERE name='obsolete'",
            [],
            |row| row.get::<_, i64>(0)
        )?,
        0
    );
    index::file(&mut connection, &path, "pi", NOW, &shell)?;
    assert_eq!(
        connection.query_row("SELECT COUNT(*) FROM tool_calls", [], |row| row
            .get::<_, i64>(0))?,
        1
    );
    connection.execute(
        "INSERT INTO input_entries(path,at_ms) VALUES(?1,?2)",
        params![path.to_str(), crate::rows::integer(START)?],
    )?;
    store::retain(&mut connection, NOW + crate::jobs::RECORD_KEEP_MS + 1)?;
    for table in ["tool_calls", "transcripts", "input_entries"] {
        assert_eq!(
            connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                .get::<_, i64>(0))?,
            0
        );
    }
    connection.pragma_update(None, "user_version", 2)?;
    drop(connection);
    assert!(store::open(&ledger).is_err());
    Ok(())
}
