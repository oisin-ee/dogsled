//! Custody of orphaned children: who is responsible for an item now.
//!
//! The task origin is immutable. The item's owner pane, the unit execution, the final
//! reply and the unit result never change. A separate record holds the current
//! custodian, the revision of that record, and the history of the claims on it. A claim
//! names the revision it expects, so two leads cannot both take the same child.
//!
//! The custody stays on the host that recorded the item. A claim from another host is
//! refused, and a refused claim writes nothing.

use crate::error::Result;
use crate::items::Item;
use crate::ledger::Ledger;
use serde::{Deserialize, Serialize};

/// The custody records, beside the item records they describe.
const FILE: &str = "custody.json";
/// The lock that guards a claim on the host that recorded the item.
const LOCK: &str = "custody";

/// Who is responsible for one item.
///
/// The record starts at the origin: the owner pane and its session hold the custody
/// until a lead claims it. Only a claim moves the custody, and every claim is kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Custody {
    /// The item this custody is for.
    pub item: String,
    /// The host that recorded the item. The custody stays there.
    pub host: String,
    /// The owner pane at the origin. It never changes.
    pub owner_pane: String,
    /// The owner session at the origin. It never changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session: Option<String>,
    /// The pane that is responsible now.
    pub custodian_pane: String,
    /// The session of the responsible pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custodian_session: Option<String>,
    /// The revision of this record. A claim names the revision it expects.
    #[serde(default)]
    pub revision: u64,
    /// The claims, in the order they were made.
    #[serde(default)]
    pub claims: Vec<Claim>,
}

/// One claim of custody, and the custodian it replaced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Claim {
    /// The stable identity of this claim.
    pub id: String,
    /// The pane that claimed the custody.
    pub claimant_pane: String,
    /// The session of the claiming pane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claimant_session: Option<String>,
    /// When the claim was made.
    pub claimed_ms: u64,
    /// The custodian before this claim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_custodian: Option<String>,
    /// The revision this claim produced.
    pub revision: u64,
}

/// What the recording host proves about an item's owner pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneState {
    /// The pane list could not be read. It proves nothing.
    Unreadable,
    /// The pane list was read, and the owner pane is not in it.
    Gone,
    /// The pane list was read, and the owner pane is in it.
    Live,
}

/// What the recording host proves about the owner session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionState {
    /// The agent list could not be read. It proves nothing.
    Unreadable,
    /// No agent runs in the owner pane. The session ended.
    Ended,
    /// An agent runs in the owner pane, with this session identity.
    Running(String),
}

/// What the evidence proves about the owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// The owner still holds the child.
    Owned,
    /// The owner is gone: its pane is gone, or its session provably ended.
    Orphaned,
    /// The evidence cannot prove either.
    Unknown,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Owned => "owned",
            Self::Orphaned => "orphaned",
            Self::Unknown => "unknown",
        };
        f.write_str(text)
    }
}

/// A claim of the custody of one item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimRequest {
    /// The item to claim.
    pub item: String,
    /// The host the claim is made from.
    pub host: String,
    /// The claiming pane.
    pub pane: String,
    /// The session of the claiming pane.
    pub session: Option<String>,
    /// The custody revision the claimant expects.
    pub expected_revision: u64,
    /// When the claim is made.
    pub now: u64,
}

/// The result of a claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The custody moved to the claimant.
    Claimed { custody: Custody },
    /// The claimant already held the custody. Nothing changed.
    Held { custody: Custody },
}

/// Why a claim was refused. A refused claim writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Refusal {
    /// The custody record changed since the claimant read it.
    #[error("{item}: custody is at revision {actual}, not {expected}; the claim is refused")]
    Conflict {
        item: String,
        expected: u64,
        actual: u64,
    },
    /// The owner still holds the child.
    #[error("{item}: the owner is still there; the claim is refused")]
    NotOrphan { item: String },
    /// The evidence cannot prove the child is an orphan.
    #[error("{item}: the evidence cannot prove the child is an orphan; the claim is refused")]
    Unproven { item: String },
    /// The claim came from another host.
    #[error("{item}: the item is recorded on {host}; a claim from {from} is refused")]
    WrongHost {
        item: String,
        host: String,
        from: String,
    },
    /// No item with that identity is recorded here.
    #[error("{item}: no such item is recorded on this host; the claim is refused")]
    UnknownItem { item: String },
}

/// Why a claim did not succeed: the custody refused it, or the host could not read or
/// write its records.
#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    /// The custody refused the claim.
    #[error(transparent)]
    Refusal(#[from] Refusal),
    /// The host could not read or write the custody records.
    #[error(transparent)]
    Host(#[from] crate::error::Error),
}

impl From<ClaimError> for crate::error::Error {
    fn from(error: ClaimError) -> Self {
        match error {
            ClaimError::Refusal(refusal) => Self::Invalid(refusal.to_string()),
            ClaimError::Host(error) => error,
        }
    }
}

impl Custody {
    /// The custody an item starts with: the origin owner holds it.
    fn origin(item: &Item) -> Self {
        Self {
            item: item.id.clone(),
            host: crate::inbox::hostname(),
            owner_pane: item.owner_pane.clone(),
            owner_session: item.owner_session.clone(),
            custodian_pane: item.owner_pane.clone(),
            custodian_session: item.owner_session.clone(),
            revision: 0,
            claims: Vec::new(),
        }
    }

    /// The custody is held by this pane and session.
    ///
    /// A reused pane id with another session is not the custodian: the session identity
    /// decides, never the pane id alone.
    #[must_use]
    pub fn held_by(&self, pane: &str, session: Option<&str>) -> bool {
        self.custodian_pane == pane && self.custodian_session.as_deref() == session
    }

    /// The custodian the last claim replaced.
    #[must_use]
    #[cfg(test)]
    pub fn previous_custodian(&self) -> Option<&str> {
        self.claims.last()?.previous_custodian.as_deref()
    }
}

/// What the evidence proves about the owner of `item`.
///
/// The owner pane being gone proves an orphan. A surviving pane whose owner session has
/// ended proves an orphan too: the shell stayed, the lead exited. An unreadable list
/// proves nothing, and a legacy item records no owner session, so no session of it can
/// have ended. Such a child is unknown, not orphaned.
#[must_use]
pub fn classify(item: &Item, panes: &PaneState, session: &SessionState) -> Status {
    match panes {
        PaneState::Unreadable => Status::Unknown,
        PaneState::Gone => Status::Orphaned,
        PaneState::Live => {
            // A legacy item records no owner session. The session-ended rule needs one,
            // and it never guesses, so the child stays unknown.
            let Some(recorded) = item.owner_session.as_deref() else {
                return Status::Unknown;
            };
            match session {
                SessionState::Unreadable => Status::Unknown,
                SessionState::Ended => Status::Orphaned,
                SessionState::Running(live) => {
                    if live == recorded {
                        Status::Owned
                    } else {
                        // The pane id was reused by another session. The owner is gone.
                        Status::Orphaned
                    }
                }
            }
        }
    }
}

/// Read the recording host's evidence about the owner pane.
#[must_use]
pub fn evidence(shell: &dyn crate::shell::Shell, owner_pane: &str) -> (PaneState, SessionState) {
    let Ok(panes) = crate::cleanup::current_panes(shell) else {
        return (PaneState::Unreadable, SessionState::Unreadable);
    };
    if !panes.iter().any(|pane| pane.pane_id == owner_pane) {
        return (PaneState::Gone, SessionState::Ended);
    }
    let Ok(agents) = crate::probe::agent_list(shell) else {
        return (PaneState::Live, SessionState::Unreadable);
    };
    let agent = agents.iter().find(|agent| agent.pane == owner_pane);
    let session = match agent {
        None => SessionState::Ended,
        Some(agent) => match agent.agent_session.clone() {
            Some(session) => SessionState::Running(session),
            None => SessionState::Unreadable,
        },
    };
    (PaneState::Live, session)
}

/// Every custody record on this host.
pub fn list(ledger: &Ledger) -> Result<Vec<Custody>> {
    Ok(ledger.read(FILE)?.unwrap_or_default())
}

/// The custody of one item, when this host holds one.
pub fn get(ledger: &Ledger, item: &str) -> Result<Option<Custody>> {
    Ok(list(ledger)?
        .into_iter()
        .find(|custody| custody.item == item))
}

/// Record the custody of `item` at its origin, when this host holds none.
///
/// The record is written under the custody lock, so a concurrent claim sees it whole.
pub fn ensure(ledger: &Ledger, item: &Item) -> Result<Custody> {
    ledger.with_lock(LOCK, || {
        if let Some(existing) = get(ledger, &item.id)? {
            return Ok(existing);
        }
        let record = Custody::origin(item);
        let mut records = list(ledger)?;
        records.push(record.clone());
        ledger.write(FILE, &records)?;
        Ok(record)
    })
}

/// Claim the custody of an orphaned item.
///
/// The claim is atomic on `expected_revision`: it holds the custody lock, so two leads
/// cannot both win. A refused claim writes nothing.
#[allow(dead_code)]
pub fn claim(
    ledger: &Ledger,
    request: &ClaimRequest,
    shell: &dyn crate::shell::Shell,
) -> std::result::Result<ClaimOutcome, ClaimError> {
    // The evidence is read before the lock, so the lock never waits on a pane list.
    let owner_pane = owner_pane(ledger, &request.item)?;
    let (panes, session) = evidence(shell, &owner_pane);
    // The lock carries the host error. A refusal is kept apart, so the caller can tell
    // a refused claim from a host that could not read or write its records.
    let mut refused: Option<Refusal> = None;
    let result = ledger.with_lock(LOCK, || {
        match claim_locked(ledger, request, &panes, &session) {
            Ok(outcome) => Ok(outcome),
            Err(ClaimError::Refusal(refusal)) => {
                refused = Some(refusal);
                Err(crate::error::Error::Invalid("claim refused".into()))
            }
            Err(ClaimError::Host(error)) => Err(error),
        }
    });
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => Err(match refused {
            Some(refusal) => ClaimError::Refusal(refusal),
            None => ClaimError::Host(error),
        }),
    }
}

/// The pane that holds the origin of `item`.
fn owner_pane(ledger: &Ledger, item: &str) -> std::result::Result<String, ClaimError> {
    if let Some(record) = get(ledger, item)? {
        return Ok(record.owner_pane);
    }
    crate::items::list(ledger)?
        .into_iter()
        .find(|recorded| recorded.id == item)
        .map(|recorded| recorded.owner_pane)
        .ok_or_else(|| {
            Refusal::UnknownItem {
                item: item.to_owned(),
            }
            .into()
        })
}

/// Claim the custody under the custody lock.
fn claim_locked(
    ledger: &Ledger,
    request: &ClaimRequest,
    panes: &PaneState,
    session: &SessionState,
) -> std::result::Result<ClaimOutcome, ClaimError> {
    let items = crate::items::list(ledger)?;
    let Some(item) = items.iter().find(|i| i.id == request.item) else {
        return Err(Refusal::UnknownItem {
            item: request.item.clone(),
        }
        .into());
    };
    let mut records = list(ledger)?;
    let record = records
        .iter()
        .find(|custody| custody.item == request.item)
        .cloned()
        .unwrap_or_else(|| Custody::origin(item));
    // The custody stays on the host that recorded the item.
    if record.host != request.host {
        return Err(Refusal::WrongHost {
            item: request.item.clone(),
            host: record.host,
            from: request.host.clone(),
        }
        .into());
    }
    // A repeat claim by the current custodian changes nothing.
    if record.held_by(&request.pane, request.session.as_deref()) {
        return Ok(ClaimOutcome::Held { custody: record });
    }
    // The claim must name the revision it read.
    if record.revision != request.expected_revision {
        return Err(Refusal::Conflict {
            item: request.item.clone(),
            expected: request.expected_revision,
            actual: record.revision,
        }
        .into());
    }
    // Only a proven orphan can be claimed.
    match classify(item, panes, session) {
        Status::Orphaned => {}
        Status::Owned => {
            return Err(Refusal::NotOrphan {
                item: request.item.clone(),
            }
            .into());
        }
        Status::Unknown => {
            return Err(Refusal::Unproven {
                item: request.item.clone(),
            }
            .into());
        }
    }
    let claim = Claim {
        id: crate::ledger::new_id(),
        claimant_pane: request.pane.clone(),
        claimant_session: request.session.clone(),
        claimed_ms: request.now,
        previous_custodian: Some(record.custodian_pane.clone()),
        revision: record.revision + 1,
    };
    let mut custody = record;
    custody.custodian_pane.clone_from(&request.pane);
    custody.custodian_session.clone_from(&request.session);
    custody.revision += 1;
    custody.claims.push(claim);
    match records
        .iter_mut()
        .find(|existing| existing.item == custody.item)
    {
        Some(slot) => *slot = custody.clone(),
        None => records.push(custody.clone()),
    }
    ledger.write(FILE, &records)?;
    Ok(ClaimOutcome::Claimed { custody })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::Reply;
    use crate::shell::{Fake, Output};

    fn ledger() -> Ledger {
        Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
    }

    fn item(ledger: &Ledger, owner_session: Option<&str>) -> Item {
        let item = Item {
            id: "child".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: "w2:p1".into(),
            owner_pane: "w1:p1".into(),
            owner_session: owner_session.map(str::to_owned),
            name: Some("worker".into()),
            created_ms: 1,
            closed_ms: None,
            reply: Some(Reply {
                seq: 3,
                result_id: Some("result-1".into()),
                text: "the final reply".into(),
                captured_ms: 2,
                reported_ms: None,
                done: true,
            }),
            native_done: true,
            ..Item::default()
        };
        crate::items::record(ledger, item.clone()).unwrap();
        item
    }

    /// A pane list that holds only the child pane, so the owner pane is gone.
    fn shell_without_owner() -> Fake {
        let shell = Fake::new();
        shell.on(
            "herdr pane list",
            r#"{"result":{"panes":[{"pane_id":"w2:p1"}]}}"#,
        );
        shell.on("herdr agent list", r#"{"result":{"agents":[]}}"#);
        shell
    }

    /// A pane list that holds the owner pane, running `session`.
    fn shell_with_owner(session: &str) -> Fake {
        let shell = Fake::new();
        shell.on(
            "herdr pane list",
            r#"{"result":{"panes":[{"pane_id":"w1:p1"},{"pane_id":"w2:p1"}]}}"#,
        );
        shell.on(
            "herdr agent list",
            &format!(
                r#"{{"result":{{"agents":[{{"pane_id":"w1:p1","agent":"pi","agent_session":{{"kind":"path","value":"{session}"}}}}]}}}}"#
            ),
        );
        shell
    }

    /// A pane list that holds the owner pane, and no agent runs in it.
    fn shell_with_idle_owner() -> Fake {
        let shell = Fake::new();
        shell.on(
            "herdr pane list",
            r#"{"result":{"panes":[{"pane_id":"w1:p1"},{"pane_id":"w2:p1"}]}}"#,
        );
        shell.on("herdr agent list", r#"{"result":{"agents":[]}}"#);
        shell
    }

    /// A shell whose pane list cannot be read.
    fn shell_without_panes() -> Fake {
        let shell = Fake::new();
        shell.on_output(
            "herdr pane list",
            Output {
                status: 1,
                stdout: String::new(),
                stderr: "no panes".into(),
            },
        );
        shell
    }

    fn request(host: &str, revision: u64) -> ClaimRequest {
        ClaimRequest {
            item: "child".into(),
            host: host.into(),
            pane: "w3:p1".into(),
            session: Some("claimant-session".into()),
            expected_revision: revision,
            now: 1_000,
        }
    }

    fn host() -> String {
        crate::inbox::hostname()
    }

    /// The refusal of a claim that failed.
    fn refusal(error: &ClaimError) -> &Refusal {
        match error {
            ClaimError::Refusal(refusal) => refusal,
            ClaimError::Host(error) => panic!("the host failed: {error}"),
        }
    }

    #[test]
    fn the_custody_record_reloads_after_a_restart() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        ensure(&ledger, &item).unwrap();
        let shell = shell_without_owner();
        let ClaimOutcome::Claimed { custody } =
            claim(&ledger, &request(&host(), 0), &shell).unwrap()
        else {
            panic!("the claim was refused");
        };
        assert_eq!(custody.revision, 1);
        assert_eq!(custody.claims.len(), 1);
        // A new ledger over the same directory is a restart of the service.
        let restarted = Ledger::new(ledger.dir.clone());
        let reloaded = get(&restarted, "child")
            .unwrap()
            .expect("the custody is durable");
        assert_eq!(reloaded, custody);
        assert_eq!(reloaded.custodian_pane, "w3:p1");
        assert_eq!(
            reloaded.custodian_session.as_deref(),
            Some("claimant-session")
        );
        assert_eq!(reloaded.previous_custodian(), Some("w1:p1"));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn a_legacy_item_is_never_session_ended() {
        let ledger = ledger();
        let item = item(&ledger, None);
        // The pane lives and runs a session, but the item records none.
        assert_eq!(
            classify(
                &item,
                &PaneState::Live,
                &SessionState::Running("other-session".into())
            ),
            Status::Unknown
        );
        // The pane is gone: that still proves an orphan.
        assert_eq!(
            classify(&item, &PaneState::Gone, &SessionState::Ended),
            Status::Orphaned
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn an_owner_pane_without_an_agent_is_an_orphaned_session() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        // The pane survives, but the owner's agent session ended.
        let shell = shell_with_idle_owner();
        let (panes, session) = evidence(&shell, "w1:p1");
        assert_eq!(panes, PaneState::Live);
        assert_eq!(session, SessionState::Ended);
        assert_eq!(classify(&item, &panes, &session), Status::Orphaned);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn a_reused_pane_with_another_session_is_not_the_owner() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        ensure(&ledger, &item).unwrap();
        // The pane id lives, but another session runs there.
        let shell = shell_with_owner("other-session");
        let (panes, session) = evidence(&shell, "w1:p1");
        assert_eq!(panes, PaneState::Live);
        assert_eq!(classify(&item, &panes, &session), Status::Orphaned);
        let custody = get(&ledger, "child").unwrap().expect("the custody exists");
        assert!(custody.held_by("w1:p1", Some("owner-session")));
        assert!(!custody.held_by("w1:p1", Some("other-session")));
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn two_claims_at_the_same_revision_leave_one_winner() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        ensure(&ledger, &item).unwrap();
        let first = request(&host(), 0);
        let mut second = request(&host(), 0);
        second.pane = "w4:p1".into();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let left = barrier.clone();
        let dir = ledger.dir.clone();
        let other = dir.clone();
        let outcome = std::thread::scope(|scope| {
            let one = scope.spawn(move || {
                let ledger = Ledger::new(dir);
                let shell = shell_without_owner();
                left.wait();
                claim(&ledger, &first, &shell)
            });
            let two = scope.spawn(move || {
                let ledger = Ledger::new(other);
                let shell = shell_without_owner();
                barrier.wait();
                claim(&ledger, &second, &shell)
            });
            (one.join().unwrap(), two.join().unwrap())
        });
        let outcomes = [outcome.0, outcome.1];
        let winners = outcomes
            .iter()
            .filter(|result| matches!(result, Ok(ClaimOutcome::Claimed { .. })))
            .count();
        assert_eq!(winners, 1, "{outcomes:?}");
        let loser = outcomes
            .iter()
            .find_map(|result| result.as_ref().err())
            .expect("the loser is refused");
        assert!(
            matches!(refusal(loser), Refusal::Conflict { .. }),
            "{loser:?}"
        );
        let custody = get(&ledger, "child").unwrap().expect("the custody exists");
        assert_eq!(custody.revision, 1);
        assert_eq!(custody.claims.len(), 1);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn a_refused_claim_writes_nothing() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        ensure(&ledger, &item).unwrap();
        let before = std::fs::read(ledger.dir.join(FILE)).unwrap();
        // The owner is still there.
        let shell = shell_with_owner("owner-session");
        let error = claim(&ledger, &request(&host(), 0), &shell).unwrap_err();
        assert!(
            matches!(refusal(&error), Refusal::NotOrphan { .. }),
            "{error:?}"
        );
        // The claim came from another host.
        let shell = shell_without_owner();
        let error = claim(&ledger, &request("other-host", 0), &shell).unwrap_err();
        assert!(
            matches!(refusal(&error), Refusal::WrongHost { .. }),
            "{error:?}"
        );
        assert_eq!(std::fs::read(ledger.dir.join(FILE)).unwrap(), before);
        let custody = get(&ledger, "child").unwrap().expect("the custody exists");
        assert_eq!(custody.revision, 0);
        assert_eq!(custody.claims, [] as [Claim; 0]);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn a_repeat_claim_by_the_custodian_changes_nothing() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        ensure(&ledger, &item).unwrap();
        let shell = shell_without_owner();
        let ClaimOutcome::Claimed { custody } =
            claim(&ledger, &request(&host(), 0), &shell).unwrap()
        else {
            panic!("the claim was refused");
        };
        let ClaimOutcome::Held { custody: held } =
            claim(&ledger, &request(&host(), 0), &shell).unwrap()
        else {
            panic!("the repeat claim moved the custody");
        };
        assert_eq!(held, custody);
        assert_eq!(held.revision, 1);
        assert_eq!(held.claims.len(), 1);
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn the_origin_the_reply_and_the_result_stay_identical_after_a_claim() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        ensure(&ledger, &item).unwrap();
        // The unit result evidence the owner recorded for the child.
        let admissions = ledger.child("admissions");
        let unit = serde_json::json!({
            "request": {"id": "child"},
            "launch": null,
            "error": null,
            "unit": {
                "execution": {"request_id": "r-1", "owner_id": "o-1"},
                "result_id": "result-1"
            }
        });
        admissions.write("child.json", &unit).unwrap();
        let items_before = std::fs::read(ledger.dir.join("items.json")).unwrap();
        let unit_before = std::fs::read(admissions.dir.join("child.json")).unwrap();
        let shell = shell_without_owner();
        claim(&ledger, &request(&host(), 0), &shell).unwrap();
        assert_eq!(
            std::fs::read(ledger.dir.join("items.json")).unwrap(),
            items_before
        );
        assert_eq!(
            std::fs::read(admissions.dir.join("child.json")).unwrap(),
            unit_before
        );
        let recorded = crate::items::list(&ledger).unwrap();
        assert_eq!(recorded[0].owner_pane, "w1:p1");
        assert_eq!(recorded[0].reply.as_ref().unwrap().text, "the final reply");
        assert_eq!(
            recorded[0].reply.as_ref().unwrap().result_id.as_deref(),
            Some("result-1")
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }

    #[test]
    fn an_unreadable_pane_list_proves_nothing() {
        let ledger = ledger();
        let item = item(&ledger, Some("owner-session"));
        assert_eq!(
            classify(&item, &PaneState::Unreadable, &SessionState::Unreadable),
            Status::Unknown
        );
        let shell = shell_without_panes();
        let error = claim(&ledger, &request(&host(), 0), &shell).unwrap_err();
        assert!(
            matches!(refusal(&error), Refusal::Unproven { .. }),
            "{error:?}"
        );
        std::fs::remove_dir_all(ledger.dir).unwrap();
    }
}
