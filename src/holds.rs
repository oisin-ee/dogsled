//! Read-only resource ownership reports. Release decisions stay with each store.

use crate::error::Result;
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hold {
    pub host: String,
    pub resource: String,
    pub owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pane: Option<String>,
    pub code: String,
    pub reason: String,
    /// Run this command on `command_host`, as the recorded owner where required.
    pub next_command: String,
    pub command_host: String,
    pub command_argv: Vec<String>,
    #[serde(default)]
    pub request_id: Option<String>,
}

struct Rule {
    code: &'static str,
    reason: &'static str,
    command: &'static [&'static str],
    evidence: &'static [&'static str],
}

// Reporting policy only. A new reason is one entry; none changes release authority.
const RULES: &[Rule] = &[
    Rule {
        code: "preparation_claim",
        evidence: &[],
        reason: "matching preparation disposition missing; retirement releases ownership, not the task",
        command: &[
            "dogsled",
            "retire-preparation",
            "{id}",
            "--host",
            "{host}",
            "--reason",
            "release reported historical preparation claim",
        ],
    },
    Rule {
        code: "preparation_fenced",
        evidence: &[],
        reason: "preparation disposition is fenced; matching claim release remains pending",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "admission",
        evidence: &[],
        reason: "admission progress retained; execution outcome is not settled by inspection",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "queued_admission",
        evidence: &[],
        reason: "pending admission retains its request",
        command: &["dogsled", "cancel", "{id}"],
    },
    Rule {
        code: "placement",
        evidence: &[],
        reason: "placement journal awaits same-request replay",
        command: &["dogsled", "place", "--resume", "{id}"],
    },
    Rule {
        code: "placement_grant",
        evidence: &[],
        reason: "host-owned placement grant retained for same-request replay",
        command: &["dogsled", "place", "--resume", "{id}"],
    },
    Rule {
        code: "capacity",
        evidence: &[],
        reason: "admission reservation holds capacity until release or expiry",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "cancellation",
        evidence: &[],
        reason: "cancel intent retained; matching release acknowledgement pending",
        command: &["dogsled", "cancel", "{id}"],
    },
    Rule {
        code: "cancellation_held",
        evidence: &[],
        reason: "cancelled request saved no preparation identity; release stays held until its preparation is retired",
        command: &[
            "dogsled",
            "retire-preparation",
            "{id}",
            "--reason",
            "release held cancellation preparation",
        ],
    },
    Rule {
        code: "validity",
        evidence: &[],
        reason: "immutable request validity evidence retained; age alone does not release ownership",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "ordinary_reference_release",
        evidence: &[],
        reason: "awaiting origin reference release; ordinary result is retained at the origin",
        command: &["dogsled", "serve", "--once"],
    },
    Rule {
        code: "reference_release",
        evidence: &[],
        reason: "awaiting origin reference release acknowledgement",
        command: &["dogsled", "unit", "retry-reference-releases"],
    },
    Rule {
        code: "launch_reference",
        evidence: &[],
        reason: "awaiting origin reference release; provider must release the exact execution binding",
        command: &["dogsled", "unit", "settlements"],
    },
    Rule {
        code: "release_certificate",
        evidence: &[],
        reason: "origin release certificate retained as release evidence",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "checkout",
        evidence: &[],
        reason: "open child retains its resources",
        command: &["dogsled", "close", "{pane}", "--host", "{host}"],
    },
    Rule {
        code: "registered_checkout",
        evidence: &[],
        reason: "managed checkout registration retained; cleanup must revalidate ownership and content",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "job_checkout",
        evidence: &[],
        reason: "nonterminal job owns its checkout",
        command: &["dogsled", "stop", "{id}"],
    },
    Rule {
        code: "retained_branch",
        evidence: &[],
        reason: "closed child branch retained for integration or explicit drop",
        command: &["dogsled", "drop", "{id}", "--host", "{host}"],
    },
    Rule {
        code: "preservation",
        evidence: &[],
        reason: "checkout retained by preservation or closure refusal",
        command: &["dogsled", "close", "{pane}", "--host", "{host}"],
    },
    Rule {
        code: "unlinked",
        evidence: &[],
        reason: "checkout registration cannot prove safe removal",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "validity_unprovable",
        evidence: &[
            "job request validity is unprovable",
            "request validity changed",
        ],
        reason: "request validity is missing or changed; new execution refused",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "validity_expired",
        evidence: &["job request is older than 30 days"],
        reason: "request is older than 30 days; new execution refused",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "validity_future",
        evidence: &["job request creation time is ahead"],
        reason: "request creation time is ahead of the host clock; new execution refused",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "validity_clock",
        evidence: &["job validity clock is unavailable"],
        reason: "request validity clock is unavailable; new execution refused",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "validity_unsupported",
        evidence: &[
            "does not support job request validity",
            "unsupported non-job request validity protocol",
            "unsupported preparation validity envelope",
        ],
        reason: "request validity protocol is unsupported; new execution refused",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "completion_missing",
        evidence: &[],
        reason: "completion timestamp missing; retention is unproven",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "completion_invalid",
        evidence: &[
            "completion timestamp contradicts",
            "completion provenance changed",
            "completion identity changed",
        ],
        reason: "completion evidence is inconsistent; retention refused",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "teardown_failed",
        evidence: &[
            "failed teardown:",
            "teardown failed or unknown, checkout kept",
        ],
        reason: "teardown failed or unknown, checkout kept",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "teardown_unknown",
        evidence: &[
            "unknown teardown outcome",
            "close interrupted after teardown",
        ],
        reason: "teardown outcome is unknown; checkout kept; hook will not be rerun automatically",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "unsaved_work",
        evidence: &["unsaved work", "unique content", "local-only commits"],
        reason: "unsaved work keeps the checkout, lock, and panes",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "preservation_pending",
        evidence: &[
            "unfinished preservation operation",
            "normalization was interrupted",
            "preservation close expectation changed",
        ],
        reason: "preservation is incomplete; inspect recovery before another close",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "child_event_undelivered",
        evidence: &["child notification undelivered:"],
        reason: "child event is undelivered; its subject is missing or expired",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "child_event_superseded",
        evidence: &["child notification superseded:"],
        reason: "child event is undelivered; its subject or custody changed",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
    Rule {
        code: "unreadable",
        evidence: &[],
        reason: "resource ownership record is unreadable; no release is proven",
        command: &["dogsled", "doctor", "--host", "{host}"],
    },
];

pub fn render(holds: &[Hold]) -> Vec<String> {
    if holds.is_empty() {
        return Vec::new();
    }
    std::iter::once("Resource holds:".into())
        .chain(holds.iter().map(|hold| format!("  {}", hold.display())))
        .collect()
}

fn command_line(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| crate::shell::quote(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

fn report(code: &str, host: &str, resource: String, owner: &str, id: &str, pane: &str) -> Hold {
    let rule = RULES
        .iter()
        .find(|rule| rule.code == code)
        .unwrap_or(&RULES[RULES.len() - 1]);
    let command_argv: Vec<String> = rule
        .command
        .iter()
        .map(|arg| {
            match *arg {
                "{id}" => id,
                "{host}" => host,
                "{pane}" => pane,
                arg => arg,
            }
            .to_owned()
        })
        .collect();
    Hold {
        host: host.into(),
        resource,
        owner: owner.into(),
        owner_pane: None,
        request_id: (!id.is_empty()).then(|| id.to_owned()),
        code: code.into(),
        reason: rule.reason.into(),
        next_command: command_line(&command_argv),
        command_host: host.into(),
        command_argv,
    }
}

impl Hold {
    fn with_reason(mut self, reason: &str) -> Self {
        if self.code == "unreadable" && self.owner_pane.is_none() && self.owner == "unrecorded" {
            self.reason.push_str("; ");
            self.reason.push_str(reason);
            return self;
        }
        if let Some(rule) = RULES.iter().find(|rule| {
            rule.evidence
                .iter()
                .any(|evidence| reason.contains(evidence))
        }) {
            let mapped = report(rule.code, &self.host, String::new(), &self.owner, "", "");
            self.code = mapped.code;
            self.command_host = mapped.command_host;
            self.command_argv = mapped.command_argv;
            self.next_command = mapped.next_command;
        }
        self.reason = reason.into();
        self
    }

    pub fn display(&self) -> String {
        format!(
            "{} on {} — owner: {}; {}; next (on {}): {}",
            self.resource, self.host, self.owner, self.reason, self.command_host, self.next_command
        )
    }

    pub fn unavailable(mut self) -> Self {
        let suffix = "; last observed ownership; host unavailable, release unproven";
        if !self.reason.ends_with(suffix) {
            self.reason.push_str(suffix);
        }
        self
    }

    pub fn on_host(mut self, host: &str) -> Self {
        if self.command_host == self.host {
            self.command_host = host.into();
        }
        self.host = host.into();
        if let Some(index) = self.command_argv.iter().position(|arg| arg == "--host")
            && let Some(argument) = self.command_argv.get_mut(index + 1)
        {
            *argument = host.into();
            self.next_command = command_line(&self.command_argv);
        }
        self
    }
}

/// Read atomic records only: no execution locks, clock refresh or reconciliation.
fn records(ledger: &Ledger, directory: &str) -> Result<Vec<(String, Option<Value>)>> {
    if directory == "admissions" {
        // Admissions live in state.db; holds name each by its request id.
        return Ok(crate::admissions::entries(ledger)?
            .into_iter()
            .map(|(id, value)| (id, Some(value)))
            .collect());
    }
    macro_rules! records {
        ($module:ident, $suffix:literal) => {{
            let mut records = Vec::new();
            for id in crate::preparation_store::$module::ids(ledger)? {
                records.push((
                    format!("{id}{}", $suffix),
                    crate::preparation_store::$module::value(ledger, &id)?,
                ));
            }
            records
        }};
    }
    if directory == "placement-grants" {
        let mut records = records!(grants, ".json");
        records.extend(records!(lifecycles, ".lifecycle.json"));
        return Ok(records);
    }
    if directory == "preparation-receipts" {
        let mut records = records!(receipts, ".json");
        records.extend(records!(dispositions, ".disposition.json"));
        return Ok(records);
    }
    if directory == "job-validity" {
        return crate::job_store::validities(ledger)?
            .into_iter()
            .map(|(id, value)| Ok((format!("{id}.json"), Some(serde_json::to_value(value)?))))
            .collect();
    }
    if directory == "cancellations" {
        return crate::job_store::cancellations(ledger)?
            .into_iter()
            .map(|saved| {
                Ok((
                    format!("{}.json", saved.ticket.request.id),
                    Some(serde_json::to_value(saved)?),
                ))
            })
            .collect();
    }
    Err(crate::error::Error::Invalid(format!(
        "state.db has no {directory} records"
    )))
}

fn text<'a>(value: &'a Value, path: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or("unrecorded")
}

type Owners = BTreeMap<String, (String, String)>;

fn owner_panes<'a>(
    ledger: &Ledger,
    host: &str,
    records: impl Iterator<Item = &'a (String, Option<Value>)>,
    tickets: &[crate::work::Ticket],
    jobs: &[crate::jobs::Job],
) -> Result<Owners> {
    let mut owners = BTreeMap::new();
    for saved in crate::job_store::owners(ledger)? {
        owners.insert(
            saved.id,
            (saved.owner, saved.owner_host.unwrap_or_else(|| host.into())),
        );
    }
    for id in crate::launch_store::ids(ledger)? {
        let launch = if let Some(release) = crate::launch_store::release(ledger, &id)? {
            Some(release.certificate.launch)
        } else {
            crate::launch_store::receipt(ledger, &id)?
                .and_then(|receipt| receipt.get("launch").cloned())
                .map(serde_json::from_value::<crate::work::Launch>)
                .transpose()?
        };
        if let Some(launch) = launch {
            owners.insert(
                id,
                (
                    launch.owner,
                    launch.owner_host.unwrap_or_else(|| host.into()),
                ),
            );
        }
    }
    for item in crate::items::list(ledger)? {
        owners
            .entry(item.id)
            .or_insert_with(|| (item.owner_pane, host.into()));
    }
    for id in crate::host_store::landing(ledger)?.keys() {
        if !owners.contains_key(id)
            && let Some(custody) = crate::custody::get(ledger, id)?
        {
            owners.insert(id.clone(), (custody.custodian_pane, custody.host));
        }
    }
    for saved in crate::job_store::cancellations(ledger)? {
        let request = saved.ticket.request;
        owners.entry(request.id).or_insert((
            request.owner,
            request.owner_host.unwrap_or_else(|| host.into()),
        ));
    }
    for (id, failure) in crate::inbox::notification_failures(ledger)? {
        owners.insert(id, (failure.owner, failure.owner_host));
    }
    for (name, value) in records {
        let Some(value) = value else {
            continue;
        };
        let request = value
            .get("request")
            .or_else(|| value.pointer("/envelope/request"));
        if let Some(request) =
            request.filter(|request| request.get("owner").and_then(Value::as_str).is_some())
        {
            let id = name
                .trim_end_matches(".json")
                .trim_end_matches(".progress")
                .trim_end_matches(".lifecycle");
            owners.insert(
                id.to_owned(),
                (
                    text(request, "/owner").to_owned(),
                    request
                        .get("owner_host")
                        .and_then(Value::as_str)
                        .unwrap_or(host)
                        .to_owned(),
                ),
            );
        }
    }
    for ticket in tickets {
        owners.insert(
            ticket.request.id.clone(),
            (
                ticket.request.owner.clone(),
                ticket
                    .request
                    .owner_host
                    .clone()
                    .unwrap_or_else(|| host.into()),
            ),
        );
    }
    for job in jobs {
        owners
            .entry(job.id.clone())
            .or_insert_with(|| (job.owner.clone(), host.into()));
    }
    Ok(owners)
}

fn job_requests<'a>(
    jobs: &[crate::jobs::Job],
    tickets: &[crate::work::Ticket],
    records: impl Iterator<Item = &'a (String, Option<Value>)>,
) -> BTreeSet<String> {
    jobs.iter()
        .map(|job| job.id.clone())
        .chain(
            tickets
                .iter()
                .filter(|ticket| matches!(ticket.request.action, crate::work::Action::Job { .. }))
                .map(|ticket| ticket.request.id.clone()),
        )
        .chain(
            records
                .filter(|(_, value)| {
                    value
                        .as_ref()
                        .and_then(|value| value.pointer("/request/action/type"))
                        .and_then(Value::as_str)
                        == Some("job")
                })
                .map(|(id, _)| id.clone()),
        )
        .collect()
}

fn job_commands(
    holds: &mut [Hold],
    owners: &Owners,
    jobs: &[crate::jobs::Job],
    job_requests: &BTreeSet<String>,
) {
    for hold in holds {
        if hold
            .command_argv
            .get(1)
            .is_some_and(|command| command == "doctor")
            && !matches!(hold.code.as_str(), "completion_invalid" | "unreadable")
            && let Some(id) = &hold.request_id
            && let Some((_, origin)) = owners.get(id)
        {
            if jobs
                .iter()
                .any(|job| job.id == *id && !crate::jobs::terminal(job))
            {
                hold.command_argv = vec!["dogsled".into(), "stop".into(), id.clone()];
                hold.command_host = origin.clone();
                hold.next_command = command_line(&hold.command_argv);
            } else if job_requests.contains(id)
                && matches!(
                    hold.code.as_str(),
                    "validity" | "admission" | "completion_missing" | "release_certificate"
                )
            {
                hold.command_argv = vec!["dogsled".into(), "serve".into(), "--once".into()];
                hold.command_host = origin.clone();
                hold.next_command = command_line(&hold.command_argv);
            }
        }
    }
}

fn owner_hold(owners: &Owners, host: &str, code: &str, resource: String, id: &str) -> Hold {
    let Some((owner, origin)) = owners
        .get(id)
        .filter(|(owner, _)| !owner.is_empty() && owner != "unrecorded" && owner != id)
    else {
        let reason =
            format!("resource {resource} has no recorded owner pane; no release is proven");
        return report("unreadable", host, resource, "unrecorded", id, "").with_reason(&reason);
    };
    let mut hold = report(code, host, resource, owner, id, "");
    hold.owner_pane = Some(owner.clone());
    if matches!(
        code,
        "placement"
            | "placement_grant"
            | "reference_release"
            | "launch_reference"
            | "ordinary_reference_release"
    ) {
        hold.command_host = origin.into();
    }
    hold
}

/// The host reports existing claims; it never decides whether to release them.
pub fn local(ledger: &Ledger, host: &str, now: u64, ttl_ms: u64) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    let admissions = records(ledger, "admissions")?;
    let launches = crate::launch_store::progress_entries(ledger)?;
    let grants = records(ledger, "placement-grants")?;
    let tickets = crate::work::queue(ledger)?;
    let jobs = crate::jobs::list(ledger)?;
    let owners = owner_panes(
        ledger,
        host,
        admissions.iter().chain(&launches).chain(&grants),
        &tickets,
        &jobs,
    )?;
    let job_requests = job_requests(&jobs, &tickets, admissions.iter().chain(&launches));
    let mut settled = BTreeSet::new();
    let mut invalid = Vec::new();
    for id in owners.keys() {
        match crate::work::settled(ledger, id, now) {
            Ok(true) => {
                settled.insert(id.clone());
            }
            Ok(false) => (),
            Err(error) => invalid.push((id.clone(), error.to_string())),
        }
    }
    let owned =
        |code: &str, resource: String, id: &str| Ok(owner_hold(&owners, host, code, resource, id));
    for (id, reason) in invalid {
        holds.push(
            owned(
                "completion_invalid",
                format!("completion evidence {id}"),
                &id,
            )?
            .with_reason(&reason),
        );
    }
    holds.extend(preparations(ledger, &owned)?);
    holds.extend(capacity(ledger, host, now, ttl_ms, &tickets, &owned)?);
    for (directory, code, saved) in [
        ("placement-grants", "placement_grant", grants),
        ("job-validity", "validity", records(ledger, "job-validity")?),
    ] {
        for (name, value) in saved {
            if directory == "placement-grants" && name.ends_with(".lifecycle.json") {
                continue;
            }
            let id = name.trim_end_matches(".json");
            if settled.contains(id) {
                continue;
            }
            holds.push(owned(
                if value.is_some() { code } else { "unreadable" },
                format!("{directory}/{id}"),
                id,
            )?);
        }
    }
    for ticket in &tickets {
        if let Some(reason) = &ticket.error
            && !settled.contains(&ticket.request.id)
        {
            holds.push(
                owned(
                    "admission",
                    format!("ticket {}", ticket.id),
                    &ticket.request.id,
                )?
                .with_reason(reason),
            );
        }
    }
    holds.extend(completion_holds(ledger, now, &settled, &owned)?);
    holds.extend(preservation_holds(ledger, host)?);
    holds.extend(notification_holds(ledger, &owned)?);
    holds.extend(self::admissions(admissions, &tickets, &settled, &owned)?);
    holds.extend(releases(ledger, &launches, &settled, &owned)?);
    holds.extend(checkouts(ledger, host, &launches, &settled)?);
    holds.extend(branches(ledger, host, &owned)?);
    holds.extend(jobs_and_closures(ledger, host, &jobs, &owned)?);
    job_commands(&mut holds, &owners, &jobs, &job_requests);
    Ok(holds)
}

fn notification_holds(
    ledger: &Ledger,
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let failures = crate::inbox::notification_failures(ledger)?;
    failures
        .into_iter()
        .map(|(id, failure)| {
            Ok(
                owned("child_event_undelivered", format!("child event {id}"), &id)?
                    .with_reason(&failure.reason),
            )
        })
        .collect()
}

fn completion_holds(
    ledger: &Ledger,
    now: u64,
    settled: &BTreeSet<String>,
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    // Do not call the owners' retention predicates: those can initialize the clock.
    let clock = crate::jobs::recorded_clock(ledger).and_then(|clock| {
        clock
            .map(serde_json::to_value)
            .transpose()
            .map_err(Into::into)
    });
    let effective = clock
        .as_ref()
        .ok()
        .and_then(|value| value.as_ref())
        .and_then(|value| value["high_water_ms"].as_u64())
        .map(|time| time.max(now));
    for directory in [
        "admissions",
        "preparation-receipts",
        "launches",
        "placement-grants",
    ] {
        let saved = if directory == "launches" {
            crate::launch_store::progress_entries(ledger)?
        } else {
            records(ledger, directory)?
        };
        for (name, value) in saved {
            if directory == "placement-grants" && !name.ends_with(".lifecycle.json") {
                continue;
            }
            let Some(value) = value else { continue };
            let id = name
                .trim_end_matches(".json")
                .trim_end_matches(".progress")
                .trim_end_matches(".disposition")
                .trim_end_matches(".lifecycle");
            if settled.contains(id) {
                continue;
            }
            let finished = &value["finished_ms"];
            let created = value
                .pointer("/request/validity/created_ms")
                .or_else(|| value.pointer("/validity/created_ms"))
                .and_then(Value::as_u64);
            let code = if finished.is_null() {
                "completion_missing"
            } else if finished.as_u64().is_none_or(|time| {
                effective.is_some_and(|effective| time > effective)
                    || created.is_some_and(|created| {
                        time.saturating_add(crate::jobs::VALIDITY_SKEW_MS) < created
                    })
            }) {
                "completion_invalid"
            } else {
                // A plausible timestamp alone never proves terminal provenance or release.
                continue;
            };
            holds.push(owned(
                code,
                format!("completion evidence {directory}/{name}"),
                id,
            )?);
        }
    }
    Ok(holds)
}

fn preservation_holds(ledger: &Ledger, host: &str) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for (name, value) in crate::preservation::operations(ledger)?
        .into_iter()
        .map(|operation| Ok((operation.id.clone(), Some(serde_json::to_value(operation)?))))
        .collect::<Result<Vec<_>>>()?
    {
        let Some(value) = value else {
            holds.push(report(
                "unreadable",
                host,
                format!("preservation {name}"),
                "unrecorded",
                "",
                "",
            ));
            continue;
        };
        if value["stage"] == "finished" {
            continue;
        }
        let code = if value["stage"] == "teardown_entered" {
            "teardown_unknown"
        } else {
            "preservation_pending"
        };
        let mut hold = report(
            code,
            host,
            format!(
                "checkout {} (operation {})",
                text(&value, "/expectation/tree/path"),
                text(&value, "/id")
            ),
            text(&value, "/expectation/caller/pane"),
            "",
            text(&value, "/expectation/pane"),
        );
        if let Some(reason) = value["blocker"].as_str() {
            hold = hold.with_reason(reason);
        }
        // Inspect pinned content rather than rerun a hook with an unknown outcome.
        if let Some(snapshot) = value
            .get("latest")
            .filter(|v| !v.is_null())
            .or_else(|| value.get("initial").filter(|v| !v.is_null()))
            && let (Some(id), Some(repo)) = (
                snapshot["id"].as_str(),
                value
                    .pointer("/expectation/tree/repo")
                    .and_then(Value::as_str),
            )
        {
            hold.command_argv = vec![
                "dogsled".into(),
                "recovery".into(),
                "show".into(),
                id.into(),
                "--repo".into(),
                repo.into(),
            ];
            hold.next_command = command_line(&hold.command_argv);
        }
        holds.push(hold);
    }
    Ok(holds)
}

fn preparations(
    ledger: &Ledger,
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    let preparations: Vec<Value> = crate::preparation_store::pending(ledger)?
        .into_iter()
        .map(serde_json::to_value)
        .collect::<std::result::Result<_, _>>()?;
    for claim in preparations {
        let resource = format!("preparation claim for {}", text(&claim, "/path"));
        // Every claim names its request. One without it is a damaged record.
        let Some(id) = claim["request_id"].as_str() else {
            holds.push(owned("unreadable", resource, "unrecorded")?);
            continue;
        };
        let disposition = crate::jobs::validate_id(id)
            .and_then(|()| crate::preparation_store::dispositions::get(ledger, id));
        let code = match &disposition {
            Err(_) => "unreadable",
            Ok(Some(_)) => "preparation_fenced",
            Ok(None) => "preparation_claim",
        };
        let mut hold = owned(
            code,
            format!("preparation claim {id} for {}", text(&claim, "/path")),
            id,
        )?;
        if let Ok(Some(disposition)) = disposition {
            hold.reason = format!(
                "{} (progress: {:?}; outcome: {:?})",
                hold.reason, disposition.progress, disposition.request.outcome
            );
            if let crate::prepare::disposition::Policy::Prohibit {
                basis: crate::prepare::disposition::Basis::Retired { audit, .. },
            } = disposition.request.policy
            {
                hold.reason
                    .push_str("; retirement releases ownership only; task unresolved");
                hold.command_argv = vec![
                    "dogsled".into(),
                    "retire-preparation".into(),
                    audit.request_id,
                    "--host".into(),
                    audit.host.id,
                    "--reason".into(),
                    audit.reason,
                ];
                if matches!(
                    audit.authority,
                    crate::preparation_retirement::Authority::Cleanup
                ) {
                    hold.command_argv.push("--cleanup".into());
                }
                hold.command_host = audit.actor.host;
                hold.next_command = command_line(&hold.command_argv);
            }
        }
        holds.push(hold);
    }
    Ok(holds)
}

fn capacity(
    ledger: &Ledger,
    host: &str,
    now: u64,
    ttl_ms: u64,
    tickets: &[crate::work::Ticket],
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for reservation in ledger.reservations(now, ttl_ms)? {
        let mut hold = owned(
            "capacity",
            format!("admission capacity {}", reservation.id),
            &reservation.id,
        )?;
        if let Some(ticket) = tickets.iter().find(|t| {
            t.request.id == reservation.id && matches!(t.state.as_str(), "pending" | "admitting")
        }) {
            hold.next_command =
                command_line(&["dogsled".into(), "cancel".into(), ticket.id.clone()]);
            hold.command_argv = vec!["dogsled".into(), "cancel".into(), ticket.id.clone()];
            hold.command_host = ticket
                .request
                .owner_host
                .clone()
                .unwrap_or_else(|| host.into());
        }
        holds.push(hold);
    }
    Ok(holds)
}

fn admissions(
    admissions: Vec<(String, Option<Value>)>,
    tickets: &[crate::work::Ticket],
    settled: &BTreeSet<String>,
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for (name, value) in admissions {
        let id = name.trim_end_matches(".json");
        if settled.contains(id) {
            continue;
        }
        let Some(value) = value else {
            holds.push(owned("unreadable", format!("admission {id}"), id)?);
            continue;
        };
        if let Some(reason) = value["error"].as_str() {
            holds.push(
                owned("admission", format!("admission refusal {id}"), id)?.with_reason(reason),
            );
        }
        if let Some(phase) = value.pointer("/placement/phase").and_then(Value::as_str) {
            if phase != "completed" {
                let mut hold = owned("placement", format!("placement {id}"), id)?;
                hold.reason = format!("{} ({phase})", hold.reason);
                holds.push(hold);
            }
        } else if value.get("result").is_none_or(Value::is_null) {
            let mut hold = owned("admission", format!("admission {id}"), id)?;
            if let Some(ticket) = tickets
                .iter()
                .find(|t| t.request.id == id && matches!(t.state.as_str(), "pending" | "admitting"))
            {
                hold = owned("queued_admission", format!("admission {id}"), id)?;
                hold.command_argv = vec!["dogsled".into(), "cancel".into(), ticket.id.clone()];
                hold.next_command = command_line(&hold.command_argv);
            }
            if let Some(diagnostic) = value.get("diagnostic").filter(|d| !d.is_null()).cloned()
                && let Ok(diagnostic) =
                    serde_json::from_value::<crate::diagnostic::Diagnostic>(diagnostic)
            {
                hold.reason = diagnostic.to_string();
            }
            holds.push(hold);
        }
        if let Some(release) = value
            .get("ordinary_release")
            .filter(|release| !release.is_null())
        {
            holds.push(owned(
                if release["acknowledgement"].is_null() {
                    "ordinary_reference_release"
                } else {
                    "release_certificate"
                },
                format!("ordinary release certificate {id}"),
                id,
            )?);
        }
        if let Some(release) = value
            .pointer("/unit/reference_release")
            .filter(|r| !r.is_null())
        {
            holds.push(owned(
                if release["acknowledgement"].is_null() {
                    "reference_release"
                } else {
                    "release_certificate"
                },
                format!("release certificate {id}"),
                id,
            )?);
        }
    }
    Ok(holds)
}

fn releases(
    ledger: &Ledger,
    launches: &[(String, Option<Value>)],
    settled: &BTreeSet<String>,
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for (name, value) in records(ledger, "cancellations")? {
        let id = name.trim_end_matches(".json");
        match value {
            Some(value) if value["release"] == "pending" && value["held"].is_object() => {
                holds.push(owned(
                    "cancellation_held",
                    format!("cancellation {id}"),
                    id,
                )?);
            }
            Some(value) if value["release"] == "pending" => {
                let mut hold = owned("cancellation", format!("cancellation {id}"), id)?;
                hold.command_argv = vec![
                    "dogsled".into(),
                    "cancel".into(),
                    text(&value, "/ticket/id").into(),
                ];
                hold.next_command = command_line(&hold.command_argv);
                holds.push(hold);
            }
            None => holds.push(owned("unreadable", format!("cancellation {id}"), id)?),
            _ => (),
        }
    }
    for id in crate::launch_store::release_ids(ledger)? {
        if settled.contains(&id) {
            continue;
        }
        holds.push(owned(
            "release_certificate",
            format!("ordinary launch references {id}"),
            &id,
        )?);
    }
    for (id, value) in launches {
        if settled.contains(id) {
            continue;
        }
        if let Some(reason) = value.as_ref().and_then(|v| v["error"].as_str()) {
            holds.push(owned("admission", format!("launch refusal {id}"), id)?.with_reason(reason));
        }
        match value {
            Some(value)
                if value
                    .pointer("/request/unit")
                    .is_some_and(|unit| !unit.is_null()) =>
            {
                holds.push(owned(
                    if value["origin_release"].is_null() {
                        "launch_reference"
                    } else {
                        "release_certificate"
                    },
                    format!("launch references {id}"),
                    id,
                )?);
            }
            None => holds.push(owned("unreadable", format!("launch {id}"), id)?),
            Some(value) if value.get("request").is_some() => holds.push(owned(
                "ordinary_reference_release",
                format!("ordinary launch references {id}"),
                id,
            )?),
            _ => (),
        }
    }
    Ok(holds)
}

fn checkouts(
    ledger: &Ledger,
    host: &str,
    launches: &[(String, Option<Value>)],
    settled: &BTreeSet<String>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    let trees: Vec<crate::cleanup::Tree> = crate::host_store::trees(ledger)?;
    let requests: Vec<crate::work::Launch> = launches
        .iter()
        .filter_map(|(_, value)| {
            serde_json::from_value(value.as_ref()?.get("request")?.clone()).ok()
        })
        .collect();
    for tree in trees {
        // A registry entry locates a cleanup candidate; it is not ownership evidence.
        // Only a still-owned execution can retain this checkout.
        let Some(request) = requests
            .iter()
            .find(|request| request.cwd == tree.path && !settled.contains(&request.id))
        else {
            continue;
        };
        let mut hold = report(
            "registered_checkout",
            host,
            format!("checkout {}", tree.path),
            &request.owner,
            &request.id,
            "",
        );
        hold.owner_pane = Some(request.owner.clone());
        if let Some(pane) = request.pane.as_deref() {
            hold.command_argv = vec![
                "dogsled".into(),
                "close".into(),
                pane.into(),
                "--host".into(),
                host.into(),
            ];
            hold.next_command = command_line(&hold.command_argv);
        }
        holds.push(hold);
    }
    Ok(holds)
}

fn branches(
    ledger: &Ledger,
    host: &str,
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for mark in crate::done::list(ledger)? {
        if mark.closed_ms.is_some()
            && mark.dropped_ms.is_none()
            && let Some(branch) = mark.branch.filter(|branch| !branch.landed)
        {
            let mut hold = owned(
                "retained_branch",
                format!("branch {}", branch.name),
                mark.item.as_deref().unwrap_or("unrecorded"),
            )?;
            if mark.item.is_none() {
                hold.command_argv = vec![
                    "dogsled".into(),
                    "doctor".into(),
                    "--host".into(),
                    host.into(),
                ];
                hold.next_command = command_line(&hold.command_argv);
            }
            holds.push(hold);
        }
    }
    Ok(holds)
}

fn jobs_and_closures(
    ledger: &Ledger,
    host: &str,
    jobs: &[crate::jobs::Job],
    owned: &impl Fn(&str, String, &str) -> Result<Hold>,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for job in jobs {
        if !crate::jobs::terminal(job) {
            let mut hold = report(
                "job_checkout",
                host,
                format!("checkout {}", job.cwd),
                &job.owner,
                &job.id,
                "",
            );
            hold.owner_pane = Some(job.owner.clone());
            hold.reason = format!("{} ({})", hold.reason, job.state);
            holds.push(hold);
        }
    }
    let landing: BTreeMap<String, String> = crate::host_store::landing(ledger)?;
    for (id, reason) in landing {
        let mut hold = owned("preservation", format!("closure {id}"), &id)?.with_reason(&reason);
        // The origin item supplies the child pane and exact close command below.
        hold.command_argv = vec![
            "dogsled".into(),
            "doctor".into(),
            "--host".into(),
            host.into(),
        ];
        hold.next_command = command_line(&hold.command_argv);
        holds.push(hold);
    }
    Ok(holds)
}

/// Owner-host facts remain independent of task completion and resource closure.
pub fn owner(status: &crate::commands::Status) -> Vec<Hold> {
    let mut holds = Vec::new();
    for item in &status.items {
        if item.kind != "agent" {
            continue;
        }
        if item.closed_ms.is_none() && item.resource_closed_ms.is_none() {
            let mut hold = report(
                "checkout",
                &item.host,
                format!("child resources {}", item.pane),
                &item.owner_pane,
                &item.id,
                &item.pane,
            );
            hold.owner_pane = Some(item.owner_pane.clone());
            if let Some(reason) = &item.resource_close_reason {
                hold.code = "preservation".into();
                hold = hold.with_reason(reason);
            }
            if let Some(orphan) = status.orphans.iter().find(|o| o.item == item.id) {
                hold = hold.with_reason(&orphan.reason);
            }
            if let Some(path) = status
                .hosts
                .iter()
                .find(|host| host.host.id == item.host)
                .and_then(|host| host.agent(&item.pane))
                .and_then(|agent| agent.worktree.as_ref())
                .map(|tree| &tree.path)
            {
                hold.resource = format!("checkout {path}");
            }
            holds.push(hold);
        }
        if (item.closed_ms.is_some() || item.resource_closed_ms.is_some())
            && item.dropped_ms.is_none()
            && let Some(branch) = item.branch.as_ref().filter(|branch| !branch.landed)
        {
            let mut hold = report(
                "retained_branch",
                &item.host,
                format!("branch {}", branch.name),
                &item.owner_pane,
                &item.id,
                &item.pane,
            );
            hold.owner_pane = Some(item.owner_pane.clone());
            holds.push(hold);
        }
    }
    if let Some(sweep) = &status.sweep {
        for entry in sweep
            .applied
            .unlinked
            .iter()
            .chain(&sweep.applied.landed_dirty)
        {
            let mut hold = report(
                "unlinked",
                "local",
                format!("checkout {}", entry.id),
                "unrecorded",
                &entry.id,
                "",
            );
            hold.reason.clone_from(&entry.detail);
            holds.push(hold);
        }
        for failure in &sweep.applied.failure_entries {
            let mut hold = report(
                "unlinked",
                "local",
                format!("{} {}", failure.entry.class, failure.entry.id),
                "unrecorded",
                &failure.entry.id,
                "",
            );
            hold.reason.clone_from(&failure.detail);
            holds.push(hold);
        }
    }
    holds
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[test]
    fn every_hold_command_is_an_existing_cli_command() {
        for rule in RULES {
            let hold = report(
                rule.code,
                "local",
                "resource".into(),
                "owner",
                "request-held",
                "w2:p1",
            );
            crate::cli::Cli::try_parse_from(&hold.command_argv)
                .unwrap_or_else(|error| panic!("{}: {error}", rule.code));
        }
    }

    fn fixture() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    /// The bytes of every file under `path`, after this thread closes its cached store
    /// connections, so SQLite checkpoints and removes its WAL files as a closed process does.
    fn saved_files(path: &std::path::Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        crate::store::release();
        files_under(path)
    }

    fn files_under(path: &std::path::Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(files_under(&path));
            } else {
                files.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
        files
    }

    fn assert_reported_reason(reason: &str, code: &str) {
        use crate::admissions::fixtures;
        let ledger = fixture();
        let attempt = crate::work::Attempt {
            error: Some(reason.into()),
            ..fixtures::attempt(&fixtures::request("held", "w1:p1", Some("origin")))
        };
        crate::admissions::put(&ledger, "held", &attempt).unwrap();
        let before = saved_files(&ledger.dir);
        let holds = local(&ledger, "worker", 100, 90).unwrap();
        let hold = holds.iter().find(|hold| hold.code == code).unwrap();
        assert_eq!(hold.reason, reason);
        assert_eq!(hold.owner, "w1:p1");
        assert_eq!(hold.request_id.as_deref(), Some("held"));
        assert_eq!(hold.command_argv, ["dogsled", "doctor", "--host", "worker"]);
        assert_eq!(hold.command_host, "worker");
        assert_eq!(
            before,
            saved_files(&ledger.dir),
            "reports must not initialize clocks, acquire locks, or alter decisions"
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn validity_unprovable_has_an_exact_inspection_command() {
        for reason in [
            "job request validity is unprovable",
            "request validity changed",
        ] {
            assert_reported_reason(reason, "validity_unprovable");
        }
    }

    #[test]
    fn validity_expired_has_an_exact_inspection_command() {
        assert_reported_reason("job request is older than 30 days", "validity_expired");
    }

    #[test]
    fn validity_future_has_an_exact_inspection_command() {
        assert_reported_reason("job request creation time is ahead", "validity_future");
    }

    #[test]
    fn validity_clock_has_an_exact_inspection_command() {
        assert_reported_reason("job validity clock is unavailable", "validity_clock");
    }

    #[test]
    fn validity_unsupported_has_an_exact_inspection_command() {
        for reason in [
            "does not support job request validity",
            "unsupported non-job request validity protocol",
            "unsupported preparation validity envelope",
        ] {
            assert_reported_reason(reason, "validity_unsupported");
        }
    }

    #[test]
    fn completion_invalid_has_an_exact_inspection_command() {
        for reason in [
            "completion timestamp contradicts",
            "completion provenance changed",
            "completion identity changed",
        ] {
            assert_reported_reason(reason, "completion_invalid");
        }
    }

    #[test]
    fn teardown_failed_has_an_exact_inspection_command() {
        for reason in [
            "failed teardown:",
            "teardown failed or unknown, checkout kept",
        ] {
            assert_reported_reason(reason, "teardown_failed");
        }
    }

    #[test]
    fn teardown_unknown_has_an_exact_inspection_command() {
        for reason in [
            "unknown teardown outcome",
            "close interrupted after teardown",
        ] {
            assert_reported_reason(reason, "teardown_unknown");
        }
    }

    #[test]
    fn unsaved_work_has_an_exact_inspection_command() {
        for reason in ["unsaved work", "unique content", "local-only commits"] {
            assert_reported_reason(reason, "unsaved_work");
        }
    }

    #[test]
    fn preservation_pending_has_an_exact_inspection_command() {
        for reason in [
            "unfinished preservation operation",
            "normalization was interrupted",
            "preservation close expectation changed",
        ] {
            assert_reported_reason(reason, "preservation_pending");
        }
    }

    #[test]
    fn child_event_undelivered_has_an_exact_inspection_command() {
        assert_reported_reason("child notification undelivered:", "child_event_undelivered");
    }

    #[test]
    fn child_event_superseded_has_an_exact_inspection_command() {
        assert_reported_reason("child notification superseded:", "child_event_superseded");
    }

    #[test]
    fn finished_time_gaps_and_contradictions_remain_read_only_holds() {
        let ledger = fixture();
        crate::store::write(&ledger, |tx| {
            crate::job_store::put_clock(
                tx,
                &crate::jobs::Clock {
                    version: 1,
                    high_water_ms: 100,
                },
            )
        })
        .unwrap();
        for (id, evidence) in [
            ("missing", serde_json::json!({})),
            ("future", serde_json::json!({"finished_ms":101})),
            ("malformed", serde_json::json!({"finished_ms":"100"})),
            (
                "before-creation",
                serde_json::json!({"finished_ms":1,"validity":{"created_ms":300_002}}),
            ),
            ("plausible", serde_json::json!({"finished_ms":100})),
        ] {
            crate::store::write(&ledger, |tx| {
                crate::job_store::put_validity(tx, id, crate::jobs::Validity::new(100))
            })
            .unwrap();
            crate::job_store::remember_owner(&ledger, id, "w1:p1", Some("worker")).unwrap();
            let mut record = serde_json::json!({"request":["/repo","branch",null],"prepared":null});
            for (name, value) in evidence.as_object().unwrap() {
                record[name] = value.clone();
            }
            if id == "malformed" {
                assert!(
                    serde_json::from_value::<crate::prepare::PreparationReceipt>(record).is_err()
                );
                assert!(
                    crate::preparation_store::receipts::get(&ledger, id)
                        .unwrap()
                        .is_none()
                );
            } else {
                if id == "before-creation" {
                    record["validity"]["version"] = serde_json::json!(1);
                }
                crate::preparation_store::receipts::put(
                    &ledger,
                    id,
                    &serde_json::from_value(record).unwrap(),
                )
                .unwrap();
            }
        }
        let before = saved_files(&ledger.dir);
        let holds = local(&ledger, "worker", 100, 90).unwrap();
        for (id, code) in [
            ("missing", "completion_missing"),
            ("future", "completion_invalid"),
            ("before-creation", "completion_invalid"),
        ] {
            let hold = holds
                .iter()
                .find(|hold| {
                    hold.request_id.as_deref() == Some(id)
                        && hold.resource.starts_with("completion evidence")
                })
                .unwrap();
            assert_eq!(hold.code, code);
            assert_eq!(hold.command_argv, ["dogsled", "doctor", "--host", "worker"]);
        }
        assert!(
            !holds
                .iter()
                .any(|hold| hold.request_id.as_deref() == Some("plausible")
                    && hold.resource.starts_with("completion evidence"))
        );
        assert_eq!(before, saved_files(&ledger.dir));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn teardown_and_incomplete_preservation_inspect_the_pinned_snapshot() {
        for (stage, blocker, code) in [
            (
                "teardown_entered",
                Some("failed teardown: exit 1; hook will not be rerun automatically"),
                "teardown_failed",
            ),
            ("teardown_entered", None, "teardown_unknown"),
            ("normalizing", None, "preservation_pending"),
        ] {
            let ledger = fixture();
            let operation = serde_json::from_value(serde_json::json!({
                "id":"operation", "initial_id":"snapshot", "stage":stage, "blocker":blocker,
                "expectation":{"pane":"w2:p1","owners":[],"mode":{"kind":"owner"},"caller":{"pane":"w1:p1"},"tree":{"path":"/tmp/checkout","repo":"/tmp/repo","branch":"feature"}},
                "initial":{"id":"snapshot","ref_name":"refs/dogsled/recovery/snapshot","commit":"commit","head":{"kind":"detached","commit":"head"},"index_tree":"index","worktree_tree":"working","raw_worktree_bytes":true,"created_ms":1,"ignored_files_captured":false}
            })).unwrap();
            crate::host_store::put_operations(
                &ledger,
                &BTreeMap::from([("operation".into(), operation)]),
            )
            .unwrap();
            let before = saved_files(&ledger.dir);
            let holds = local(&ledger, "worker", 100, 90).unwrap();
            let hold = holds.iter().find(|hold| hold.code == code).unwrap();
            assert_eq!(
                hold.command_argv,
                [
                    "dogsled",
                    "recovery",
                    "show",
                    "snapshot",
                    "--repo",
                    "/tmp/repo"
                ]
            );
            assert_eq!(hold.command_host, "worker");
            assert!(hold.resource.contains("/tmp/checkout"));
            crate::cli::Cli::try_parse_from(&hold.command_argv).unwrap();
            assert_eq!(before, saved_files(&ledger.dir));
            std::fs::remove_dir_all(ledger.dir).unwrap();
        }
    }

    #[test]
    fn unsaved_work_and_child_event_failures_are_not_hidden_in_generic_holds() {
        let ledger = fixture();
        crate::items::record(
            &ledger,
            crate::items::Item {
                id: "child".into(),
                owner_pane: "w1:p1".into(),
                ..Default::default()
            },
        )
        .unwrap();
        crate::host_store::put_landing(
            &ledger,
            &serde_json::from_value(
                serde_json::json!({"child":"the teardown left unsaved work; checkout kept"}),
            )
            .unwrap(),
        )
        .unwrap();
        crate::store::write(&ledger, |tx| {
            crate::web_store::inbox::put_failure(
                tx,
                "reply-child-1",
                &crate::inbox::NotificationFailure {
                    reason: "child notification undelivered: subject expired or missing".into(),
                    owner: "w1:p1".into(),
                    owner_host: "worker".into(),
                },
            )?;
            crate::web_store::inbox::put_failure(
                tx,
                "reply-child-2",
                &crate::inbox::NotificationFailure {
                    reason: "child notification superseded: subject or custody changed".into(),
                    owner: "w1:p1".into(),
                    owner_host: "worker".into(),
                },
            )
        })
        .unwrap();
        let before = saved_files(&ledger.dir);
        let holds = local(&ledger, "worker", 100, 90).unwrap();
        for code in [
            "unsaved_work",
            "child_event_undelivered",
            "child_event_superseded",
        ] {
            let hold = holds.iter().find(|hold| hold.code == code).unwrap();
            assert_eq!(hold.command_argv, ["dogsled", "doctor", "--host", "worker"]);
        }
        assert_eq!(before, saved_files(&ledger.dir));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn origin_reference_holds_name_the_existing_commands_on_the_origin() {
        let ledger = fixture();
        let fixtures: Value = serde_json::from_str(include_str!(
            "../crates/todo-core/tests/fixtures/unit-exports.json"
        ))
        .unwrap();
        let mut exported: Value =
            serde_json::from_str(fixtures[0]["canonical"].as_str().unwrap()).unwrap();
        exported["digest"] = fixtures[0]["digest"].clone();
        let execution =
            serde_json::json!({"executor":"dogsled","ownerId":"w1:p1","requestId":"held"});
        let unit = serde_json::json!({"version":1,"execution":execution,"binding":{"contract":exported["contract"]["ref"],"execution":execution,"exportDigest":exported["digest"]},"exported":exported});
        let request = serde_json::json!({"id":"held","action":{"type":"agent","kind":"pi","name":"worker","message":"go"},"host":"worker","cwd":"/","owner":"w1:p1","owner_host":"origin","unit":unit});
        let attempt = crate::admissions::fixtures::released("held", "w1:p1", "origin");
        crate::admissions::put(&ledger, "held", &attempt).unwrap();
        crate::launch_store::put_progress_value(
            &ledger,
            "held",
            &serde_json::json!({
                "request":request,"origin_release":null,"pane":null,"terminal":null,
                "attempted":false,"confirmed":false,"result":{},"error":null
            }),
        )
        .unwrap();
        let before = saved_files(&ledger.dir);
        let holds = local(&ledger, "worker", 100, 90).unwrap();
        for (code, command) in [
            ("reference_release", "retry-reference-releases"),
            ("launch_reference", "settlements"),
        ] {
            let hold = holds.iter().find(|hold| hold.code == code).unwrap();
            assert_eq!(hold.command_argv, ["dogsled", "unit", command]);
            assert_eq!(hold.command_host, "origin");
            assert!(hold.reason.contains("awaiting origin reference release"));
            crate::cli::Cli::try_parse_from(&hold.command_argv).unwrap();
        }
        assert_eq!(before, saved_files(&ledger.dir));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn host_retargeting_and_argument_quoting_never_reinterpret_recorded_values() {
        let id = "request-{host}'; echo secret";
        let hold = report(
            "preparation_claim",
            "local",
            "resource".into(),
            "owner",
            id,
            "",
        )
        .on_host("peer host");
        assert_eq!(hold.command_argv[2], id);
        assert_eq!(hold.command_argv[4], "peer host");
        assert_eq!(hold.next_command, command_line(&hold.command_argv));
        assert_eq!(hold.command_host, "peer host");
    }
}
