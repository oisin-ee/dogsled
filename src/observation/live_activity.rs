//! Serve-owned transient activity. Commands and native binding observations meet here;
//! neither HTTP readers nor IPC transports validate native files or mutate producer state.

use super::details::Session;
use portal_types::agents::{
    ActivityAck, ActivityCode, ActivityDetail, ActivityError, ActivityProducer, ActivityView,
    ProducerEvidence, ProducerPayload, ProducerUnavailable, PublishActivity, RegisterProducer,
    UnavailableReason,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

#[cfg(test)]
#[path = "tests/live_activity.rs"]
pub(crate) mod tests;

pub(crate) const MAX_BODY: usize = 64 * 1024;
pub(crate) const MAX_FRAME: usize = 2 * MAX_BODY;
const CAPACITY: usize = 128;
const SAFE_COUNTER: u64 = 9_007_199_254_740_991;
const LEASE: Duration = Duration::from_secs(15);
pub(super) const NATIVE_TTL_MS: u64 = 10_000;
const SAMPLE: Duration = Duration::from_secs(5);

type Outcome = std::result::Result<ActivityAck, ActivityError>;

pub(crate) fn error(code: ActivityCode, message: &str) -> ActivityError {
    ActivityError {
        code,
        error: message.into(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(
    tag = "operation",
    content = "body",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum Command {
    Register(RegisterProducer),
    Publish(PublishActivity),
}

#[derive(Debug)]
struct Pending {
    pane: String,
    command: Command,
    reply: oneshot::Sender<Outcome>,
}

/// Bounded command admission. Only the collector consumes and applies commands.
#[derive(Debug, Clone)]
pub(crate) struct Commands(mpsc::Sender<Pending>);

impl Commands {
    pub async fn send(
        &self,
        pane: String,
        command: Command,
        dirty: Option<&crate::service::wake::Dirty>,
    ) -> Outcome {
        let (reply, response) = oneshot::channel();
        self.0
            .try_send(Pending {
                pane,
                command,
                reply,
            })
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => error(
                    ActivityCode::Capacity,
                    "activity command capacity exhausted",
                ),
                mpsc::error::TrySendError::Closed(_) => error(
                    ActivityCode::UpstreamUnavailable,
                    "activity authority stopped",
                ),
            })?;
        if let Some(dirty) = dirty {
            dirty.mark(crate::service::wake::DETAILS);
        }
        tokio::time::timeout(Duration::from_secs(5), response)
            .await
            .map_err(|_| {
                error(
                    ActivityCode::UpstreamUnavailable,
                    "activity command timed out",
                )
            })?
            .map_err(|_| {
                error(
                    ActivityCode::UpstreamUnavailable,
                    "activity authority stopped",
                )
            })?
    }
}

#[derive(Debug)]
struct Binding {
    session: Option<Session>,
    state: String,
    evidence: Option<ProducerEvidence>,
    measured_ms: u64,
    generation: u64,
}

#[derive(Debug)]
struct Producer {
    registration: RegisterProducer,
    ack: ActivityAck,
    last: Option<PublishActivity>,
    received: Instant,
    received_ms: u64,
    closed: bool,
}

impl Producer {
    fn fresh(&self, now: Instant) -> bool {
        !self.closed && now.saturating_duration_since(self.received) < LEASE
    }
}

#[derive(Debug)]
pub(crate) struct Authority {
    epoch: String,
    generation: u64,
    bindings: BTreeMap<String, Binding>,
    producers: BTreeMap<String, Producer>,
    commands: Commands,
    pending: mpsc::Receiver<Pending>,
    next_sample: Instant,
    sampled_ms: Option<u64>,
    registration_interest: Option<Instant>,
}

impl Authority {
    pub fn new(epoch: String) -> Self {
        let (sender, pending) = mpsc::channel(CAPACITY);
        Self {
            epoch,
            generation: 0,
            bindings: BTreeMap::new(),
            producers: BTreeMap::new(),
            commands: Commands(sender),
            pending,
            next_sample: Instant::now(),
            sampled_ms: None,
            registration_interest: None,
        }
    }

    pub fn commands(&self) -> Commands {
        self.commands.clone()
    }

    pub fn next_wake(&self) -> Option<Instant> {
        self.next_wake_at(Instant::now())
    }

    fn next_wake_at(&self, now: Instant) -> Option<Instant> {
        (self.producers.values().any(|producer| producer.fresh(now))
            || self
                .registration_interest
                .is_some_and(|received| now.saturating_duration_since(received) < LEASE))
        .then_some(self.next_sample)
    }

    pub fn sample_due(&self) -> bool {
        self.sample_due_at(Instant::now())
    }

    fn sample_due_at(&self, now: Instant) -> bool {
        self.next_wake_at(now).is_some_and(|next| now >= next)
    }

    /// Only producer interest enables this cheap native binding read. It never captures
    /// conversation or screen bytes, and command receipt never triggers a read.
    pub fn sample(&mut self, shell: &dyn crate::shell::Shell, now_ms: u64) {
        if let Ok(agents) = crate::probe::agent_list(shell) {
            self.observe(&agents, now_ms, true);
            self.sampled_ms = Some(now_ms);
        }
        self.next_sample = Instant::now() + SAMPLE;
    }

    pub fn refresh(&mut self, input: &super::Publication) -> crate::error::Result<()> {
        self.refresh_at(input, Instant::now(), crate::ledger::now_ms())
    }

    fn refresh_at(
        &mut self,
        input: &super::Publication,
        now: Instant,
        now_ms: u64,
    ) -> crate::error::Result<()> {
        let mut pending = Vec::new();
        while let Ok(command) = self.pending.try_recv() {
            pending.push(command);
        }
        let measured = input.sources.local.get("terminals").copied();
        let valid = !input.gaps.iter().any(|gap| gap.domain == "terminals");
        if valid
            && let Some(measured) = measured
            && self.sampled_ms.is_none_or(|previous| measured >= previous)
        {
            let terminals = input
                .local
                .get("terminals")
                .map(|value| serde_json::from_value::<crate::probe::Terminals>(value.clone()))
                .transpose()?;
            if let Some(terminals) = terminals {
                self.observe(
                    &terminals.agents,
                    measured,
                    self.sampled_ms != Some(measured),
                );
                self.sampled_ms = Some(measured);
            }
        }
        for pending in pending {
            let result = self.apply(&pending.pane, pending.command, now, now_ms);
            let _ = pending.reply.send(result);
        }
        Ok(())
    }

    fn observe(&mut self, agents: &[crate::probe::Agent], measured_ms: u64, new_sample: bool) {
        let mut bindings = BTreeMap::new();
        for agent in agents {
            let session = Session::of(agent);
            let previous = self.bindings.get(&agent.pane);
            let changed = previous.is_some_and(|old| old.session != session);
            let mut generation = if changed {
                self.producers.remove(&agent.pane);
                self.generation = self.generation.saturating_add(1);
                self.generation
            } else {
                previous.map_or(self.generation, |binding| binding.generation)
            };
            let evidence = if new_sample || previous.is_none() || changed {
                verified_evidence(session.as_ref())
            } else {
                previous.and_then(|binding| binding.evidence.clone())
            };
            if let Some(producer) = self.producers.get(&agent.pane)
                && evidence.as_ref() != Some(&producer.registration.evidence)
            {
                self.producers.remove(&agent.pane);
                self.generation = self.generation.saturating_add(1);
                generation = self.generation;
            }
            bindings.insert(
                agent.pane.clone(),
                Binding {
                    session,
                    state: agent.state.clone(),
                    evidence,
                    measured_ms,
                    generation,
                },
            );
        }
        if self
            .bindings
            .keys()
            .any(|pane| !bindings.contains_key(pane))
        {
            self.generation = self.generation.saturating_add(1);
        }
        self.producers.retain(|pane, _| bindings.contains_key(pane));
        self.bindings = bindings;
    }

    fn apply(&mut self, pane: &str, command: Command, now: Instant, now_ms: u64) -> Outcome {
        validate_pane(pane)?;
        match command {
            Command::Register(body) => self.register(pane, body, now, now_ms),
            Command::Publish(body) => self.publish(pane, body, now, now_ms),
        }
    }

    fn binding(&self, pane: &str, now_ms: u64) -> std::result::Result<&Binding, ActivityError> {
        if self
            .sampled_ms
            .is_none_or(|measured| now_ms.saturating_sub(measured) >= NATIVE_TTL_MS)
        {
            return Err(error(
                ActivityCode::UpstreamUnavailable,
                "native binding unavailable",
            ));
        }
        let binding = self
            .bindings
            .get(pane)
            .ok_or_else(|| error(ActivityCode::AgentMissing, "agent is absent"))?;
        if now_ms.saturating_sub(binding.measured_ms) >= NATIVE_TTL_MS {
            return Err(error(
                ActivityCode::UpstreamUnavailable,
                "native binding is stale",
            ));
        }
        if binding.session.is_none() || binding.evidence.is_none() {
            return Err(error(
                ActivityCode::SessionUnverified,
                "Pi session evidence unavailable",
            ));
        }
        Ok(binding)
    }

    fn register(
        &mut self,
        pane: &str,
        body: RegisterProducer,
        now: Instant,
        now_ms: u64,
    ) -> Outcome {
        validate_registration(&body)?;
        let binding = match self.binding(pane, now_ms) {
            Ok(binding) => binding,
            Err(error) => {
                if error.code == ActivityCode::UpstreamUnavailable {
                    // A stale binding must arrange its own recovery even before a producer
                    // exists. Keep one host-wide lease because sampling reads the agent list.
                    // Receipt never changes next_sample or starts detail capture.
                    self.registration_interest = Some(now);
                }
                return Err(error);
            }
        };
        if binding.evidence.as_ref() != Some(&body.evidence)
            || verified_evidence(binding.session.as_ref()).as_ref() != Some(&body.evidence)
        {
            return Err(error(
                ActivityCode::SessionChanged,
                "native session evidence changed",
            ));
        }
        let session = serde_json::to_string(&binding.session.as_ref())
            .map_err(|_| error(ActivityCode::UpstreamInvalid, "invalid native session"))?;
        if let Some(current) = self.producers.get(pane)
            && current.registration == body
        {
            let mut ack = current.ack.clone();
            ack.sequence = 0;
            return Ok(ack);
        }
        let generation = binding.generation;
        if (body.epoch.as_deref() != Some(&self.epoch)
            && !(body.epoch.is_none() && generation == 0))
            || body.expected_generation != generation
        {
            return Err(error(
                ActivityCode::StaleGeneration,
                "producer registration no longer current",
            ));
        }
        if self
            .producers
            .get(pane)
            .is_some_and(|producer| producer.registration.producer_id == body.producer_id)
        {
            return Err(error(
                ActivityCode::StaleGeneration,
                "producer identity cannot be reused",
            ));
        }
        if !self.producers.contains_key(pane) && self.producers.len() >= CAPACITY {
            let expired = self
                .producers
                .iter()
                .find(|(_, producer)| !producer.fresh(now))
                .map(|(pane, _)| pane.clone());
            if let Some(expired) = expired {
                self.producers.remove(&expired);
            } else {
                return Err(error(
                    ActivityCode::Capacity,
                    "activity pane capacity exhausted",
                ));
            }
        }
        self.generation = self
            .generation
            .checked_add(1)
            .filter(|value| *value <= SAFE_COUNTER)
            .ok_or_else(|| error(ActivityCode::Capacity, "producer generation exhausted"))?;
        let ack = ActivityAck {
            version: 1,
            pane: pane.into(),
            epoch: self.epoch.clone(),
            generation: self.generation,
            producer_id: body.producer_id.clone(),
            session,
            sequence: 0,
            heartbeat_ms: 5000,
            ttl_ms: 15000,
        };
        if let Some(binding) = self.bindings.get_mut(pane) {
            binding.generation = self.generation;
        }
        self.producers.insert(
            pane.into(),
            Producer {
                registration: body,
                ack: ack.clone(),
                last: None,
                received: now,
                received_ms: now_ms,
                closed: false,
            },
        );
        Ok(ack)
    }

    fn publish(&mut self, pane: &str, body: PublishActivity, now: Instant, now_ms: u64) -> Outcome {
        validate_publication(&body)?;
        let binding = self.binding(pane, now_ms)?;
        let session = serde_json::to_string(&binding.session.as_ref())
            .map_err(|_| error(ActivityCode::UpstreamInvalid, "invalid native session"))?;
        if body.session != session || body.epoch != self.epoch {
            return Err(error(
                ActivityCode::SessionChanged,
                "activity session or epoch changed",
            ));
        }
        let current = self
            .producers
            .get_mut(pane)
            .ok_or_else(|| error(ActivityCode::StaleGeneration, "producer was revoked"))?;
        if body.generation != current.ack.generation || body.producer_id != current.ack.producer_id
        {
            return Err(error(
                ActivityCode::StaleGeneration,
                "producer was replaced",
            ));
        }
        if current.last.as_ref() == Some(&body) {
            return Ok(current.ack.clone());
        }
        if current.closed {
            return Err(error(ActivityCode::ProducerClosed, "producer is closed"));
        }
        if body.sequence <= current.ack.sequence {
            return Err(error(
                ActivityCode::SequenceConflict,
                "activity sequence conflicts",
            ));
        }
        current.closed = matches!(body.payload, ProducerPayload::Closed);
        current.received = now;
        current.received_ms = now_ms;
        current.ack.sequence = body.sequence;
        current.last = Some(body);
        Ok(current.ack.clone())
    }

    pub fn views(&self, now: Instant, now_ms: u64) -> BTreeMap<String, ActivityView> {
        self.bindings
            .iter()
            .map(|(pane, binding)| {
                let producer = self.producers.get(pane);
                let detail = Self::detail(binding, producer, now, now_ms);
                (
                    pane.clone(),
                    ActivityView {
                        version: 1,
                        pane: pane.clone(),
                        epoch: self.epoch.clone(),
                        revision: 0,
                        session: binding
                            .session
                            .as_ref()
                            .and_then(|session| serde_json::to_string(session).ok()),
                        coarse_state: Some(binding.state.clone()),
                        producer: ActivityProducer {
                            generation: binding.generation,
                            id: producer.map(|producer| producer.ack.producer_id.clone()),
                        },
                        detail,
                    },
                )
            })
            .collect()
    }

    fn detail(
        binding: &Binding,
        producer: Option<&Producer>,
        now: Instant,
        now_ms: u64,
    ) -> ActivityDetail {
        let unavailable = |reason| ActivityDetail::Unavailable { reason };
        if now_ms.saturating_sub(binding.measured_ms) >= NATIVE_TTL_MS {
            return unavailable(UnavailableReason::NativeUnavailable);
        }
        if binding
            .session
            .as_ref()
            .and_then(|session| session.harness.as_deref())
            != Some("pi")
        {
            return unavailable(UnavailableReason::UnsupportedHarness);
        }
        if binding.evidence.is_none() {
            return unavailable(UnavailableReason::SessionUnverified);
        }
        let Some(producer) = producer else {
            return unavailable(UnavailableReason::NoProducer);
        };
        if producer.closed {
            return unavailable(UnavailableReason::ProducerClosed);
        }
        if !producer.fresh(now) {
            return unavailable(UnavailableReason::Expired);
        }
        let Some(last) = &producer.last else {
            return unavailable(UnavailableReason::NoSnapshot);
        };
        match &last.payload {
            ProducerPayload::Snapshot { state } => ActivityDetail::Available {
                sequence: last.sequence,
                received_at_ms: producer.received_ms,
                valid_for_ms: u64::try_from(
                    LEASE
                        .saturating_sub(now.saturating_duration_since(producer.received))
                        .as_millis(),
                )
                .unwrap_or(0)
                .min(NATIVE_TTL_MS.saturating_sub(now_ms.saturating_sub(binding.measured_ms))),
                state: state.clone(),
            },
            ProducerPayload::Unavailable {
                reason: ProducerUnavailable::Resyncing,
            } => unavailable(UnavailableReason::Resyncing),
            ProducerPayload::Unavailable {
                reason: ProducerUnavailable::Overflow,
            } => unavailable(UnavailableReason::Overflow),
            ProducerPayload::Closed => unavailable(UnavailableReason::ProducerClosed),
        }
    }
}

fn verified_evidence(session: Option<&Session>) -> Option<ProducerEvidence> {
    let session = session?;
    if session.harness.as_deref() != Some("pi") {
        return None;
    }
    let path = session.transcript.as_deref()?;
    let id = crate::transcript::pi_session_id(path).ok()?;
    Some(ProducerEvidence {
        session_file: path.into(),
        session_id: id,
    })
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

pub(crate) fn validate_pane(pane: &str) -> std::result::Result<(), ActivityError> {
    if bounded(pane, 128) {
        Ok(())
    } else {
        Err(error(ActivityCode::InvalidRequest, "invalid activity pane"))
    }
}

fn version(value: u32) -> std::result::Result<(), ActivityError> {
    if value == 1 {
        Ok(())
    } else {
        Err(error(
            ActivityCode::UnsupportedVersion,
            "unsupported activity version",
        ))
    }
}

fn validate_registration(body: &RegisterProducer) -> std::result::Result<(), ActivityError> {
    version(body.version)?;
    if !bounded(&body.producer_id, 256)
        || !bounded(&body.evidence.session_file, 4096)
        || !bounded(&body.evidence.session_id, 1024)
        || body.expected_generation > SAFE_COUNTER
        || body
            .epoch
            .as_ref()
            .is_some_and(|epoch| !bounded(epoch, 256))
    {
        return Err(error(
            ActivityCode::InvalidRequest,
            "invalid producer registration",
        ));
    }
    Ok(())
}

fn validate_publication(body: &PublishActivity) -> std::result::Result<(), ActivityError> {
    version(body.version)?;
    if !bounded(&body.producer_id, 256)
        || !bounded(&body.epoch, 256)
        || body.session.is_empty()
        || body.session.len() > 16 * 1024
        || body.generation == 0
        || body.generation > SAFE_COUNTER
        || body.sequence == 0
        || body.sequence > SAFE_COUNTER
    {
        return Err(error(
            ActivityCode::InvalidRequest,
            "invalid activity correlation",
        ));
    }
    if let ProducerPayload::Snapshot { state } = &body.payload {
        use portal_types::agents::LivePhase;
        let mut ids = std::collections::BTreeSet::new();
        if state.active_tools.len() > 64
            || state
                .reasoning
                .as_ref()
                .is_some_and(|text| text.len() > 16 * 1024)
            || ((state.phase == LivePhase::ToolRunning) == state.active_tools.is_empty())
            || state.active_tools.iter().any(|tool| {
                !bounded(&tool.call_id, 256)
                    || !bounded(&tool.name, 128)
                    || tool.call.as_ref().is_some_and(|call| call.len() > 512)
                    || !ids.insert(&tool.call_id)
                    || tool
                        .parent_call_id
                        .as_ref()
                        .is_some_and(|parent| !bounded(parent, 256))
            })
        {
            return Err(error(
                ActivityCode::InvalidRequest,
                "invalid activity state",
            ));
        }
    }
    Ok(())
}
