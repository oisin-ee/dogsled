//! The one owner of an agent's lifecycle state. A pure derivation over positive evidence:
//! silence never proves completion, and missing evidence is `unknown`, never `stopped`,
//! `starting` or `done`. Raw Herdr terminal states stay on `probe::Agent` for the capacity
//! and safety guards; Herdr's `done` is normalized here and nowhere else. Agent state,
//! resource closure and result stay separate facts: this module derives only the first.

use crate::probe::Agent;
use serde::{Deserialize, Serialize};

/// One state per agent, each with a positive signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Waiting for admission.
    Queued,
    /// Admission or startup is in progress.
    Starting,
    /// In a turn.
    Working,
    /// The turn ended, and the agent waits for work it owns.
    Waiting,
    /// The turn ended with a question for its owner.
    Asking,
    /// The turn ended, and the agent cannot continue until something else changes.
    Blocked,
    /// `done` was accepted, but the final handoff is not complete.
    Finishing,
    /// The final handoff is recorded.
    Done,
    /// The turn ended with no signal.
    Stopped,
    /// The assigned host is unavailable, or the launched pane is gone without a final
    /// handoff.
    Lost,
    /// The state cannot be established from the available evidence.
    Unknown,
}

impl State {
    /// The state name, as the board and the host snapshot publish it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Starting => "starting",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Asking => "asking",
            Self::Blocked => "blocked",
            Self::Finishing => "finishing",
            Self::Done => "done",
            Self::Stopped => "stopped",
            Self::Lost => "lost",
            Self::Unknown => "unknown",
        }
    }
}

/// A typed turn signal. The signal units record these; this slice always passes `None`,
/// so the derivation already accepts them and no consumer can derive its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Signal {
    /// The agent asked its owner a question.
    Ask,
    /// The agent reported that it cannot continue.
    Blocked,
}

/// Where the native `done` handoff stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Handoff {
    /// No native `done` was recorded.
    #[default]
    None,
    /// `done` was accepted, but the sealed reply or terminal evidence is not complete.
    Accepted,
    /// Native `done`, the sealed final reply and the required terminal evidence are recorded.
    Recorded,
}

/// How the assigned host answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Host {
    /// The host answered and its service is up.
    #[default]
    Up,
    /// The host cannot be read. Nothing it last saw is current.
    Unavailable,
}

/// What the launch evidence says about the pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Launch {
    /// No launch evidence names the pane.
    #[default]
    None,
    /// A launch is in progress: it has not been confirmed.
    Progress,
    /// The launch is confirmed.
    Confirmed,
    /// Competing launch records name the pane. They contradict each other, so they prove
    /// neither a launch nor the absence of one.
    Ambiguous,
}

/// What the evidence says about the pane's existence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pane {
    /// The pane was not observed.
    #[default]
    Unseen,
    /// The pane was observed and it runs an agent.
    Present,
    /// Fresh, identity-correlated proof that the launched pane no longer exists.
    Gone,
}

/// What is known about one agent. Every field is positive evidence: an absent field is an
/// absent fact, never a conclusion.
#[derive(Debug, Clone, Copy, Default)]
pub struct Evidence<'a> {
    /// How the assigned host answered.
    pub host: Host,
    /// Herdr's raw terminal state for the correlated pane, when one is observed.
    pub terminal: Option<&'a str>,
    /// What the launch evidence says about the pane.
    pub launch: Launch,
    /// What the evidence says about the pane's existence.
    pub pane: Pane,
    /// The native `done` handoff.
    pub handoff: Handoff,
    /// The agent owns open items or pending or admitting child tickets.
    pub owns_open: bool,
    /// An explicit turn signal.
    pub signal: Option<Signal>,
    /// A pending queue ticket without execution evidence.
    pub queued: bool,
    /// The sources disagree about this agent.
    pub contradictory: bool,
}

/// The derived state and, when the state is not positively established, the evidence gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Derived {
    pub state: State,
    pub gap: Option<&'static str>,
}

const fn known(state: State) -> Derived {
    Derived { state, gap: None }
}

const fn gap(state: State, gap: &'static str) -> Derived {
    Derived {
        state,
        gap: Some(gap),
    }
}

/// Derive the lifecycle state. This function is the only owner of the mapping.
#[must_use]
pub fn classify(e: &Evidence) -> Derived {
    if e.contradictory {
        return gap(State::Unknown, "contradictory evidence");
    }
    let Some(terminal) = e.terminal else {
        // No agent was observed. A recorded final handoff stays done, even when its host
        // later became unavailable. Missing agent data alone is never `starting`.
        if e.handoff == Handoff::Recorded {
            return known(State::Done);
        }
        if e.host == Host::Unavailable {
            return gap(State::Lost, "host unavailable");
        }
        if e.pane == Pane::Gone {
            return gap(State::Lost, "pane gone without a final handoff");
        }
        if e.queued && e.launch == Launch::None {
            return known(State::Queued);
        }
        if e.launch == Launch::Progress {
            return known(State::Starting);
        }
        // Competing launch records contradict each other. They prove neither a launch nor
        // its absence, so no state is claimed from them.
        if e.launch == Launch::Ambiguous {
            return gap(State::Unknown, "ambiguous launch evidence");
        }
        return gap(State::Unknown, "no agent observed");
    };
    if e.host == Host::Unavailable {
        // The host cannot be read. A recorded final handoff still proves the handoff;
        // nothing else survives an unreachable host.
        return if e.handoff == Handoff::Recorded {
            known(State::Done)
        } else {
            gap(State::Lost, "host unavailable")
        };
    }
    match terminal {
        // A live turn outranks a recorded handoff: the agent is in a turn right now.
        // The host clears its own `done` flag for a live turn, so a stale one proves
        // nothing.
        "working" => match e.handoff {
            // The child called `done` and is writing its final reply: the handoff is
            // pending.
            Handoff::Accepted => known(State::Finishing),
            _ => known(State::Working),
        },
        "blocked" => known(State::Blocked),
        // A launch that has not been confirmed is still starting up. A live turn would
        // have proved it, so the terminal never outranks this.
        _ if e.launch == Launch::Progress => known(State::Starting),
        // Competing launch records contradict each other. The turn ended, but the launch
        // evidence about it is contradictory, so no state is claimed from it.
        _ if e.launch == Launch::Ambiguous => gap(State::Unknown, "ambiguous launch evidence"),
        _ if ended(terminal) => match e.signal {
            Some(Signal::Blocked) => known(State::Blocked),
            Some(Signal::Ask) => known(State::Asking),
            // The native `done` was accepted, and the final handoff is not complete.
            None if e.handoff == Handoff::Accepted => known(State::Finishing),
            // The final handoff is recorded.
            None if e.handoff == Handoff::Recorded => known(State::Done),
            None if e.owns_open => known(State::Waiting),
            None => known(State::Stopped),
        },
        // An unreadable terminal state never proves completion.
        _ => gap(State::Unknown, "unreadable terminal state"),
    }
}

/// Herdr `idle` and `done` both mean only that the turn ended. Herdr's `done` is not the
/// native `done` handoff.
fn ended(terminal: &str) -> bool {
    matches!(terminal, "idle" | "done")
}

/// The native `done` handoff an agent published. A host that predates the published
/// handoff offers only the two booleans, so the read falls back to them.
#[must_use]
pub fn handoff(agent: &Agent) -> Handoff {
    if let Some(name) = agent.handoff.as_deref() {
        return match name {
            "recorded" => Handoff::Recorded,
            "accepted" => Handoff::Accepted,
            _ => Handoff::None,
        };
    }
    if agent.done {
        Handoff::Recorded
    } else if agent.finishing {
        Handoff::Accepted
    } else {
        Handoff::None
    }
}

/// The state of an observed agent, from the facts published with it.
#[must_use]
pub fn of_agent(agent: &Agent, host: Host) -> Derived {
    classify(&Evidence {
        host,
        terminal: Some(agent.state.as_str()),
        handoff: handoff(agent),
        owns_open: agent.owns_open,
        launch: agent_launch(agent),
        pane: Pane::Present,
        ..Evidence::default()
    })
}

/// What the launch progress the host published with the agent says.
fn agent_launch(agent: &Agent) -> Launch {
    match agent.launch_progress.as_deref() {
        Some(name) if name == crate::launch::LaunchProgress::Confirmed.as_str() => {
            Launch::Confirmed
        }
        Some(name) if name == crate::launch::LaunchProgress::Ambiguous.as_str() => {
            Launch::Ambiguous
        }
        Some(_) => Launch::Progress,
        None => Launch::None,
    }
}

/// Publish the derived state and its evidence gap with the agent, so every reader of the
/// host snapshot sees the state the host observed.
pub fn publish(agent: &mut Agent, host: Host) {
    let derived = of_agent(agent, host);
    agent.lifecycle = Some(derived.state.as_str().to_owned());
    agent.lifecycle_gap = derived.gap.map(str::to_owned);
}

/// The board's published word for a lifecycle state. The board keeps its wording: a turn
/// that ended reads `idle`, and a host that cannot be reached reads `unavailable`. The
/// precise state travels beside it, in the row's `lifecycle` field.
#[must_use]
pub fn display(state: State) -> &'static str {
    match state {
        State::Working => "working",
        State::Blocked => "blocked",
        State::Queued => "queued",
        State::Starting => "starting",
        State::Done => "done",
        State::Lost => "unavailable",
        State::Unknown => "unknown",
        State::Waiting | State::Stopped | State::Finishing | State::Asking => "idle",
    }
}

/// The evidence for one owner-side item, from the host reports. The host's own liveness
/// decides whether its answer counts, and the launch record decides whether the host has
/// already had a chance to see the launched pane.
pub fn evidence<'a>(
    item: &crate::items::Item,
    hosts: &'a [crate::hosts::HostReport],
) -> (Evidence<'a>, Option<&'a Agent>) {
    let report = hosts.iter().find(|h| h.host.id == item.host);
    let host = if report.is_some_and(|h| h.service == "up" && h.reachable) {
        Host::Up
    } else {
        Host::Unavailable
    };
    let agent = report.and_then(|h| h.agent(&item.pane));
    // The host's sample and the launch confirmation share the host's clock, so their order
    // proves whether the host has already looked after the launch.
    let confirmed = item.launch_confirmed_ms;
    let sampled = report.and_then(|h| h.sampled_ms);
    let seen =
        confirmed.is_none_or(|confirmed| sampled.is_some_and(|sampled| sampled >= confirmed));
    // A launch that has not been confirmed is startup in progress. The host publishes the
    // progress it recorded for each agent; without an observed agent, the host's own
    // sample dates the launch confirmation.
    let launch = match agent {
        Some(agent) => agent_launch(agent),
        None => {
            if confirmed.is_some_and(|confirmed| sampled.is_some_and(|sampled| sampled < confirmed))
            {
                Launch::Progress
            } else {
                Launch::None
            }
        }
    };
    // The host looked after the launch, and the pane is neither an agent nor a pane: the
    // launched pane is gone. Only a launched item has this proof; a tracked pane that was
    // never launched proves nothing.
    let pane = if agent.is_some() {
        Pane::Present
    } else if confirmed.is_some()
        && seen
        && report
            .and_then(|h| h.probe.as_ref())
            .is_some_and(|p| !p.panes.iter().any(|pane| pane.pane_id == item.pane))
    {
        Pane::Gone
    } else {
        Pane::Unseen
    };
    let evidence = Evidence {
        host,
        terminal: agent.map(|a| a.state.as_str()),
        handoff: match agent {
            // The host published what it observed, and it is the current answer.
            Some(agent) => handoff(agent),
            // No agent is observed. A recorded handoff is durable, so it outlasts the
            // pane and the host that published it.
            None if recorded_handoff(item, report) => Handoff::Recorded,
            None => Handoff::None,
        },
        owns_open: agent.is_some_and(|a| a.owns_open),
        launch,
        pane,
        ..Evidence::default()
    };
    (evidence, agent)
}

/// The recorded final handoff of an item whose pane is no longer observed. The mark the
/// child host kept, with its saved final reply, or the reply the owner imported into the
/// item, proves that the native `done` was accepted and its handoff recorded. The handoff
/// outlasts the pane, so an unavailable host does not erase it.
fn recorded_handoff(item: &crate::items::Item, report: Option<&crate::hosts::HostReport>) -> bool {
    if item.reply.as_ref().is_some_and(|reply| reply.done) {
        return true;
    }
    report.is_some_and(|report| {
        report.handoffs.iter().any(|mark| {
            mark.pane == item.pane
                && mark.item.as_deref().is_none_or(|id| id == item.id)
                && mark.reply.is_some()
        })
    })
}

/// The evidence for a queued request: a pending ticket with no execution evidence, or a
/// ticket whose admission is in progress.
#[must_use]
pub fn ticket_evidence(state: &str) -> Evidence<'static> {
    match state {
        "admitting" => Evidence {
            host: Host::Up,
            launch: Launch::Progress,
            ..Evidence::default()
        },
        _ => Evidence {
            host: Host::Up,
            queued: true,
            ..Evidence::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(terminal: &str) -> Evidence<'_> {
        Evidence {
            host: Host::Up,
            terminal: Some(terminal),
            ..Evidence::default()
        }
    }

    fn state(e: &Evidence) -> State {
        classify(e).state
    }

    #[test]
    fn every_state_has_a_positive_signal() {
        assert_eq!(state(&up("working")), State::Working);
        assert_eq!(state(&up("blocked")), State::Blocked);
        assert_eq!(state(&up("idle")), State::Stopped);
        assert_eq!(state(&up("done")), State::Stopped);
        let waiting = Evidence {
            owns_open: true,
            ..up("idle")
        };
        assert_eq!(state(&waiting), State::Waiting);
        let queued = Evidence {
            host: Host::Up,
            queued: true,
            ..Evidence::default()
        };
        assert_eq!(state(&queued), State::Queued);
        let starting = Evidence {
            host: Host::Up,
            launch: Launch::Progress,
            ..Evidence::default()
        };
        assert_eq!(state(&starting), State::Starting);
        let asking = Evidence {
            host: Host::Up,
            terminal: Some("idle"),
            signal: Some(Signal::Ask),
            ..Evidence::default()
        };
        assert_eq!(state(&asking), State::Asking);
        let blocked = Evidence {
            host: Host::Up,
            terminal: Some("idle"),
            signal: Some(Signal::Blocked),
            ..Evidence::default()
        };
        assert_eq!(state(&blocked), State::Blocked);
        let finishing = Evidence {
            host: Host::Up,
            terminal: Some("idle"),
            handoff: Handoff::Accepted,
            ..Evidence::default()
        };
        assert_eq!(state(&finishing), State::Finishing);
        let done = Evidence {
            host: Host::Up,
            terminal: Some("idle"),
            handoff: Handoff::Recorded,
            ..Evidence::default()
        };
        assert_eq!(state(&done), State::Done);
        let lost = Evidence {
            host: Host::Unavailable,
            terminal: Some("working"),
            ..Evidence::default()
        };
        assert_eq!(state(&lost), State::Lost);
        let unknown = Evidence {
            host: Host::Up,
            terminal: Some("unknown"),
            ..Evidence::default()
        };
        assert_eq!(state(&unknown), State::Unknown);
    }

    #[test]
    fn herdr_done_alone_is_never_the_final_handoff() {
        let d = classify(&up("done"));
        assert_eq!(d.state, State::Stopped);
        assert_ne!(d.state, State::Done);
    }

    #[test]
    fn unreadable_terminal_state_is_unknown_with_a_gap() {
        for raw in ["unknown", "", "mystery"] {
            let d = classify(&up(raw));
            assert_eq!(d.state, State::Unknown, "{raw}");
            assert!(d.gap.is_some());
        }
    }

    #[test]
    fn missing_agent_data_alone_is_unknown_not_starting_or_stopped() {
        let d = classify(&Evidence {
            host: Host::Up,
            ..Evidence::default()
        });
        assert_eq!(d.state, State::Unknown);
        assert_eq!(d.gap, Some("no agent observed"));
    }

    #[test]
    fn starting_needs_launch_progress() {
        let e = Evidence {
            host: Host::Up,
            launch: Launch::Progress,
            ..Evidence::default()
        };
        assert_eq!(state(&e), State::Starting);
    }

    #[test]
    fn queued_needs_a_pending_ticket_without_execution() {
        let e = Evidence {
            host: Host::Up,
            queued: true,
            ..Evidence::default()
        };
        assert_eq!(state(&e), State::Queued);
        let started = Evidence {
            launch: Launch::Progress,
            ..e
        };
        assert_eq!(state(&started), State::Starting);
    }

    #[test]
    fn open_children_wait_and_no_children_stop() {
        let waiting = Evidence {
            owns_open: true,
            ..up("idle")
        };
        assert_eq!(state(&waiting), State::Waiting);
        assert_eq!(state(&up("idle")), State::Stopped);
    }

    #[test]
    fn open_children_do_not_hide_a_working_turn() {
        let e = Evidence {
            owns_open: true,
            ..up("working")
        };
        assert_eq!(state(&e), State::Working);
    }

    #[test]
    fn explicit_signals_outrank_inferred_waiting() {
        let ask = Evidence {
            owns_open: true,
            signal: Some(Signal::Ask),
            ..up("idle")
        };
        assert_eq!(state(&ask), State::Asking);
        let blocked = Evidence {
            owns_open: true,
            signal: Some(Signal::Blocked),
            ..up("idle")
        };
        assert_eq!(state(&blocked), State::Blocked);
    }

    #[test]
    fn a_signal_does_not_outlive_its_turn() {
        let e = Evidence {
            signal: Some(Signal::Ask),
            ..up("working")
        };
        assert_eq!(state(&e), State::Working);
    }

    #[test]
    fn accepted_handoff_is_finishing_until_recorded() {
        for raw in ["working", "idle"] {
            let e = Evidence {
                handoff: Handoff::Accepted,
                ..up(raw)
            };
            assert_eq!(state(&e), State::Finishing, "{raw}");
        }
        let recorded = Evidence {
            handoff: Handoff::Recorded,
            ..up("idle")
        };
        assert_eq!(state(&recorded), State::Done);
    }

    #[test]
    fn a_recorded_handoff_stays_done_when_the_host_is_lost() {
        let e = Evidence {
            host: Host::Unavailable,
            handoff: Handoff::Recorded,
            ..Evidence::default()
        };
        assert_eq!(state(&e), State::Done);
        let observed = Evidence {
            host: Host::Unavailable,
            handoff: Handoff::Recorded,
            terminal: Some("idle"),
            ..Evidence::default()
        };
        assert_eq!(state(&observed), State::Done);
    }

    #[test]
    fn a_live_turn_outranks_a_stale_recorded_handoff() {
        let e = Evidence {
            handoff: Handoff::Recorded,
            ..up("working")
        };
        assert_eq!(state(&e), State::Working);
    }

    #[test]
    fn unavailable_host_without_a_handoff_is_lost() {
        let gone = Evidence {
            host: Host::Unavailable,
            terminal: Some("working"),
            ..Evidence::default()
        };
        assert_eq!(state(&gone), State::Lost);
        let unseen = Evidence {
            host: Host::Unavailable,
            ..Evidence::default()
        };
        assert_eq!(state(&unseen), State::Lost);
    }

    #[test]
    fn proven_gone_pane_is_lost_only_with_a_fresh_host() {
        let e = Evidence {
            host: Host::Up,
            pane: Pane::Gone,
            ..Evidence::default()
        };
        assert_eq!(state(&e), State::Lost);
        let observed = Evidence {
            pane: Pane::Present,
            ..e
        };
        assert_ne!(state(&observed), State::Lost);
    }

    #[test]
    fn contradictory_evidence_is_unknown_even_with_a_handoff() {
        let e = Evidence {
            contradictory: true,
            handoff: Handoff::Recorded,
            ..up("idle")
        };
        let d = classify(&e);
        assert_eq!(d.state, State::Unknown);
        assert_eq!(d.gap, Some("contradictory evidence"));
    }

    #[test]
    fn of_agent_reads_the_published_agent_facts() {
        let mut agent = Agent {
            pane: "p1".into(),
            state: "idle".into(),
            ..Agent::default()
        };
        assert_eq!(of_agent(&agent, Host::Up).state, State::Stopped);
        agent.owns_open = true;
        assert_eq!(of_agent(&agent, Host::Up).state, State::Waiting);
        agent.finishing = true;
        assert_eq!(of_agent(&agent, Host::Up).state, State::Finishing);
        agent.done = true;
        assert_eq!(of_agent(&agent, Host::Up).state, State::Done);
        assert_eq!(of_agent(&agent, Host::Unavailable).state, State::Done);
        assert_eq!(agent.state, "idle", "the raw terminal state stays intact");
    }

    #[test]
    fn publish_writes_the_state_and_its_gap() {
        let mut agent = Agent {
            pane: "p1".into(),
            state: "unknown".into(),
            ..Agent::default()
        };
        publish(&mut agent, Host::Up);
        assert_eq!(agent.lifecycle.as_deref(), Some("unknown"));
        assert!(agent.lifecycle_gap.is_some());
        let mut agent = Agent {
            pane: "p1".into(),
            state: "working".into(),
            ..Agent::default()
        };
        publish(&mut agent, Host::Up);
        assert_eq!(agent.lifecycle.as_deref(), Some("working"));
        assert!(agent.lifecycle_gap.is_none());
    }

    /// A recorded final handoff survives the pane it belongs to. The host report carries
    /// the mark the child host kept, with its saved final reply.
    #[test]
    fn a_recorded_handoff_stays_done_when_the_pane_is_gone() {
        let (mut item, mut hosts) = host_with(agent("w2:p1", "idle"));
        // The pane is gone: the host no longer observes an agent for it.
        hosts[0].probe.as_mut().unwrap().agent_panes.clear();
        // The child host kept the mark, and the mark holds the final reply.
        hosts[0].handoffs.push(crate::done::Mark {
            pane: item.pane.clone(),
            item: Some(item.id.clone()),
            session: Some("session".into()),
            name: Some("worker".into()),
            after: Some(10),
            sealed: Some(0),
            marked_ms: 1,
            branch: None,
            detached: false,
            reply: Some(crate::items::Reply {
                seq: 0,
                result_id: None,
                text: "done".into(),
                captured_ms: 2,
                reported_ms: None,
                done: true,
            }),
            closed_ms: None,
        });
        let (found, observed) = evidence(&item, &hosts);
        assert!(observed.is_none(), "the pane is not observed any more");
        assert_eq!(found.handoff, Handoff::Recorded, "{found:?}");
        assert_eq!(classify(&found).state, State::Done);
        // The host is unreachable as well: the recorded handoff still proves the handoff.
        hosts[0].reachable = false;
        hosts[0].service = "down".into();
        let (found, _) = evidence(&item, &hosts);
        assert_eq!(found.handoff, Handoff::Recorded, "{found:?}");
        assert_eq!(classify(&found).state, State::Done);
        // A pane that is gone without a recorded handoff stays lost.
        item.pane = "w3:p1".into();
        let (found, _) = evidence(&item, &hosts);
        assert_eq!(found.handoff, Handoff::None, "{found:?}");
    }

    /// The owner imported the final reply into the item. That reply is the durable
    /// handoff, so the item stays done after its pane is gone.
    #[test]
    fn an_imported_final_reply_keeps_a_gone_item_done() {
        let (mut item, mut hosts) = host_with(agent("w2:p1", "idle"));
        hosts[0].probe.as_mut().unwrap().agent_panes.clear();
        item.reply = Some(crate::items::Reply {
            seq: 3,
            result_id: None,
            text: "the final reply".into(),
            captured_ms: 5,
            reported_ms: None,
            done: true,
        });
        let (found, _) = evidence(&item, &hosts);
        assert_eq!(found.handoff, Handoff::Recorded, "{found:?}");
        assert_eq!(classify(&found).state, State::Done);
    }

    /// Competing launch records contradict each other. They prove neither a launch nor its
    /// absence, so an idle agent never reads as stopped.
    #[test]
    fn competing_launch_records_are_unknown_not_stopped() {
        let (item, mut hosts) = host_with(agent("w2:p1", "idle"));
        hosts[0].probe.as_mut().unwrap().agent_panes[0].launch_progress =
            Some(crate::launch::LaunchProgress::Ambiguous.as_str().to_owned());
        let (found, _) = evidence(&item, &hosts);
        assert_eq!(found.launch, Launch::Ambiguous, "{found:?}");
        let derived = classify(&found);
        assert_eq!(derived.state, State::Unknown);
        assert!(derived.gap.is_some());
        // A live turn still outranks the contradiction: the turn itself is proved.
        hosts[0].probe.as_mut().unwrap().agent_panes[0].state = "working".into();
        let (found, _) = evidence(&item, &hosts);
        assert_eq!(classify(&found).state, State::Working);
    }

    /// The board's evidence builder needs host reports. A local host with one agent.
    fn host_with(agent: Agent) -> (crate::items::Item, Vec<crate::hosts::HostReport>) {
        let item = crate::items::Item {
            id: "i1".into(),
            kind: "agent".into(),
            host: "local".into(),
            pane: agent.pane.clone(),
            owner_pane: "w1:p1".into(),
            name: None,
            created_ms: 1,
            closed_ms: None,
            launch_confirmed_ms: Some(10),
            ..crate::items::Item::default()
        };
        let report = crate::hosts::HostReport {
            host: crate::hosts::Host {
                id: "local".into(),
                label: "local".into(),
                target: None,
            },
            service: "up".into(),
            reachable: true,
            sampled_ms: Some(20),
            probe: Some(crate::probe::Probe {
                host: "local".into(),
                label: "local".into(),
                load1: 0.0,
                cores: 1,
                cpu_ticks: None,
                busy: None,
                memory: crate::probe::Memory::default(),
                disk_free_percent: 50.0,
                agents: crate::probe::AgentCounts::default(),
                agent_panes: vec![agent],
                reservations: Vec::new(),
                panes: Vec::new(),
            }),
            headroom: None,
            error: None,
            failure: None,
            jobs: Vec::new(),
            handoffs: Vec::new(),
            retry_ms: None,
            job_validity: None,
        };
        (item, vec![report])
    }

    fn agent(pane: &str, state: &str) -> Agent {
        Agent {
            pane: pane.into(),
            state: state.into(),
            ..Agent::default()
        }
    }

    #[test]
    fn a_launch_the_host_has_not_looked_after_yet_is_starting() {
        let (item, mut hosts) = host_with(agent("w2:p1", "idle"));
        // The host has not observed the pane yet, and its sample predates the launch
        // confirmation: it has not looked after the launch.
        hosts[0].probe.as_mut().unwrap().agent_panes.clear();
        hosts[0].sampled_ms = Some(5);
        let (evidence, _) = evidence(&item, &hosts);
        assert_eq!(evidence.launch, Launch::Progress, "{evidence:?}");
        assert_eq!(classify(&evidence).state, State::Starting);
    }

    #[test]
    fn a_launch_the_host_has_looked_after_is_not_starting() {
        let (item, hosts) = host_with(agent("w2:p1", "idle"));
        let (evidence, _) = evidence(&item, &hosts);
        assert_eq!(evidence.launch, Launch::None, "{evidence:?}");
        assert_eq!(classify(&evidence).state, State::Stopped);
    }

    #[test]
    fn a_launched_pane_that_is_gone_is_lost_without_a_handoff() {
        let (item, mut hosts) = host_with(agent("w2:p1", "idle"));
        hosts[0].probe.as_mut().unwrap().agent_panes.clear();
        let (evidence, _) = evidence(&item, &hosts);
        assert_eq!(evidence.pane, Pane::Gone, "{evidence:?}");
        assert_eq!(classify(&evidence).state, State::Lost);
    }

    #[test]
    fn a_tracked_pane_that_never_launched_is_not_lost() {
        let (mut item, mut hosts) = host_with(agent("w2:p1", "idle"));
        item.launch_confirmed_ms = None;
        hosts[0].probe.as_mut().unwrap().agent_panes.clear();
        let (evidence, _) = evidence(&item, &hosts);
        assert_eq!(evidence.pane, Pane::Unseen, "{evidence:?}");
        assert_eq!(classify(&evidence).state, State::Unknown);
    }

    /// Older hosts publish no lifecycle fields, and older readers ignore them. A snapshot
    /// without the new fields must still decode.
    #[test]
    fn a_snapshot_without_the_published_fields_still_decodes() {
        let agent: Agent = serde_json::from_value(serde_json::json!({
            "pane":"w1:p1","agent":"pi","state":"idle"
        }))
        .unwrap();
        assert_eq!(agent.lifecycle, None);
        assert_eq!(agent.lifecycle_gap, None);
        assert_eq!(agent.handoff, None);
        assert_eq!(agent.launch_progress, None);
        assert!(!agent.done);
        assert!(!agent.finishing);
        assert!(!agent.owns_open);
        // The state still derives from what the old snapshot holds.
        assert_eq!(of_agent(&agent, Host::Up).state, State::Stopped);
    }

    #[test]
    fn the_board_word_keeps_its_wording() {
        assert_eq!(display(State::Working), "working");
        assert_eq!(display(State::Stopped), "idle");
        assert_eq!(display(State::Waiting), "idle");
        assert_eq!(display(State::Finishing), "idle");
        assert_eq!(display(State::Asking), "idle");
        assert_eq!(display(State::Done), "done");
        assert_eq!(display(State::Lost), "unavailable");
        assert_eq!(display(State::Unknown), "unknown");
        assert_eq!(display(State::Queued), "queued");
        assert_eq!(display(State::Starting), "starting");
        assert_eq!(display(State::Blocked), "blocked");
    }
}
