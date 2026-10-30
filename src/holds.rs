//! Read-only resource ownership reports. Release decisions stay with each store.

use crate::error::Result;
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

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
        code: "legacy_preparation",
        evidence: &[],
        reason: "requestless preparation claim; request-scoped retirement is unsupported",
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
    let store = ledger.child(directory);
    let entries = match std::fs::read_dir(&store.dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut records = Vec::new();
    for entry in entries {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if std::path::Path::new(&name)
            .extension()
            .is_none_or(|ext| ext != "json")
        {
            continue;
        }
        match store.read(&name) {
            Ok(Some(value)) => records.push((name, Some(value))),
            Ok(None) => (), // Concurrent retention removed the record.
            Err(_) => records.push((name, None)),
        }
    }
    records.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(records)
}

fn text<'a>(value: &'a Value, path: &str) -> &'a str {
    value
        .pointer(path)
        .and_then(Value::as_str)
        .unwrap_or("unrecorded")
}

/// The host reports existing claims; it never decides whether to release them.
pub fn local(ledger: &Ledger, host: &str, now: u64, ttl_ms: u64) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    let admissions = records(ledger, "admissions")?;
    let launches = records(ledger, "launches")?;
    let grants = records(ledger, "placement-grants")?;
    let tickets = crate::work::queue(ledger)?;
    let jobs = crate::jobs::list(ledger)?;
    let mut owners = BTreeMap::new();
    for (name, value) in admissions.iter().chain(&launches).chain(&grants) {
        let Some(value) = value else {
            continue;
        };
        let request = value
            .get("request")
            .or_else(|| value.pointer("/envelope/request"));
        if let Some(request) = request {
            let id = name.trim_end_matches(".json").trim_end_matches(".progress");
            owners.insert(
                id.to_owned(),
                (
                    text(request, "/owner").to_owned(),
                    text(request, "/owner_host").to_owned(),
                ),
            );
        }
    }
    for ticket in &tickets {
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
    for job in &jobs {
        owners
            .entry(job.id.clone())
            .or_insert_with(|| (job.owner.clone(), host.into()));
    }
    let owned = |code: &str, resource: String, id: &str| {
        let (owner, origin) = owners.get(id).map_or((id, host), |(owner, origin)| {
            (owner.as_str(), origin.as_str())
        });
        let mut hold = report(code, host, resource, owner, id, "");
        if owners.contains_key(id) {
            hold.owner_pane = Some(owner.into());
        }
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
    };
    holds.extend(preparations(ledger, &owned)?);
    holds.extend(capacity(ledger, host, now, ttl_ms, &tickets, &owned)?);
    for (directory, code, saved) in [
        ("placement-grants", "placement_grant", grants),
        ("job-validity", "validity", records(ledger, "job-validity")?),
    ] {
        for (name, value) in saved {
            let id = name.trim_end_matches(".json");
            holds.push(owned(
                if value.is_some() { code } else { "unreadable" },
                format!("{directory}/{id}"),
                id,
            ));
        }
    }
    for ticket in &tickets {
        if let Some(reason) = &ticket.error {
            holds.push(
                owned(
                    "admission",
                    format!("ticket {}", ticket.id),
                    &ticket.request.id,
                )
                .with_reason(reason),
            );
        }
    }
    holds.extend(completion_holds(ledger, now, &owned)?);
    holds.extend(preservation_holds(ledger, host)?);
    holds.extend(notification_holds(ledger, &owned)?);
    holds.extend(self::admissions(admissions, &tickets, &owned));
    holds.extend(releases(ledger, &launches, &owned)?);
    holds.extend(checkouts(ledger, host, &launches)?);
    holds.extend(branches(ledger, host, &owned)?);
    holds.extend(jobs_and_closures(ledger, host, jobs, &owned)?);
    Ok(holds)
}

fn notification_holds(
    ledger: &Ledger,
    owned: &impl Fn(&str, String, &str) -> Hold,
) -> Result<Vec<Hold>> {
    let failures: BTreeMap<String, String> = ledger
        .read("notification-failures.json")?
        .unwrap_or_default();
    Ok(failures
        .into_iter()
        .map(|(id, reason)| {
            owned("child_event_undelivered", format!("child event {id}"), &id).with_reason(&reason)
        })
        .collect())
}

fn completion_holds(
    ledger: &Ledger,
    now: u64,
    owned: &impl Fn(&str, String, &str) -> Hold,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    // Do not call the owners' retention predicates: those can initialize the clock.
    let clock = ledger.read::<Value>("job-validity-clock.json");
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
        for (name, value) in records(ledger, directory)? {
            if (directory == "launches" && !name.ends_with(".progress.json"))
                || (directory == "placement-grants" && !name.ends_with(".lifecycle.json"))
            {
                continue;
            }
            let Some(value) = value else { continue };
            let id = name
                .trim_end_matches(".json")
                .trim_end_matches(".progress")
                .trim_end_matches(".disposition")
                .trim_end_matches(".lifecycle");
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
            ));
        }
    }
    Ok(holds)
}

fn preservation_holds(ledger: &Ledger, host: &str) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for (name, value) in records(ledger, "preservation-operations")? {
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

fn preparations(ledger: &Ledger, owned: &impl Fn(&str, String, &str) -> Hold) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    let preparations: Vec<Value> = ledger.read("preparations.json")?.unwrap_or_default();
    for claim in preparations {
        let id = claim["request_id"].as_str();
        let disposition = id
            .map(|id| {
                crate::jobs::validate_id(id)?;
                ledger
                    .child("preparation-receipts")
                    .read::<crate::prepare::disposition::Receipt>(&format!("{id}.disposition.json"))
            })
            .transpose();
        let code = match (&id, &disposition) {
            (None, _) => "legacy_preparation",
            (_, Err(_)) => "unreadable",
            (_, Ok(Some(Some(_)))) => "preparation_fenced",
            _ => "preparation_claim",
        };
        let mut hold = owned(
            code,
            format!(
                "preparation claim {} for {}",
                id.unwrap_or("requestless"),
                text(&claim, "/path")
            ),
            id.unwrap_or("unrecorded"),
        );
        if let Ok(Some(Some(disposition))) = disposition {
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
    owned: &impl Fn(&str, String, &str) -> Hold,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for reservation in ledger.reservations(now, ttl_ms)? {
        let mut hold = owned(
            "capacity",
            format!("admission capacity {}", reservation.id),
            &reservation.id,
        );
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
    owned: &impl Fn(&str, String, &str) -> Hold,
) -> Vec<Hold> {
    let mut holds = Vec::new();
    for (name, value) in admissions {
        let id = name.trim_end_matches(".json");
        let Some(value) = value else {
            holds.push(owned("unreadable", format!("admission {id}"), id));
            continue;
        };
        if let Some(reason) = value["error"].as_str() {
            holds.push(
                owned("admission", format!("admission refusal {id}"), id).with_reason(reason),
            );
        }
        if let Some(phase) = value.pointer("/placement/phase").and_then(Value::as_str) {
            if phase != "completed" {
                let mut hold = owned("placement", format!("placement {id}"), id);
                hold.reason = format!("{} ({phase})", hold.reason);
                holds.push(hold);
            }
        } else if value.get("result").is_none_or(Value::is_null) {
            let mut hold = owned("admission", format!("admission {id}"), id);
            if let Some(ticket) = tickets
                .iter()
                .find(|t| t.request.id == id && matches!(t.state.as_str(), "pending" | "admitting"))
            {
                hold = owned("queued_admission", format!("admission {id}"), id);
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
            ));
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
            ));
        }
    }
    holds
}

fn releases(
    ledger: &Ledger,
    launches: &[(String, Option<Value>)],
    owned: &impl Fn(&str, String, &str) -> Hold,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for (name, value) in records(ledger, "cancellations")? {
        let id = name.trim_end_matches(".json");
        match value {
            Some(value) if value["release"] == "pending" && value["held"].is_object() => {
                holds.push(owned("cancellation_held", format!("cancellation {id}"), id));
            }
            Some(value) if value["release"] == "pending" => {
                let mut hold = owned("cancellation", format!("cancellation {id}"), id);
                hold.command_argv = vec![
                    "dogsled".into(),
                    "cancel".into(),
                    text(&value, "/ticket/id").into(),
                ];
                hold.next_command = command_line(&hold.command_argv);
                holds.push(hold);
            }
            None => holds.push(owned("unreadable", format!("cancellation {id}"), id)),
            _ => (),
        }
    }
    for (name, value) in launches {
        if name.ends_with(".release.json") {
            holds.push(owned(
                "release_certificate",
                format!(
                    "ordinary launch references {}",
                    name.trim_end_matches(".release.json")
                ),
                name.trim_end_matches(".release.json"),
            ));
        }
        if !name.ends_with(".progress.json") {
            continue;
        }
        let id = name.trim_end_matches(".progress.json");
        if let Some(reason) = value.as_ref().and_then(|v| v["error"].as_str()) {
            holds.push(owned("admission", format!("launch refusal {id}"), id).with_reason(reason));
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
                ));
            }
            None => holds.push(owned("unreadable", format!("launch {id}"), id)),
            Some(value) if value.get("request").is_some() => holds.push(owned(
                "ordinary_reference_release",
                format!("ordinary launch references {id}"),
                id,
            )),
            _ => (),
        }
    }
    Ok(holds)
}

fn checkouts(
    ledger: &Ledger,
    host: &str,
    launches: &[(String, Option<Value>)],
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    let trees: Vec<crate::cleanup::Tree> = ledger.read("worktrees.json")?.unwrap_or_default();
    let requests: Vec<crate::work::Launch> = launches
        .iter()
        .filter_map(|(_, value)| {
            serde_json::from_value(value.as_ref()?.get("request")?.clone()).ok()
        })
        .collect();
    for tree in trees {
        let request = requests.iter().find(|request| request.cwd == tree.path);
        let mut hold = report(
            "registered_checkout",
            host,
            format!("checkout {}", tree.path),
            request.map_or("unrecorded", |request| request.owner.as_str()),
            "",
            "",
        );
        hold.owner_pane = request.map(|request| request.owner.clone());
        if let Some(pane) = request.and_then(|request| request.pane.as_deref()) {
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
    owned: &impl Fn(&str, String, &str) -> Hold,
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
            );
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
    jobs: Vec<crate::jobs::Job>,
    owned: &impl Fn(&str, String, &str) -> Hold,
) -> Result<Vec<Hold>> {
    let mut holds = Vec::new();
    for job in jobs {
        if !crate::jobs::terminal(&job) {
            let mut hold = report(
                "job_checkout",
                host,
                format!("checkout {}", job.cwd),
                &job.owner,
                &job.id,
                "",
            );
            hold.owner_pane = Some(job.owner);
            hold.reason = format!("{} ({})", hold.reason, job.state);
            holds.push(hold);
        }
    }
    let landing: BTreeMap<String, String> = ledger.read("landing.json")?.unwrap_or_default();
    for (id, reason) in landing {
        let mut hold = owned("preservation", format!("closure {id}"), &id).with_reason(&reason);
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

    fn saved_files(path: &std::path::Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
        let mut files = BTreeMap::new();
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(saved_files(&path));
            } else {
                files.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
        files
    }

    fn assert_reported_reason(reason: &str, code: &str) {
        let ledger = fixture();
        ledger
            .child("admissions")
            .write(
                "held.json",
                &serde_json::json!({
                    "request":{"owner":"w1:p1", "owner_host":"origin"}, "error":reason
                }),
            )
            .unwrap();
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
        ledger
            .write(
                "job-validity-clock.json",
                &serde_json::json!({"version":1,"high_water_ms":100}),
            )
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
            ledger
                .child("preparation-receipts")
                .write(&format!("{id}.json"), &evidence)
                .unwrap();
        }
        let before = saved_files(&ledger.dir);
        let holds = local(&ledger, "worker", 100, 90).unwrap();
        for (id, code) in [
            ("missing", "completion_missing"),
            ("future", "completion_invalid"),
            ("malformed", "completion_invalid"),
            ("before-creation", "completion_invalid"),
        ] {
            let hold = holds
                .iter()
                .find(|hold| hold.request_id.as_deref() == Some(id))
                .unwrap();
            assert_eq!(hold.code, code);
            assert_eq!(hold.command_argv, ["dogsled", "doctor", "--host", "worker"]);
        }
        assert!(
            !holds
                .iter()
                .any(|hold| hold.request_id.as_deref() == Some("plausible"))
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
            ledger.child("preservation-operations").write("operation.json", &serde_json::json!({
                "id":"operation", "stage":stage, "blocker":blocker,
                "expectation":{"pane":"w2:p1","caller":{"pane":"w1:p1"},"tree":{"path":"/tmp/checkout","repo":"/tmp/repo"}},
                "initial":{"id":"snapshot"}
            })).unwrap();
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
        ledger
            .write(
                "landing.json",
                &serde_json::json!({"child":"the teardown left unsaved work; checkout kept"}),
            )
            .unwrap();
        ledger
            .write(
                "notification-failures.json",
                &serde_json::json!({
                    "reply-child-1":"child notification undelivered: subject expired or missing",
                    "reply-child-2":"child notification superseded: subject or custody changed"
                }),
            )
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
        let request = serde_json::json!({"owner":"w1:p1","owner_host":"origin","unit":{}});
        ledger
            .child("admissions")
            .write(
                "held.json",
                &serde_json::json!({
                    "request":request,"unit":{"reference_release":{"acknowledgement":null}}
                }),
            )
            .unwrap();
        ledger
            .child("launches")
            .write(
                "held.progress.json",
                &serde_json::json!({
                    "request":request,"origin_release":null
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
