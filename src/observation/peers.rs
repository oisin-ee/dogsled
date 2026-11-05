//! Observer-owned peer exchange. Only host-owned domains cross the host boundary;
//! fleet projections and their revisions never become peer inputs.

use super::{Availability, Domains, Gap, PeerSource, Published, VERSION};
use crate::drop::Checked;
use crate::error::{Error, Result};
use crate::hosts::{Cached, Fleet, Host};
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub(crate) fn retry_command(
    fleet: &Fleet<'_>,
    host: &str,
    json: bool,
) -> Result<std::process::ExitCode> {
    let client = super::ipc::Client::at(&fleet.ledger.dir);
    let report = if let Ok(subscription) = client.subscribe() {
        retry_published(&client, subscription, host)?
    } else {
        // host_peer uses fixed SQL columns, not serialized topology. This cold-only
        // observer control is the single CLI discovery exception outside serve.
        let observed = crate::hosts::ObserverFleet::discover(
            fleet.shell,
            fleet.ledger,
            fleet.policy,
            fleet.home.clone(),
        )?;
        fleet.retry_now(observed.find(host)?)?
    };
    crate::print(json, &report, || {
        format!(
            "{}: {}\n",
            report.host.label,
            if report.reachable {
                "reachable".into()
            } else {
                report.error.clone().unwrap_or_else(|| "unreachable".into())
            }
        )
    })?;
    Ok(std::process::ExitCode::SUCCESS)
}

pub(crate) fn retry_published(
    client: &super::ipc::Client,
    mut subscription: super::ipc::Subscription,
    host: &str,
) -> Result<crate::hosts::HostReport> {
    let reports: Vec<crate::hosts::HostReport> =
        super::read::view(&subscription.current().publication, "hosts")?;
    if let Some(local) = reports.into_iter().find(|report| {
        report.host.is_local() && (report.host.id == host || report.host.label == host)
    }) {
        return Ok(local);
    }
    let retry_token = client.retry(host)?;
    let deadline =
        std::time::Instant::now() + crate::budget::CALL_LIMIT + std::time::Duration::from_secs(10);
    loop {
        let connection = subscription.current();
        if let Some((id, _)) = connection
            .publication
            .sources
            .peers
            .iter()
            .find(|(_, source)| source.retry_tokens.contains(&retry_token))
        {
            let reports: Vec<crate::hosts::HostReport> =
                super::read::view(&connection.publication, "hosts")?;
            return reports
                .into_iter()
                .find(|report| report.host.id == *id)
                .ok_or_else(|| {
                    Error::Invalid("observation unavailable: peer host was not published".into())
                });
        }
        if matches!(connection.status, super::ipc::Status::Disconnected(_)) {
            return Err(Error::Invalid(
                "backoff reset; observation unavailable because serve stopped".into(),
            ));
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Err(Error::Invalid(
                "backoff reset; peer observation pending".into(),
            ));
        }
        let _ = subscription.changed(remaining);
    }
}

const CHECK_INTERVAL_MS: u64 = 3_600_000;
const CACHE_MS: u64 = 10_000;

pub(crate) fn exportable(local: &Domains) -> Domains {
    local
        .iter()
        .filter(|(key, _)| matches!(key.as_str(), "host" | "headroom" | "todos" | "branches"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub epoch: Option<String>,
    pub local_revision: Option<u64>,
    #[serde(default)]
    pub interests: Vec<Checked>,
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Export {
    pub version: u32,
    pub epoch: String,
    pub local_revision: u64,
    pub measured: BTreeMap<String, u64>,
    pub local: Domains,
    pub gaps: Vec<Gap>,
}

impl Export {
    pub fn of(value: &Published) -> Self {
        let local = exportable(&value.local);
        Self {
            version: value.version,
            epoch: value.epoch.clone(),
            local_revision: value.local_revision,
            measured: value
                .sources
                .local
                .iter()
                .filter(|(key, _)| local.contains_key(*key))
                .map(|(key, value)| (key.clone(), *value))
                .collect(),
            gaps: value
                .gaps
                .iter()
                .filter(|gap| {
                    !gap.domain.starts_with("peer:")
                        && matches!(
                            gap.domain.as_str(),
                            "host" | "headroom" | "todos" | "branches"
                        )
                })
                .cloned()
                .collect(),
            local,
        }
    }

    fn validate(&self) -> Result<()> {
        if self.version != VERSION {
            return Err(Error::Invalid("unsupported observation version".into()));
        }
        if self.epoch.is_empty()
            || self.local_revision == 0
            || exportable(&self.local) != self.local
        {
            return Err(Error::Invalid("invalid host observation export".into()));
        }
        Ok(())
    }
}

/// Service-lifetime, bounded interests. Registration is an explicit input mutation,
/// never branch observation in the SSH helper. Paths remain byte-for-byte strings.
#[derive(Debug, Clone, Default)]
pub(crate) struct Interests(Arc<Mutex<BTreeMap<String, Checked>>>);

impl Interests {
    pub fn register(&self, checks: &[Checked]) -> Result<bool> {
        let mut interests = self
            .0
            .lock()
            .map_err(|_| Error::Invalid("branch interests unavailable".into()))?;
        if checks.len() > 4096
            || interests.len()
                + checks
                    .iter()
                    .filter(|check| !interests.contains_key(&check.item))
                    .count()
                > 4096
        {
            return Err(Error::Invalid("too many branch interests".into()));
        }
        let mut changed = false;
        for check in checks {
            if check.item.is_empty() || check.branch.repo.is_empty() || check.branch.name.is_empty()
            {
                return Err(Error::Invalid("invalid branch interest".into()));
            }
        }
        for check in checks {
            if interests.get(&check.item) != Some(check) {
                interests.insert(check.item.clone(), check.clone());
                changed = true;
            }
        }
        Ok(changed)
    }

    pub fn current(&self) -> Result<Vec<Checked>> {
        Ok(self
            .0
            .lock()
            .map_err(|_| Error::Invalid("branch interests unavailable".into()))?
            .values()
            .cloned()
            .collect())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BranchObservation {
    pub checked: Checked,
    pub measured_ms: u64,
}

#[derive(Default)]
pub(crate) struct Branches {
    results: BTreeMap<String, BranchObservation>,
    attempted: BTreeMap<String, (crate::done::Branch, u64)>,
    failures: BTreeMap<String, String>,
    stale: bool,
}

fn cached_fresh(cached: &Cached, now: u64) -> bool {
    cached.available
        && cached.reachable
        && cached.running.unwrap_or(true)
        && cached
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.fresh(now))
}

pub(crate) fn interests(ledger: &Ledger, host: &str, now: u64) -> Result<Vec<Checked>> {
    let fresh = if host == crate::hosts::LOCAL {
        crate::host_store::snapshot(ledger)?.is_some_and(|snapshot| snapshot.fresh(now))
    } else {
        crate::host_store::peer(ledger, host)?
            .as_ref()
            .is_some_and(|cached| cached_fresh(cached, now))
    };
    if fresh {
        missing(ledger, host, now)
    } else {
        Ok(Vec::new())
    }
}

fn stale_branches() -> Gap {
    Gap {
        domain: "branches".into(),
        availability: Availability::Stale,
        detail: "branch observation requires fresh execution-host facts".into(),
    }
}

fn has_mark(item: &crate::items::Item, marks: &[crate::done::Mark]) -> bool {
    marks.iter().any(|mark| {
        mark.item
            .as_deref()
            .map_or(mark.pane == item.pane, |id| id == item.id)
    })
}

fn missing(ledger: &Ledger, host: &str, now: u64) -> Result<Vec<Checked>> {
    let marks = if host == crate::hosts::LOCAL {
        crate::done::list(ledger)?
    } else {
        crate::host_store::peer(ledger, host)?.map_or_else(Vec::new, |cached| cached.handoffs)
    };
    Ok(crate::items::list(ledger)?
        .into_iter()
        .filter(|item| {
            item.host == host
                && !has_mark(item, &marks)
                && item.kind == "agent"
                && item.closed_ms.is_some()
                && item.dropped_ms.is_none()
                && item
                    .branch_checked_ms
                    .is_none_or(|time| now.saturating_sub(time) >= CHECK_INTERVAL_MS)
        })
        .filter_map(|item| {
            item.branch
                .filter(|branch| !branch.landed)
                .map(|branch| Checked {
                    item: item.id,
                    branch,
                })
        })
        .collect())
}

impl Branches {
    /// Marks and missing-mark interests share one observation per repository/branch.
    /// Failed checks retain both the successful value and its measurement time.
    pub fn observe(
        &mut self,
        node: &crate::node::Node<'_>,
        interests: &[Checked],
        now: u64,
    ) -> Result<Vec<Gap>> {
        let mut requested = interests.to_vec();
        requested.extend(missing(node.ledger, crate::hosts::LOCAL, now)?);
        self.stale = (!requested.is_empty() || !self.results.is_empty())
            && !crate::host_store::snapshot(node.ledger)?
                .is_some_and(|snapshot| snapshot.fresh(now));
        if self.stale {
            requested.clear();
        }
        let marks = crate::done::list(node.ledger)?;
        requested.extend(
            marks
                .iter()
                .filter(|mark| mark.closed_ms.is_some() && mark.dropped_ms.is_none())
                .filter_map(|mark| mark.item.as_ref().zip(mark.branch.as_ref()))
                .filter(|(_, branch)| !branch.landed)
                .map(|(item, branch)| Checked {
                    item: item.clone(),
                    branch: branch.clone(),
                }),
        );
        let mut sampled = BTreeMap::new();
        for mut check in requested {
            if self
                .attempted
                .get(&check.item)
                .is_some_and(|(branch, time)| {
                    branch.repo == check.branch.repo
                        && branch.name == check.branch.name
                        && now.saturating_sub(*time) < CHECK_INTERVAL_MS
                })
            {
                continue;
            }
            self.attempted
                .insert(check.item.clone(), (check.branch.clone(), now));
            let key = (check.branch.repo.clone(), check.branch.name.clone());
            let result = sampled.entry(key).or_insert_with(|| {
                let mut branch = check.branch.clone();
                crate::done::refresh_branch(node.shell, &mut branch)
                    .map(|()| branch)
                    .map_err(|error| error.to_string())
            });
            match result {
                Ok(branch) => {
                    self.failures.remove(&check.item);
                    check.branch.clone_from(branch);
                    let observation = BranchObservation {
                        checked: check,
                        measured_ms: now,
                    };
                    import(
                        node.ledger,
                        crate::hosts::LOCAL,
                        std::slice::from_ref(&observation),
                    )?;
                    crate::done::observed_branch(node.ledger, &observation.checked)?;
                    self.results
                        .insert(observation.checked.item.clone(), observation);
                }
                Err(error) => {
                    self.failures.insert(check.item.clone(), error.clone());
                }
            }
        }
        Ok(self.gaps())
    }

    pub fn gaps(&self) -> Vec<Gap> {
        self.failures
            .values()
            .map(|detail| Gap {
                domain: "branches".into(),
                availability: Availability::Failed,
                detail: detail.clone(),
            })
            .chain(self.stale.then(stale_branches))
            .collect()
    }

    pub fn results(&self) -> Vec<BranchObservation> {
        self.results.values().cloned().collect()
    }
}

pub(crate) fn import(ledger: &Ledger, host: &str, results: &[BranchObservation]) -> Result<bool> {
    let mut changed = false;
    for result in results {
        changed |= crate::items::checked(
            ledger,
            host,
            std::slice::from_ref(&result.checked),
            result.measured_ms,
        )?;
    }
    Ok(changed)
}

#[derive(Default)]
pub(crate) struct Ingestion {
    export: Option<Export>,
    pub source: Option<PeerSource>,
    pub cached: Option<Cached>,
    local_changed: bool,
    forced: bool,
}

impl Ingestion {
    /// Imported items are local inputs, unlike ordinary peer cache replacements.
    pub fn take_local_changes(&mut self) -> u8 {
        if std::mem::take(&mut self.local_changed) {
            crate::service::wake::LEDGER
        } else {
            0
        }
    }

    pub fn reset(&mut self) {
        self.export = None;
        self.forced = true;
    }

    pub fn gaps(&self) -> Vec<Gap> {
        let mut gaps = self
            .export
            .as_ref()
            .map_or_else(Vec::new, |export| export.gaps.clone());
        if self
            .source
            .as_ref()
            .is_some_and(|source| source.availability == Availability::Stale)
        {
            gaps.push(stale_branches());
        }
        gaps
    }

    fn accept(
        &mut self,
        export: Export,
        now: u64,
    ) -> Result<(Option<crate::node::SnapshotReport>, Vec<BranchObservation>)> {
        if export.version != VERSION {
            self.failure(Availability::Unsupported, now);
            return Err(Error::Invalid("unsupported observation version".into()));
        }
        export.validate()?;
        if let Some(previous) = &self.export
            && previous.epoch == export.epoch
            && export.local_revision < previous.local_revision
        {
            return Err(Error::Invalid("out of order host observation".into()));
        }
        let branches = export
            .local
            .get("branches")
            .cloned()
            .map(serde_json::from_value)
            .transpose()?
            .unwrap_or_default();
        let report = export
            .local
            .get("host")
            .ok_or_else(|| Error::Invalid("host observation unavailable".into()))?;
        let report: crate::node::SnapshotReport = serde_json::from_value(report.clone())?;
        let availability = if export.gaps.iter().any(|gap| gap.domain == "host") {
            Availability::Failed
        } else if report
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.fresh(now))
        {
            Availability::Available
        } else {
            Availability::Stale
        };
        let changed = self.export.as_ref().is_none_or(|old| {
            old.epoch != export.epoch || old.local_revision != export.local_revision
        });
        let received_ms = if changed {
            now
        } else {
            self.source
                .as_ref()
                .map_or(now, |source| source.received_ms)
        };
        self.source = Some(PeerSource {
            epoch: export.epoch.clone(),
            local_revision: export.local_revision,
            received_ms,
            retry_tokens: Vec::new(),
            availability,
        });
        self.export = Some(export);
        Ok((Some(report), branches))
    }

    fn failure(&mut self, availability: Availability, now: u64) {
        let source = self.source.get_or_insert(PeerSource {
            epoch: String::new(),
            local_revision: 0,
            received_ms: now,
            retry_tokens: Vec::new(),
            availability: availability.clone(),
        });
        source.availability = availability;
        source.received_ms = now;
    }

    fn legacy(
        &mut self,
        fleet: &Fleet<'_>,
        host: &Host,
        now: u64,
    ) -> Result<(Option<crate::node::SnapshotReport>, Vec<BranchObservation>)> {
        let report =
            fleet.remote::<Option<crate::node::SnapshotReport>>(host, &["snapshot".into()])?;
        let snapshot = report.as_ref().and_then(|report| report.snapshot.as_ref());
        self.source = Some(PeerSource {
            epoch: "legacy".into(),
            local_revision: snapshot.map_or(0, |snapshot| snapshot.sampled_ms),
            received_ms: now,
            retry_tokens: Vec::new(),
            availability: if snapshot.is_some_and(|snapshot| snapshot.fresh(now)) {
                Availability::Available
            } else {
                Availability::Stale
            },
        });
        Ok((report, Vec::new()))
    }

    fn receive(
        &mut self,
        fleet: &Fleet<'_>,
        host: &Host,
        request: &Request,
        now: u64,
    ) -> Result<(Option<crate::node::SnapshotReport>, Vec<BranchObservation>)> {
        let reply = fleet.remote::<serde_json::Value>(
            host,
            &[
                "observation".into(),
                "--request".into(),
                serde_json::to_string(request)?,
            ],
        );
        match reply {
            Ok(value) => {
                if let Some(version) = value.get("version")
                    && version.as_u64() != Some(u64::from(VERSION))
                {
                    self.failure(Availability::Unsupported, now);
                    return Err(Error::Invalid("unsupported observation version".into()));
                }
                if value.get("version").is_none() {
                    return self.legacy(fleet, host, now);
                }
                self.accept(serde_json::from_value(value)?, now)
            }
            Err(error)
                if matches!(
                    crate::hosts::Failure::of(&error),
                    crate::hosts::Failure::RemoteCommandFailed
                        | crate::hosts::Failure::UnexpectedOutput
                ) || matches!(error.root(), Error::Command { .. }) =>
            {
                self.legacy(fleet, host, now)
            }
            Err(error) => Err(error),
        }
    }

    /// One bounded call per peer. Only serve calls this; readers never refresh a cache.
    pub fn refresh(
        &mut self,
        fleet: &Fleet<'_>,
        host: &Host,
        now: u64,
        checks: Vec<Checked>,
        wait: bool,
    ) -> Result<bool> {
        let source = fleet.remote_argv(host, &["snapshot".into()]);
        let previous = crate::host_store::peer(fleet.ledger, &host.id)?
            .filter(|cached| cached.source == source);
        let forced = std::mem::take(&mut self.forced);
        if !forced
            && let Some(cached) = &previous
            && (!cached.retry.ready(now)
                || (!wait && now.saturating_sub(cached.fetched_ms) < CACHE_MS))
        {
            self.cached.clone_from(&previous);
            return Ok(false);
        }
        let request = Request {
            epoch: self.export.as_ref().map(|value| value.epoch.clone()),
            local_revision: self.export.as_ref().map(|value| value.local_revision),
            interests: if previous
                .as_ref()
                .is_some_and(|cached| cached_fresh(cached, now))
            {
                checks
            } else {
                Vec::new()
            },
            wait,
        };
        let received = self.receive(fleet, host, &request, now);
        if received.is_err()
            && !self
                .source
                .as_ref()
                .is_some_and(|source| source.availability == Availability::Unsupported)
        {
            self.failure(Availability::Unavailable, now);
        }
        let mut cached = cache(
            previous.as_ref(),
            received
                .as_ref()
                .map(|(report, _)| report)
                .map_err(crate::hosts::Failure::of),
        );
        if self
            .source
            .as_ref()
            .is_some_and(|source| source.availability == Availability::Failed)
        {
            cached.available = false;
        }
        let mut retry = previous
            .as_ref()
            .map(|cached| cached.retry)
            .unwrap_or_default();
        if cached.reachable {
            retry.reset();
        } else {
            retry.fail(now, &host.id);
        }
        cached.fetched_ms = now;
        cached.source = source;
        cached.retry = retry;
        let changed = previous.as_ref().is_none_or(|previous| {
            let mut previous = previous.clone();
            previous.fetched_ms = cached.fetched_ms;
            serde_json::to_value(previous).ok() != serde_json::to_value(&cached).ok()
        });
        if changed {
            crate::host_store::put_peer(fleet.ledger, &host.id, &cached)?;
        }
        self.cached = Some(cached.clone());
        if cached_fresh(&cached, now)
            && let Ok((report, branches)) = received
        {
            let marks = report.map_or_else(Vec::new, |report| report.handoffs);
            let items = crate::items::list(fleet.ledger)?;
            for branch in branches.into_iter().filter(|branch| {
                !items
                    .iter()
                    .any(|item| item.id == branch.checked.item && has_mark(item, &marks))
            }) {
                self.local_changed |=
                    import(fleet.ledger, &host.id, std::slice::from_ref(&branch))?;
            }
        }
        Ok(changed)
    }
}

fn cache(
    previous: Option<&Cached>,
    report: std::result::Result<&Option<crate::node::SnapshotReport>, crate::hosts::Failure>,
) -> Cached {
    let previous_snapshot = || previous.and_then(|cached| cached.snapshot.clone());
    match report {
        Ok(Some(report)) => Cached {
            available: report.snapshot.is_some(),
            snapshot: report.snapshot.clone().or_else(previous_snapshot),
            reachable: true,
            running: report.running,
            handoffs: report.handoffs.clone(),
            holds: report.holds.clone(),
            service_lanes: report.service_lanes.clone(),
            job_validity: report.job_validity,
            ..Cached::default()
        },
        Ok(None) => Cached {
            snapshot: previous_snapshot(),
            job_validity: previous.and_then(|cached| cached.job_validity),
            reachable: true,
            error: Some("host snapshot is missing; service: down".into()),
            ..Cached::default()
        },
        Err(failure) => Cached {
            snapshot: previous_snapshot(),
            // Retain the last advertisement with its original snapshot/source identity.
            // It permits queueing only; unavailable facts cannot authorize placement.
            job_validity: previous.and_then(|cached| cached.job_validity),
            holds: previous.map_or_else(Vec::new, |cached| {
                cached
                    .holds
                    .clone()
                    .into_iter()
                    .map(crate::holds::Hold::unavailable)
                    .collect()
            }),
            failure: Some(failure),
            ..Cached::default()
        },
    }
}
