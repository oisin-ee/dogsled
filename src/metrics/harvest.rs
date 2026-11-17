//! Read host work state once, then write only the derived database.
use std::collections::{BTreeMap, BTreeSet};

use rusqlite::{Connection, params};

use super::index::Binding;
use crate::error::Result;
use crate::items::Item;
use crate::ledger::Ledger;

#[derive(Debug)]
struct Wait {
    id: String,
    kind: String,
    pane: String,
    cwd: Option<String>,
    started: u64,
    ended: Option<u64>,
}

#[derive(Debug)]
struct Ticket {
    queued: u64,
    origin: Option<String>,
}

#[derive(Debug)]
pub(super) struct Source {
    items: Vec<Item>,
    leads: BTreeMap<String, String>,
    tickets: BTreeMap<String, Ticket>,
    waits: Vec<Wait>,
}

impl Source {
    pub(super) fn read(ledger: &Ledger) -> Result<Self> {
        crate::store::read(ledger, |tx| {
            let items = crate::tables::items(tx)?;
            let leads = tx
                .prepare("SELECT pane,name FROM leads WHERE pane IS NOT NULL")?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            let tickets = tx
                .prepare("SELECT request_id,created_ms,repository_origin FROM tickets")?
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        Ticket {
                            queued: super::store::unsigned(row.get(1)?)?,
                            origin: row.get(2)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let waits = tx.prepare("SELECT id,'question' AS kind,pane,cwd,created_ms,answered_ms FROM web_questions UNION ALL SELECT r.id,'plan_review',r.pane,r.cwd,MIN(v.submitted_ms),r.decided_ms FROM web_reviews r JOIN web_review_revisions v ON v.review=r.id GROUP BY r.id")?
                .query_map([], |row| Ok(Wait { id: row.get(0)?, kind: row.get(1)?,pane: row.get(2)?,cwd: row.get(3)?, started: super::store::unsigned(row.get(4)?)?,ended:row.get::<_,Option<i64>>(5)?.map(super::store::unsigned).transpose()? }))?
                .collect::<rusqlite::Result<_>>()?;
            Ok(Self {
                items,
                leads,
                tickets,
                waits,
            })
        })
    }

    fn lead(&self, pane: &str) -> Option<String> {
        let mut pane = pane;
        let mut seen = BTreeSet::new();
        while seen.insert(pane) {
            if let Some(name) = self.leads.get(pane) {
                return Some(name.clone());
            }
            let item = self.items.iter().rev().find(|item| item.pane == pane)?;
            if item.owner_pane.is_empty() {
                return item.name.clone();
            }
            pane = &item.owner_pane;
        }
        None
    }

    pub(super) fn bindings(&self, reports: &[crate::hosts::HostReport]) -> Vec<Binding> {
        reports
            .iter()
            .filter(|host| host.host.is_local())
            .filter_map(|host| host.probe.as_ref())
            .flat_map(|probe| &probe.agent_panes)
            .filter_map(|agent| {
                let harness = agent.agent.as_deref()?;
                if !matches!(harness, "pi" | "claude" | "codex") {
                    return None;
                }
                let path = agent.transcript.clone().or_else(|| {
                    crate::transcript::resolve(Some(harness), "id", agent.agent_session.as_deref()?)
                })?;
                let item = self.items.iter().rev().find(|item| {
                    item.host == crate::hosts::LOCAL
                        && item.pane == agent.pane
                        && item.closed_ms.is_none()
                });
                Some(Binding {
                    path,
                    harness: harness.to_owned(),
                    pane: agent.pane.clone(),
                    item: item.map(|item| item.id.clone()),
                    lead: self.lead(&agent.pane),
                })
            })
            .collect()
    }

    pub(super) fn write(
        &self,
        connection: &mut Connection,
        shell: &crate::shell::System,
    ) -> Result<()> {
        let repositories: BTreeMap<_, _> = self
            .waits
            .iter()
            .filter_map(|wait| wait.cwd.as_deref())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|cwd| Ok((cwd, super::index::repository(shell, cwd)?)))
            .collect::<Result<_>>()?;
        super::store::write(connection, |tx| {
            // A withdrawn question has no wait. Keep harvested rows after state retention.
            let present: BTreeSet<_> = self
                .waits
                .iter()
                .filter(|wait| wait.kind == "question")
                .map(|wait| wait.id.as_str())
                .collect();
            let open_questions = tx
                .prepare("SELECT id FROM owner_waits WHERE kind='question' AND ended_ms IS NULL")?
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for id in open_questions {
                if !present.contains(id.as_str()) {
                    tx.execute("DELETE FROM owner_waits WHERE id=?1", [id])?;
                }
            }
            for wait in &self.waits {
                let repository = wait
                    .cwd
                    .as_deref()
                    .and_then(|cwd| repositories.get(cwd))
                    .and_then(Option::as_deref);
                tx.execute("INSERT INTO owner_waits(id,kind,pane,repository,lead,started_ms,ended_ms) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET ended_ms=excluded.ended_ms,repository=excluded.repository,lead=excluded.lead",params![wait.id,wait.kind,wait.pane,repository,self.lead(&wait.pane),crate::rows::integer(wait.started)?,wait.ended.map(crate::rows::integer).transpose()?])?;
            }
            for item in self.items.iter().filter(|item| item.kind == "agent") {
                self.run(tx, item)?;
                if let Some(signal) = &item.signal {
                    let id = format!("{}:{}", item.id, signal.event_id);
                    tx.execute("INSERT INTO owner_waits(id,kind,pane,repository,lead,started_ms) VALUES(?1,?2,?3,(SELECT repository FROM transcripts WHERE item=?4 LIMIT 1),?5,?6) ON CONFLICT(id) DO NOTHING",params![id,signal.kind.as_str(),item.pane,item.id,self.lead(&item.pane),crate::rows::integer(signal.recorded_ms)?])?;
                }
            }
            // Close even a previously harvested signal that state.db has since cleared.
            tx.execute("UPDATE owner_waits SET ended_ms=(SELECT MIN(e.at_ms) FROM input_entries e JOIN transcripts t ON t.path=e.path WHERE substr(owner_waits.id,1,length(t.item)+1)=t.item||':' AND e.at_ms>owner_waits.started_ms) WHERE kind IN ('ask','blocked') AND ended_ms IS NULL", [])?;
            Ok(())
        })
    }

    fn run(&self, tx: &rusqlite::Transaction<'_>, item: &Item) -> Result<()> {
        let ticket = self.tickets.get(&item.id);
        let repository = ticket
            .and_then(|ticket| ticket.origin.as_deref())
            .and_then(crate::checkout::identity)
            .and_then(|identity| identity.split_once('/').map(|(_, slug)| slug.to_owned()));
        let outcome = if item.dropped_ms.is_some() {
            "dropped"
        } else if item.reply.as_ref().is_some_and(|reply| reply.done) {
            "done"
        } else if item.closed_ms.is_none() && item.signal.is_some() {
            item.signal
                .as_ref()
                .map_or("running", |signal| signal.kind.as_str())
        } else if item.closed_ms.is_some() {
            "closed"
        } else {
            "running"
        };
        tx.execute("INSERT INTO runs(item,host,harness,profile,model,role,lead,repository,queued_ms,created_ms,confirmed_ms,done_ms,closed_ms,outcome) VALUES(?1,?2,?3,?4,?5,?6,?7,COALESCE(?8,(SELECT repository FROM transcripts WHERE item=?1 LIMIT 1)),?9,?10,?11,?12,?13,?14) ON CONFLICT(item) DO UPDATE SET host=excluded.host,harness=excluded.harness,profile=excluded.profile,model=excluded.model,role=excluded.role,lead=excluded.lead,repository=excluded.repository,queued_ms=excluded.queued_ms,confirmed_ms=excluded.confirmed_ms,done_ms=excluded.done_ms,closed_ms=excluded.closed_ms,outcome=excluded.outcome",
            params![item.id,item.host,item.harness,item.profile,item.model,item.role,self.lead(&item.pane),repository,ticket.map(|ticket|crate::rows::integer(ticket.queued)).transpose()?,crate::rows::integer(item.created_ms)?,item.launch_confirmed_ms.map(crate::rows::integer).transpose()?,item.reply.as_ref().filter(|reply|reply.done).map(|reply|crate::rows::integer(reply.captured_ms)).transpose()?,item.closed_ms.map(crate::rows::integer).transpose()?,outcome])?;
        Ok(())
    }
}
