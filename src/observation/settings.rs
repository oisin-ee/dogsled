//! Serve-owned, session-qualified live Pi settings. HTTP and peers only forward commands.

use super::{Published, details::Session};
use portal_types::agents::ProducerEvidence;
use portal_types::models::{AgentSettings, ModelRef, SettingsChange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub(crate) const MAX_BODY: usize = 2 * 1024 * 1024;
pub(crate) const WAIT: Duration = Duration::from_secs(10);
const MAX_PENDING: usize = 64;

#[cfg(test)]
#[path = "tests/settings.rs"]
pub(crate) mod tests;
type Outcome<T> = std::result::Result<T, Failure>;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Failure {
    pub code: String,
    pub error: String,
}

impl Failure {
    pub fn new(code: &str, error: &str) -> Self {
        Self {
            code: code.into(),
            error: error.into(),
        }
    }
    pub fn unavailable() -> Self {
        Self::new("settings_unavailable", "Pi settings are unavailable")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Publication {
    pub evidence: ProducerEvidence,
    pub settings: AgentSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Applied {
    pub evidence: ProducerEvidence,
    pub settings: Option<AgentSettings>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    pub id: String,
    pub evidence: ProducerEvidence,
    pub change: SettingsChange,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Command {
    Current,
    Publish(Publication),
    Change(SettingsChange),
    Subscribe(ProducerEvidence),
    Result { id: String, result: Applied },
}

pub(crate) fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
fn valid_model(model: &ModelRef) -> bool {
    bounded(&model.provider, 256) && bounded(&model.id, 256)
}

pub(crate) fn validate_change(change: &SettingsChange) -> Outcome<()> {
    if (change.model.is_none() && change.thinking.is_none())
        || change
            .model
            .as_ref()
            .is_some_and(|model| !valid_model(model))
    {
        return Err(Failure::new(
            "invalid_settings",
            "invalid or empty settings change",
        ));
    }
    Ok(())
}
fn validate_settings(settings: &AgentSettings) -> Outcome<()> {
    if settings.models.len() > 1000
        || settings
            .model
            .as_ref()
            .is_some_and(|model| !valid_model(model))
        || settings.models.iter().any(|choice| {
            !valid_model(&choice.model)
                || choice
                    .name
                    .as_ref()
                    .is_some_and(|name| !bounded(name, 1024))
                || choice.thinking.is_empty()
                || choice.thinking.len() > 7
        })
    {
        return Err(Failure::new(
            "invalid_settings",
            "invalid settings catalogue",
        ));
    }
    let mut models = std::collections::HashSet::new();
    if settings
        .models
        .iter()
        .any(|choice| !models.insert(&choice.model))
    {
        return Err(Failure::new("invalid_settings", "duplicate model choice"));
    }
    Ok(())
}

#[derive(Debug)]
struct Pending {
    pane: String,
    reply: oneshot::Sender<Outcome<AgentSettings>>,
}
#[derive(Debug)]
struct Entry {
    session: Session,
    publication: Publication,
    requests: Option<mpsc::Sender<Request>>,
}
#[derive(Debug, Default)]
struct State {
    bindings: BTreeMap<String, Session>,
    entries: BTreeMap<String, Entry>,
    pending: BTreeMap<String, Pending>,
}

/// Shared only inside serve's IPC runtime. No web-process settings cache exists.
#[derive(Debug, Clone, Default)]
pub(crate) struct Authority(Arc<Mutex<State>>);

impl Authority {
    fn state(&self) -> Outcome<std::sync::MutexGuard<'_, State>> {
        self.0
            .lock()
            .map_err(|_| Failure::new("settings_unavailable", "settings authority stopped"))
    }

    /// Idle hosts do not read publications or allocate terminal bindings.
    pub fn observe<'a>(&self, publication: impl FnOnce() -> Option<&'a Published>) {
        let Ok(state) = self.state() else {
            return;
        };
        if state.entries.is_empty() {
            return;
        }
        drop(state);
        self.refresh_bindings(publication());
    }

    /// An explicit Pi publication must establish its native binding, including the first one.
    /// Only a fresh terminal measurement reconciles. A stale or missing one proves nothing about
    /// a session, so it keeps the last bindings: a loaded host measures terminals more than
    /// `NATIVE_TTL_MS` apart, and dropping entries then made live sessions unavailable.
    pub fn refresh_bindings(&self, publication: Option<&Published>) {
        let Some(bindings) = publication
            .filter(|publication| {
                !publication.gaps.iter().any(|gap| gap.domain == "terminals")
                    && publication
                        .sources
                        .local
                        .get("terminals")
                        .is_some_and(|measured| {
                            crate::ledger::now_ms().saturating_sub(*measured)
                                < super::live_activity::NATIVE_TTL_MS
                        })
            })
            .and_then(|publication| publication.local.get("terminals"))
            .and_then(|value| serde_json::from_value::<crate::probe::Terminals>(value.clone()).ok())
            .map(|terminals| {
                terminals
                    .agents
                    .into_iter()
                    .filter_map(|agent| {
                        let session = Session::of(&agent)?;
                        (session.harness.as_deref() == Some("pi")).then_some((agent.pane, session))
                    })
                    .collect()
            })
        else {
            return;
        };
        if let Ok(mut state) = self.state() {
            state.reconcile(bindings);
        }
    }

    pub async fn execute(&self, pane: String, command: Command) -> Outcome<AgentSettings> {
        if !bounded(&pane, 128) {
            return Err(Failure::new("invalid_settings", "invalid pane"));
        }
        match command {
            Command::Publish(publication) => self.publish(pane, publication).await,
            Command::Current => Ok(self.state()?.entry(&pane)?.publication.settings.clone()),
            Command::Change(change) => self.change(pane, change).await,
            Command::Result { id, result } => self.applied(&pane, &id, result),
            Command::Subscribe(_) => Err(Failure::new(
                "invalid_settings",
                "subscription requires a stream",
            )),
        }
    }

    async fn publish(&self, pane: String, publication: Publication) -> Outcome<AgentSettings> {
        validate_settings(&publication.settings)?;
        let session = self
            .state()?
            .bindings
            .get(&pane)
            .cloned()
            .ok_or_else(Failure::unavailable)?;
        let evidence = publication.evidence.clone();
        let path = session.transcript.clone();
        let verified = tokio::task::spawn_blocking(move || {
            path.as_deref() == Some(evidence.session_file.as_str())
                && bounded(&evidence.session_id, 1024)
                && path
                    .as_deref()
                    .and_then(|path| crate::transcript::pi_session_id(path).ok())
                    .as_deref()
                    == Some(evidence.session_id.as_str())
        })
        .await
        .unwrap_or(false);
        if !verified {
            return Err(Failure::new(
                "session_changed",
                "native Pi session evidence changed",
            ));
        }
        let mut state = self.state()?;
        if state.bindings.get(&pane) != Some(&session) {
            return Err(Failure::new("session_changed", "Pi session changed"));
        }
        let settings = publication.settings.clone();
        if state
            .entries
            .get(&pane)
            .is_some_and(|entry| entry.publication.evidence != publication.evidence)
        {
            state.remove(&pane);
        }
        if let Some(entry) = state.entries.get_mut(&pane) {
            entry.publication = publication;
        } else {
            state.entries.insert(
                pane,
                Entry {
                    session,
                    publication,
                    requests: None,
                },
            );
        }
        Ok(settings)
    }

    async fn change(&self, pane: String, change: SettingsChange) -> Outcome<AgentSettings> {
        validate_change(&change)?;
        let (id, response) = {
            let mut state = self.state()?;
            state
                .pending
                .retain(|_, pending| !pending.reply.is_closed());
            if state.pending.len() >= MAX_PENDING {
                return Err(Failure::new("settings_busy", "too many settings requests"));
            }
            let entry = state.entry(&pane)?;
            if let Some(model) = &change.model
                && !entry
                    .publication
                    .settings
                    .models
                    .iter()
                    .any(|choice| &choice.model == model)
            {
                return Err(Failure::new("settings_refused", "unknown model"));
            }
            let sender = entry.requests.as_ref().ok_or_else(Failure::unavailable)?;
            let id = crate::ledger::new_id();
            sender
                .try_send(Request {
                    id: id.clone(),
                    evidence: entry.publication.evidence.clone(),
                    change,
                })
                .map_err(|_| Failure::unavailable())?;
            let (reply, response) = oneshot::channel();
            state.pending.insert(
                id.clone(),
                Pending {
                    pane: pane.clone(),
                    reply,
                },
            );
            (id, response)
        };
        let result = tokio::time::timeout(WAIT, response).await;
        self.state()?.pending.remove(&id);
        match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) => Err(Failure::new("session_changed", "Pi session changed")),
            Err(_) => Err(Failure::new(
                "settings_timeout",
                "Pi did not answer in 10 seconds",
            )),
        }
    }

    fn applied(&self, pane: &str, id: &str, result: Applied) -> Outcome<AgentSettings> {
        let mut state = self.state()?;
        state.evidence(pane, &result.evidence)?;
        if result.settings.is_some() == result.error.is_some()
            || result
                .error
                .as_ref()
                .is_some_and(|error| !bounded(error, 4096))
        {
            return Err(Failure::new("invalid_settings", "invalid settings result"));
        }
        if let Some(settings) = &result.settings {
            validate_settings(settings)?;
        }
        if state
            .pending
            .get(id)
            .is_none_or(|pending| pending.pane != pane)
        {
            return Err(Failure::new(
                "session_changed",
                "settings request is no longer pending",
            ));
        }
        let outcome = match result.settings {
            Some(settings) => {
                if let Some(entry) = state.entries.get_mut(pane) {
                    entry.publication.settings = settings.clone();
                }
                Ok(settings)
            }
            None => Err(Failure::new(
                "settings_refused",
                result.error.as_deref().unwrap_or("Pi refused the change"),
            )),
        };
        if let Some(pending) = state.pending.remove(id) {
            let _ = pending.reply.send(outcome);
        }
        Ok(state.entry(pane)?.publication.settings.clone())
    }

    pub fn subscribe(
        &self,
        pane: &str,
        evidence: &ProducerEvidence,
    ) -> Outcome<mpsc::Receiver<Request>> {
        let mut state = self.state()?;
        state.evidence(pane, evidence)?;
        let (sender, receiver) = mpsc::channel(MAX_PENDING);
        let entry = state
            .entries
            .get_mut(pane)
            .ok_or_else(Failure::unavailable)?;
        entry.requests = Some(sender);
        Ok(receiver)
    }
}

impl State {
    fn entry(&self, pane: &str) -> Outcome<&Entry> {
        self.entries.get(pane).ok_or_else(Failure::unavailable)
    }
    fn evidence(&self, pane: &str, evidence: &ProducerEvidence) -> Outcome<()> {
        if &self.entry(pane)?.publication.evidence != evidence {
            return Err(Failure::new("session_changed", "Pi session changed"));
        }
        Ok(())
    }
    fn remove(&mut self, pane: &str) {
        self.entries.remove(pane);
        let ids: Vec<_> = self
            .pending
            .iter()
            .filter(|(_, pending)| pending.pane == pane)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            if let Some(pending) = self.pending.remove(&id) {
                let _ = pending
                    .reply
                    .send(Err(Failure::new("session_changed", "Pi session changed")));
            }
        }
    }
    fn reconcile(&mut self, bindings: BTreeMap<String, Session>) {
        let removed: Vec<_> = self
            .entries
            .iter()
            .filter(|(pane, entry)| {
                bindings.get(*pane) != Some(&entry.session)
                    || entry
                        .session
                        .transcript
                        .as_deref()
                        .and_then(|path| crate::transcript::pi_session_id(path).ok())
                        .as_deref()
                        != Some(entry.publication.evidence.session_id.as_str())
            })
            .map(|(pane, _)| pane.clone())
            .collect();
        for pane in removed {
            self.remove(&pane);
        }
        self.bindings = bindings;
    }
}
