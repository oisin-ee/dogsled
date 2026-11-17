//! Local queries and histogram-preserving fleet aggregation.
use portal_types::metrics::{
    Histogram, HostCoverage, METRICS_VERSION, MetricsReport, Outcomes, RunGroup, RunRow, ToolGroup,
    ToolRow, UNATTRIBUTED, WaitKind, WaitRow,
};
use rusqlite::{Connection, OptionalExtension as _, params};

use super::Options;
use crate::error::{Error, Result};

pub(super) fn local(
    connection: &mut Connection,
    options: &Options,
    now: u64,
) -> Result<MetricsReport> {
    let tx = connection.transaction()?;
    let since = crate::rows::integer(now.saturating_sub(options.window.as_ms()))?;
    let until = crate::rows::integer(now)?;
    let transcripts = tx.query_row(
        "SELECT COUNT(*) FROM transcripts WHERE indexed_ms>=?1",
        [since],
        |row| super::store::unsigned(row.get(0)?),
    )?;
    let indexed_ms = tx
        .query_row("SELECT indexed_ms FROM index_state WHERE id=1", [], |row| {
            super::store::unsigned(row.get(0)?)
        })
        .optional()?;
    let mut report = MetricsReport {
        version: METRICS_VERSION,
        window: options.window,
        tool_group: options.group,
        run_group: options.run_group,
        until_ms: now,
        hosts: vec![HostCoverage {
            host: crate::hosts::LOCAL.into(),
            transcripts,
            indexed_ms,
            error: None,
        }],
        tools: Vec::new(),
        owner_waits: Vec::new(),
        runs: Vec::new(),
    };
    tools(&tx, options, since, until, &mut report.tools)?;
    waits(&tx, since, until, &mut report.owner_waits)?;
    runs(&tx, report.run_group, since, until, &mut report.runs)?;
    tx.commit()?;
    sort(&mut report);
    Ok(report)
}

fn tools(
    connection: &Connection,
    options: &Options,
    since: i64,
    until: i64,
    rows: &mut Vec<ToolRow>,
) -> Result<()> {
    let group = match options.group {
        ToolGroup::Tool => "NULL",
        ToolGroup::Repository => "t.repository",
        ToolGroup::Model => "c.model",
        ToolGroup::Host => "'local'",
        ToolGroup::Lead => "t.lead",
        ToolGroup::Harness => "t.harness",
    };
    let mut statement = connection.prepare(&format!("SELECT c.tool,{group},c.ended_ms,c.error,c.started_ms FROM tool_calls c JOIN transcripts t ON t.path=c.path WHERE c.started_ms>=?1 AND c.started_ms<=?2 AND (?3 IS NULL OR c.tool=?3)"))?;
    let mut values = statement.query(params![since, until, options.tool])?;
    while let Some(value) = values.next()? {
        let tool: String = value.get(0)?;
        let group = if options.group == ToolGroup::Tool {
            None
        } else {
            Some(
                value
                    .get::<_, Option<String>>(1)?
                    .unwrap_or_else(|| UNATTRIBUTED.into()),
            )
        };
        let index = rows
            .iter()
            .position(|row| row.tool == tool && row.group == group)
            .unwrap_or_else(|| {
                rows.push(ToolRow {
                    tool,
                    group,
                    calls: 0,
                    errors: 0,
                    unknown_status: 0,
                    running: 0,
                    duration: Histogram::default(),
                });
                rows.len() - 1
            });
        let row = &mut rows[index];
        row.calls += 1;
        let ended: Option<i64> = value.get(2)?;
        let error: Option<bool> = value.get(3)?;
        if let Some(ended) = ended.filter(|ended| *ended <= until) {
            row.errors += u64::from(error == Some(true));
            row.unknown_status += u64::from(error.is_none());
            let started: i64 = value.get(4)?;
            if let Ok(duration) = u64::try_from(ended - started) {
                row.duration.record(duration);
            }
        } else {
            row.running += 1;
        }
    }
    Ok(())
}

fn waits(connection: &Connection, since: i64, until: i64, rows: &mut Vec<WaitRow>) -> Result<()> {
    for (name, kind) in [
        ("question", WaitKind::Question),
        ("plan_review", WaitKind::PlanReview),
        ("ask", WaitKind::Ask),
        ("blocked", WaitKind::Blocked),
    ] {
        let mut row = WaitRow {
            kind,
            answered: 0,
            open: 0,
            wait: Histogram::default(),
        };
        let mut statement = connection.prepare("SELECT started_ms,ended_ms FROM owner_waits WHERE kind=?1 AND started_ms>=?2 AND started_ms<=?3")?;
        let mut values = statement.query(params![name, since, until])?;
        while let Some(value) = values.next()? {
            let started: i64 = value.get(0)?;
            let ended: Option<i64> = value.get(1)?;
            if let Some(ended) = ended.filter(|ended| *ended <= until) {
                row.answered += 1;
                if let Ok(duration) = u64::try_from(ended - started) {
                    row.wait.record(duration);
                }
            } else {
                row.open += 1;
            }
        }
        rows.push(row);
    }
    Ok(())
}

fn runs(
    connection: &Connection,
    group: RunGroup,
    since: i64,
    until: i64,
    rows: &mut Vec<RunRow>,
) -> Result<()> {
    let group = match group {
        RunGroup::Profile => "profile",
        RunGroup::Model => "model",
        RunGroup::Harness => "harness",
    };
    let mut statement = connection.prepare(&format!("SELECT {group},outcome,queued_ms,created_ms,confirmed_ms,done_ms FROM runs WHERE created_ms>=?1 AND created_ms<=?2"))?;
    let mut values = statement.query(params![since, until])?;
    while let Some(value) = values.next()? {
        let group = value
            .get::<_, Option<String>>(0)?
            .unwrap_or_else(|| UNATTRIBUTED.into());
        let index = rows
            .iter()
            .position(|row| row.group == group)
            .unwrap_or_else(|| {
                rows.push(RunRow {
                    group,
                    outcomes: Outcomes::default(),
                    queue_wait: Histogram::default(),
                    spawn: Histogram::default(),
                    to_done: Histogram::default(),
                });
                rows.len() - 1
            });
        let row = &mut rows[index];
        match value.get::<_, String>(1)?.as_str() {
            "running" => row.outcomes.running += 1,
            "done" => row.outcomes.done += 1,
            "ask" => row.outcomes.ask += 1,
            "blocked" => row.outcomes.blocked += 1,
            "closed" => row.outcomes.closed += 1,
            "dropped" => row.outcomes.dropped += 1,
            outcome => return Err(Error::Invalid(format!("invalid metrics outcome {outcome}"))),
        }
        let queued: Option<i64> = value.get(2)?;
        let created: i64 = value.get(3)?;
        let confirmed: Option<i64> = value.get(4)?;
        let done: Option<i64> = value.get(5)?;
        duration(&mut row.queue_wait, queued, Some(created), until);
        duration(&mut row.spawn, Some(created), confirmed, until);
        duration(&mut row.to_done, confirmed, done, until);
    }
    Ok(())
}

fn duration(histogram: &mut Histogram, start: Option<i64>, end: Option<i64>, until: i64) {
    if let Some((start, end)) = start.zip(end).filter(|(_, end)| *end <= until)
        && let Ok(duration) = u64::try_from(end - start)
    {
        histogram.record(duration);
    }
}

pub(crate) fn merge(report: &mut MetricsReport, mut peer: MetricsReport, host: &str) -> Result<()> {
    if peer.version != report.version
        || peer.window != report.window
        || peer.tool_group != report.tool_group
        || peer.run_group != report.run_group
        || peer.hosts.len() != 1
    {
        return Err(Error::Invalid("incompatible peer metrics report".into()));
    }
    for coverage in &mut peer.hosts {
        host.clone_into(&mut coverage.host);
    }
    report.hosts.extend(peer.hosts);
    for mut row in peer.tools {
        if report.tool_group == ToolGroup::Host {
            row.group = Some(host.to_owned());
        }
        if let Some(target) = report
            .tools
            .iter_mut()
            .find(|target| target.tool == row.tool && target.group == row.group)
        {
            target.calls += row.calls;
            target.errors += row.errors;
            target.unknown_status += row.unknown_status;
            target.running += row.running;
            target.duration.merge(&row.duration);
        } else {
            report.tools.push(row);
        }
    }
    for row in peer.owner_waits {
        if let Some(target) = report
            .owner_waits
            .iter_mut()
            .find(|target| target.kind == row.kind)
        {
            target.answered += row.answered;
            target.open += row.open;
            target.wait.merge(&row.wait);
        } else {
            report.owner_waits.push(row);
        }
    }
    for row in peer.runs {
        if let Some(target) = report
            .runs
            .iter_mut()
            .find(|target| target.group == row.group)
        {
            target.outcomes.merge(&row.outcomes);
            target.queue_wait.merge(&row.queue_wait);
            target.spawn.merge(&row.spawn);
            target.to_done.merge(&row.to_done);
        } else {
            report.runs.push(row);
        }
    }
    sort(report);
    Ok(())
}

fn sort(report: &mut MetricsReport) {
    report.tools.sort_by(|left, right| {
        right
            .calls
            .cmp(&left.calls)
            .then_with(|| left.tool.cmp(&right.tool))
            .then_with(|| left.group.cmp(&right.group))
    });
    report
        .runs
        .sort_by(|left, right| left.group.cmp(&right.group));
}

fn human(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1_000.0)
    } else if ms < 3_600_000 {
        format!("{:.1} min", ms as f64 / 60_000.0)
    } else {
        format!("{:.1} h", ms as f64 / 3_600_000.0)
    }
}

fn quantiles(histogram: &Histogram) -> String {
    if histogram.count() == 0 {
        return "-\t-\t-".into();
    }
    format!(
        "{}\t{}\t{}",
        histogram.quantile(0.5).map_or_else(|| "-".into(), human),
        histogram.quantile(0.95).map_or_else(|| "-".into(), human),
        human(histogram.max_ms)
    )
}

pub(crate) fn render(report: &MetricsReport) -> String {
    use std::fmt::Write as _;
    let mut text = String::from("Tools\nTool\tGroup\tCalls\tError %\tRunning\tp50\tp95\tMax\n");
    for row in &report.tools {
        let known = row
            .calls
            .saturating_sub(row.unknown_status)
            .saturating_sub(row.running);
        let error = if known == 0 {
            "-".into()
        } else {
            format!("{:.1}", 100.0 * row.errors as f64 / known as f64)
        };
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{error}\t{}\t{}",
            row.tool,
            row.group.as_deref().unwrap_or("-"),
            row.calls,
            row.running,
            quantiles(&row.duration)
        );
    }
    text.push_str("\nOwner wait\nKind\tAnswered\tOpen\tp50\tp95\tMax\n");
    for row in &report.owner_waits {
        let kind = match row.kind {
            WaitKind::Question => "question",
            WaitKind::PlanReview => "plan_review",
            WaitKind::Ask => "ask",
            WaitKind::Blocked => "blocked",
        };
        let _ = writeln!(
            text,
            "{kind}\t{}\t{}\t{}",
            row.answered,
            row.open,
            quantiles(&row.wait)
        );
    }
    text.push_str("\nRuns\nGroup\tRunning\tDone\tAsk\tBlocked\tClosed\tDropped\tQueue p50\tQueue p95\tQueue max\tSpawn p50\tSpawn p95\tSpawn max\tDone p50\tDone p95\tDone max\n");
    for row in &report.runs {
        let o = row.outcomes;
        let _ = writeln!(
            text,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.group,
            o.running,
            o.done,
            o.ask,
            o.blocked,
            o.closed,
            o.dropped,
            quantiles(&row.queue_wait),
            quantiles(&row.spawn),
            quantiles(&row.to_done)
        );
    }
    for host in &report.hosts {
        if let Some(error) = &host.error {
            let _ = writeln!(text, "{}: {error}", host.host);
        }
    }
    text
}
