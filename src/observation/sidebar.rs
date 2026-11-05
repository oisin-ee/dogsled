//! Producer-owned sidebar classifications and preference capture.

use crate::board::{Board, Group, Row, TodoSections};
use crate::commands::HostState;
use crate::config::Thresholds;
use crate::error::Result;
use crate::hosts::HostReport;
use crate::ledger::Ledger;
use crate::sidebar::model::{Card, Data, HostRow, Level, Saved, Tone};
use std::collections::BTreeMap;

pub(crate) fn preferences(ledger: &Ledger) -> Result<BTreeMap<String, Saved>> {
    crate::store::read(ledger, |tx| {
        let owners = {
            let mut statement =
                tx.prepare_cached("SELECT owner FROM sidebar_state ORDER BY owner")?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?
        };
        let mut saved = BTreeMap::new();
        for owner in owners {
            if let Some(value) = crate::web_store::board::sidebar(tx, &owner)? {
                saved.insert(owner, value);
            }
        }
        Ok(saved)
    })
}

pub(crate) fn host(report: &HostReport, limits: &Thresholds, now: u64) -> HostRow {
    let brief = crate::commands::HostBrief::of(report);
    let memory = report.probe.as_ref().map(|p| {
        let free = p.memory.total_bytes.saturating_sub(p.memory.used_bytes) as f64;
        // The admission rule: the reserve for every held reservation plus the new agent.
        let need = limits
            .memory_reserve_bytes
            .saturating_mul(p.reservations.len() as u64 + 1) as f64;
        let level = if free < need {
            Level::Bad
        } else if free < need * 1.1 {
            Level::Warn
        } else {
            Level::Ok
        };
        (
            p.memory.used_bytes as f64 / (p.memory.total_bytes.max(1) as f64),
            level,
        )
    });
    let disk = report.probe.as_ref().map(|p| {
        let min = limits.disk_free_min_percent;
        let level = if p.disk_free_percent < min {
            Level::Bad
        } else if p.disk_free_percent < 2.0 * min {
            Level::Warn
        } else {
            Level::Ok
        };
        (p.disk_free_percent, level)
    });
    HostRow {
        label: brief.label,
        load: brief.load,
        state: brief.state,
        measured: brief.measured,
        memory,
        disk,
        silent_ms: (brief.state == HostState::Unreachable)
            .then(|| report.sampled_ms.map(|sampled| now.saturating_sub(sampled)))
            .flatten(),
    }
}

impl Card {
    pub fn from_row(row: &Row) -> Self {
        let (pill, tone) = if row.group == Group::ToLand {
            ("to land", Tone::ToLand)
        } else {
            // The board publishes the lifecycle display name of an agent, so the sidebar
            // shows the same word the lifecycle owner publishes.
            match row.state.as_str() {
                "stalled" => ("stalled", Tone::Alert),
                "blocked" => ("blocked", Tone::Alert),
                "working" => ("working", Tone::Busy),
                "running" => ("running", Tone::Busy),
                "unavailable" => ("unavailable", Tone::Alert),
                "unknown" => ("unknown", Tone::Alert),
                "idle" => ("idle", Tone::Idle),
                "done" => ("done", Tone::Idle),
                "queued" => ("queued", Tone::Queued),
                "starting" => ("starting", Tone::Queued),
                state if state.starts_with("exit ") => ("failed", Tone::Alert),
                _ if row.group == Group::Attention => ("blocked", Tone::Alert),
                _ => ("queued", Tone::Queued),
            }
        };
        let harness = row.what.split(" · ").next().unwrap_or(&row.what);
        Self {
            id: row.id.clone(),
            kind: row.kind.clone(),
            name: row.name.clone(),
            harness: harness.to_owned(),
            host: row.host.clone(),
            pane: row.pane.clone(),
            group: row.group,
            pill: pill.to_owned(),
            tone,
            ahead: row.ahead.filter(|n| *n > 0),
            age_ms: row.age_ms,
            activity: row
                .activity
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" "),
        }
    }
}

impl Data {
    /// Groups in board order (to land, attention, running, waiting); newest first within one.
    pub fn from_board(board: &Board, todos: Option<TodoSections>, hosts: Vec<HostRow>) -> Self {
        let mut cards: Vec<Card> = board.rows.iter().map(Card::from_row).collect();
        cards.sort_by(|a, b| a.group.cmp(&b.group).then(a.age_ms.cmp(&b.age_ms)));
        Self {
            cards,
            todos,
            hosts,
        }
    }
}
