//! Host-local switches and decisions, and the rules for resolving their fleet view.

use portal_types::autonomy::{AutonomyCause, DecideBody, DecisionSource, DecisionView};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::error::{Error, Result};
use crate::ledger::Ledger;
use crate::rows::{self, required, text};
use crate::shell::Shell;
use crate::{inbox, store};

pub(crate) const SCHEMA: &str = "
CREATE TABLE autonomy_fleet (
    id TEXT PRIMARY KEY CHECK(id = 'local'),
    on_ms INTEGER NOT NULL CHECK(on_ms >= 0)
) STRICT;
CREATE TABLE autonomy_agents (
    pane TEXT NOT NULL, session TEXT NOT NULL, on_ms INTEGER NOT NULL CHECK(on_ms >= 0),
    PRIMARY KEY(pane, session)
) STRICT;
CREATE TABLE autonomy_decisions (
    seq INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE,
    pane TEXT NOT NULL, session TEXT NOT NULL, name TEXT, owner_host TEXT, owner_pane TEXT,
    created_ms INTEGER NOT NULL CHECK(created_ms >= 0),
    source TEXT NOT NULL CHECK(source IN ('question','plan','agent')),
    title TEXT NOT NULL, decision TEXT NOT NULL, why TEXT NOT NULL,
    question TEXT, alternatives TEXT, review TEXT
) STRICT;
";

pub(crate) fn fleet(ledger: &Ledger) -> Result<bool> {
    store::read(ledger, |tx| {
        Ok(
            tx.query_row("SELECT EXISTS(SELECT 1 FROM autonomy_fleet)", [], |row| {
                row.get(0)
            })?,
        )
    })
}

pub(crate) fn set_fleet(ledger: &Ledger, on: bool, now: u64) -> Result<()> {
    store::write(ledger, |tx| {
        if on {
            tx.execute(
                "INSERT INTO autonomy_fleet (id, on_ms) VALUES ('local', ?1) ON CONFLICT DO NOTHING",
                [rows::integer(now)?],
            )?;
        } else {
            tx.execute("DELETE FROM autonomy_fleet WHERE id = 'local'", [])?;
        }
        Ok(())
    })
}

pub(crate) fn agents(ledger: &Ledger) -> Result<Vec<(String, String)>> {
    store::read(ledger, |tx| {
        let mut statement =
            tx.prepare_cached("SELECT pane, session FROM autonomy_agents ORDER BY pane, session")?;
        Ok(statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    })
}

pub(crate) fn set_agent(
    ledger: &Ledger,
    pane: &str,
    session: &str,
    on: bool,
    now: u64,
) -> Result<()> {
    store::write(ledger, |tx| {
        if on {
            tx.execute(
                "DELETE FROM autonomy_agents WHERE pane = ?1 AND session <> ?2",
                params![pane, session],
            )?;
            tx.execute(
                "INSERT INTO autonomy_agents (pane, session, on_ms) VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
                params![pane, session, rows::integer(now)?],
            )?;
        } else {
            tx.execute(
                "DELETE FROM autonomy_agents WHERE pane = ?1 AND session = ?2",
                params![pane, session],
            )?;
        }
        Ok(())
    })
}

pub(crate) const VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Extension {
    pub(crate) fleet: bool,
    pub(crate) agents: Vec<String>,
    pub(crate) decisions: Vec<DecisionView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostExtension {
    Ready(Extension),
    Unavailable,
    Unsupported,
}

impl HostExtension {
    pub(crate) fn decode(host: &Value) -> Self {
        let Some(value) = host.get("autonomy") else {
            return Self::Unsupported;
        };
        if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
            return Self::Unsupported;
        }
        Extension::deserialize(value).map_or(Self::Unavailable, Self::Ready)
    }
}

pub(crate) type Pane<'a> = (&'a str, &'a str);

/// A worker inherits autonomy from a resolved owner. Walk outwards from switched-on
/// panes; an unseeded ownership cycle never resolves, and each pane is visited once.
/// Only a host that publishes the extension enforces autonomy, so only its panes get a
/// cause: a pane on an older host still asks its owner, whatever the switches say.
pub(crate) fn resolve<'a>(
    panes: impl IntoIterator<Item = Pane<'a>>,
    extensions: impl IntoIterator<Item = (&'a str, &'a Extension)>,
    executions: impl IntoIterator<Item = (&'a str, &'a crate::observation::owned_work::Execution)>,
    labels_by_hostname: &BTreeMap<&str, &'a str>,
) -> BTreeMap<Pane<'a>, AutonomyCause> {
    use crate::observation::owned_work::Kind;
    use std::collections::btree_map::Entry;

    let mut causes = BTreeMap::new();
    let mut fleet = false;
    let mut enforcing = std::collections::BTreeSet::new();
    for (host, extension) in extensions {
        enforcing.insert(host);
        fleet |= extension.fleet;
        causes.extend(
            extension
                .agents
                .iter()
                .map(|pane| ((host, pane.as_str()), AutonomyCause::Agent)),
        );
    }
    if fleet {
        return panes
            .into_iter()
            .chain(causes.into_keys())
            .chain(executions.into_iter().filter_map(|(host, execution)| {
                (execution.kind == Kind::Agent).then_some((host, execution.pane.as_str()))
            }))
            .filter(|(host, _)| enforcing.contains(host))
            .map(|pane| (pane, AutonomyCause::Fleet))
            .collect();
    }
    let mut workers = BTreeMap::<_, Vec<_>>::new();
    for (host, execution) in executions {
        if execution.kind == Kind::Agent
            && let Some(&owner_host) = labels_by_hostname.get(execution.owner_hostname.as_str())
        {
            workers
                .entry((owner_host, execution.owner_pane.as_str()))
                .or_default()
                .push((host, execution.pane.as_str()));
        }
    }
    let mut active: Vec<_> = causes.keys().copied().collect();
    while let Some(owner) = active.pop() {
        for &pane in workers.get(&owner).into_iter().flatten() {
            if let Entry::Vacant(entry) = causes.entry(pane) {
                entry.insert(AutonomyCause::Owner);
                active.push(pane);
            }
        }
    }
    causes.retain(|(host, _), _| enforcing.contains(host));
    causes
}

/// Decision ownership is recorded with the machine hostname, not its portal alias.
pub(crate) fn decision_owner(decision: &DecisionView) -> Option<Pane<'_>> {
    Some((
        decision.owner_host.as_deref()?,
        decision.owner_pane.as_deref()?,
    ))
}

pub(crate) const VIEW: &str = "autonomy";
pub(crate) const GAP_PREFIX: &str = "autonomy:";

#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct PublishedAutonomy {
    pub(crate) panes: BTreeMap<String, AutonomyCause>,
    pub(crate) decisions: Vec<DecisionView>,
}

impl PublishedAutonomy {
    pub(crate) fn cause_of(&self, pane: &str) -> Option<AutonomyCause> {
        self.panes.get(pane).copied()
    }
}

#[cfg_attr(
    not(test),
    expect(dead_code, reason = "used by the autonomy enforcement unit")
)]
pub(crate) fn cause_of(ledger: &Ledger, pane: &str) -> Result<Option<AutonomyCause>> {
    use crate::observation::{ipc, read};
    let published = ipc::Client::at(&ledger.dir).current(&ipc::Select::views([VIEW]))?;
    Ok(read::view::<PublishedAutonomy>(&published, VIEW)?.cause_of(pane))
}

pub(crate) fn record(
    ledger: &Ledger,
    shell: &dyn Shell,
    pane: &str,
    body: &DecideBody,
    source: DecisionSource,
    review: Option<&str>,
    now: u64,
) -> Result<DecisionView> {
    record_observed(ledger, body, source, review, now, || {
        inbox::local_agent(shell, pane)
    })
}

fn record_observed(
    ledger: &Ledger,
    body: &DecideBody,
    source: DecisionSource,
    review: Option<&str>,
    now: u64,
    observe: impl FnOnce() -> Result<crate::probe::Agent>,
) -> Result<DecisionView> {
    if let Some(reason) = body.refusal() {
        return Err(Error::Invalid(reason));
    }
    let agent = observe()?;
    let session = agent
        .agent_session
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::Invalid(format!("Herdr reports no agent session for {}", agent.pane))
        })?;
    let (name, owner_host, owner_pane) = match crate::launch::observed_agent_launch(ledger, &agent)?
    {
        Some(crate::work::Launch {
            action: crate::work::Action::Agent { name, .. },
            owner_host,
            owner,
            ..
        }) => (
            Some(name),
            owner_host.filter(|host| !host.is_empty()),
            Some(owner).filter(|pane| !pane.is_empty()),
        ),
        _ => (None, None, None),
    };
    let decision = DecisionView {
        id: crate::ledger::new_id(),
        host: inbox::hostname(),
        pane: agent.pane,
        name,
        owner_host,
        owner_pane,
        created_ms: now,
        source,
        title: body.title.trim().to_owned(),
        decision: body.decision.clone(),
        why: body.why.clone(),
        question: body.question.clone(),
        alternatives: body.alternatives.clone(),
        review: review.map(str::to_owned),
    };
    store::write(ledger, |tx| {
        tx.execute(
            "INSERT INTO autonomy_decisions
             (id, pane, session, name, owner_host, owner_pane, created_ms, source, title, decision, why, question, alternatives, review)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                decision.id, decision.pane, session, decision.name, decision.owner_host,
                decision.owner_pane, rows::integer(now)?, rows::name(&decision.source)?,
                decision.title, decision.decision, decision.why, decision.question,
                decision.alternatives, decision.review,
            ],
        )?;
        tx.execute(
            "DELETE FROM autonomy_decisions WHERE seq <= (
                 SELECT seq FROM autonomy_decisions ORDER BY seq DESC LIMIT 1 OFFSET 100
             )",
            [],
        )?;
        Ok(())
    })?;
    Ok(decision)
}

pub(crate) fn decisions(ledger: &Ledger, pane: Option<&str>) -> Result<Vec<DecisionView>> {
    let host = inbox::hostname();
    store::read(ledger, |tx| {
        let mut statement = tx.prepare_cached(
            "SELECT * FROM autonomy_decisions WHERE ?1 IS NULL OR pane = ?1 ORDER BY seq DESC",
        )?;
        let mut rows = statement.query([pane])?;
        let mut decisions = Vec::new();
        while let Some(row) = rows.next()? {
            decisions.push(DecisionView {
                id: required(row, "id")?,
                host: host.clone(),
                pane: required(row, "pane")?,
                name: text(row, "name")?,
                owner_host: text(row, "owner_host")?,
                owner_pane: text(row, "owner_pane")?,
                created_ms: rows::unsigned(row.get("created_ms")?)?,
                source: rows::variant(required(row, "source")?)?,
                title: required(row, "title")?,
                decision: required(row, "decision")?,
                why: required(row, "why")?,
                question: text(row, "question")?,
                alternatives: text(row, "alternatives")?,
                review: text(row, "review")?,
            });
        }
        Ok(decisions)
    })
}

#[cfg(test)]
mod tests {
    use portal_types::questions::{Pending, ResponseItem};
    use serde_json::json;

    use super::*;
    use crate::web::store::Entry;
    use crate::web_store::questions;

    #[test]
    fn autonomy_resolve_fleet_agent_owner_and_cycles() {
        use crate::observation::owned_work::{Execution, Kind};
        let mut a = Extension {
            fleet: false,
            agents: vec!["lead".into()],
            decisions: vec![],
        };
        let mut b = Extension {
            fleet: false,
            agents: vec![],
            decisions: vec![],
        };
        let execution = |pane: &str, owner_hostname: &str, owner_pane: &str| Execution {
            request_id: pane.into(),
            kind: Kind::Agent,
            owner_hostname: owner_hostname.into(),
            owner_pane: owner_pane.into(),
            pane: pane.into(),
            title: pane.into(),
            job_label: None,
            turn_clock: None,
        };
        let worker = execution("worker", "host-a", "lead");
        let nested = execution("nested", "host-b", "worker");
        let cycle_a = execution("cycle-a", "host-b", "cycle-b");
        let cycle_b = execution("cycle-b", "host-a", "cycle-a");
        let executions = [
            ("B", &worker),
            ("B", &nested),
            ("A", &cycle_a),
            ("B", &cycle_b),
        ];
        let labels = BTreeMap::from([("host-a", "A"), ("host-b", "B")]);
        let panes = [
            ("A", "lead"),
            ("B", "lead"),
            ("B", "worker"),
            ("B", "nested"),
            ("A", "cycle-a"),
            ("B", "cycle-b"),
        ];
        assert_eq!(
            resolve(panes, [("A", &a), ("B", &b)], executions, &labels),
            BTreeMap::from([
                (("A", "lead"), AutonomyCause::Agent),
                (("B", "worker"), AutonomyCause::Owner),
                (("B", "nested"), AutonomyCause::Owner),
            ])
        );
        // Host B runs an older release: it publishes no extension and enforces nothing.
        assert_eq!(
            resolve(panes, [("A", &a)], executions, &labels),
            BTreeMap::from([(("A", "lead"), AutonomyCause::Agent)])
        );
        b.fleet = true;
        assert_eq!(
            resolve(panes, [("A", &a), ("B", &b)], executions, &labels),
            panes
                .into_iter()
                .map(|pane| (pane, AutonomyCause::Fleet))
                .collect()
        );
        a.fleet = true;
        assert_eq!(
            resolve(panes, [("A", &a)], executions, &labels),
            BTreeMap::from([
                (("A", "lead"), AutonomyCause::Fleet),
                (("A", "cycle-a"), AutonomyCause::Fleet),
            ])
        );
    }

    #[test]
    fn autonomy_upgrades_v8_without_losing_questions_or_answers() -> Result<()> {
        let ledger = Ledger::temporary();
        std::fs::create_dir_all(&ledger.dir)?;
        let mut conn = rusqlite::Connection::open(ledger.dir.join("state.db"))?;
        let v8 = store::schema().replace(SCHEMA, "").replace(
            "'pending','answered','rejected','released'",
            "'pending','answered','rejected'",
        );
        conn.execute_batch(&v8)?;
        conn.pragma_update(None, "user_version", 8)?;
        let pending = Entry {
            pending: Pending {
                id: "pending".into(),
                host: "local".into(),
                pane: "p1".into(),
                session: "session".into(),
                cwd: "/repo".into(),
                created_ms: 1,
                questions: serde_json::from_value(json!({
                    "questions": [{"id":"q","type":"text","question":"Which format?"}]
                }))?,
            },
            answers: None,
            rejection: None,
            answered_ms: None,
        };
        let answered = Entry {
            pending: Pending {
                id: "answered".into(),
                ..pending.pending.clone()
            },
            answers: Some(vec![ResponseItem {
                id: "q".into(),
                value: json!("CSV"),
                attachments: Some(vec!["/example.csv".into()]),
            }]),
            rejection: None,
            answered_ms: Some(2),
        };
        let tx = conn.transaction()?;
        questions::put(&tx, &pending)?;
        questions::put(&tx, &answered)?;
        tx.commit()?;
        assert!(
            conn.execute(
                "UPDATE web_questions SET state='released', answered_ms=3 WHERE id='pending'",
                []
            )
            .is_err()
        );
        drop(conn);

        assert!(!fleet(&ledger)?);
        assert_eq!(agents(&ledger)?, []);
        assert_eq!(decisions(&ledger, None)?, []);
        store::read(&ledger, |tx| {
            let version: i64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
            assert_eq!(version, 9);
            assert_eq!(
                questions::pending(tx)?,
                std::slice::from_ref(&pending.pending)
            );
            assert_eq!(
                serde_json::to_value(questions::get(tx, "pending")?)?,
                json!(pending)
            );
            assert_eq!(
                serde_json::to_value(questions::get(tx, "answered")?)?,
                json!(answered)
            );
            assert!(!tx.prepare("PRAGMA foreign_key_check")?.exists([])?);
            Ok(())
        })?;
        store::write(&ledger, |tx| {
            assert_eq!(
                tx.execute(
                    "UPDATE web_questions SET state='released', answered_ms=3 WHERE id='pending'",
                    []
                )?,
                1
            );
            assert_eq!(questions::pending(tx)?, []);
            Ok(())
        })
    }

    #[test]
    fn autonomy_switches_round_trip_and_replace_an_old_session() -> Result<()> {
        let ledger = Ledger::temporary();
        assert!(!fleet(&ledger)?);
        assert_eq!(agents(&ledger)?, []);
        set_fleet(&ledger, true, 1)?;
        set_agent(&ledger, "p1", "old", true, 2)?;
        set_agent(&ledger, "p2", "other", true, 3)?;
        store::release();
        assert!(fleet(&ledger)?);
        assert_eq!(
            agents(&ledger)?,
            [("p1".into(), "old".into()), ("p2".into(), "other".into())]
        );
        set_agent(&ledger, "p1", "new", true, 4)?;
        set_agent(&ledger, "p1", "old", false, 5)?;
        set_fleet(&ledger, false, 6)?;
        assert!(!fleet(&ledger)?);
        assert_eq!(
            agents(&ledger)?,
            [("p1".into(), "new".into()), ("p2".into(), "other".into())]
        );
        set_agent(&ledger, "p1", "new", false, 7)?;
        assert_eq!(agents(&ledger)?, [("p2".into(), "other".into())]);
        Ok(())
    }

    #[test]
    fn autonomy_keeps_the_newest_100_decisions_and_refuses_a_multiline_title() -> Result<()> {
        let ledger = Ledger::temporary();
        let body = DecideBody {
            title: "Finish the CSV export".into(),
            decision: "Write the header.".into(),
            why: "Readers use column names.".into(),
            question: Some("Should the export have a header?".into()),
            alternatives: Some("Write data only.".into()),
        };
        let agent = crate::probe::Agent {
            pane: "p1".into(),
            agent_session: Some("session".into()),
            ..Default::default()
        };
        let mut saved = Vec::new();
        for n in 1..=101 {
            let mut observed = agent.clone();
            if n == 2 {
                observed.pane = "p2".into();
            }
            saved.push(record_observed(
                &ledger,
                &body,
                DecisionSource::Agent,
                None,
                n,
                || Ok(observed),
            )?);
        }
        let bad = DecideBody {
            title: "First line\nSecond line".into(),
            ..body
        };
        let error = record_observed(&ledger, &bad, DecisionSource::Agent, None, 102, || {
            Ok(agent)
        });
        assert!(
            matches!(error, Err(Error::Invalid(reason)) if Some(reason.as_str()) == bad.refusal().as_deref())
        );
        saved.remove(0);
        saved.reverse();
        store::release();
        assert_eq!(decisions(&ledger, None)?, saved);
        assert_eq!(decisions(&ledger, Some("p2"))?, saved[99..]);
        Ok(())
    }
}
