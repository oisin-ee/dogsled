//! Live activity for the selected agent, kept separate from history and lifecycle.
//!
//! Contract shared by the activity data hook and the timeline. Detailed phases come only
//! from fresh telemetry that matches the selected native session and the current serve
//! epoch. Anything else is a gap with a reason; a gap is never idle or finished.

use std::collections::BTreeSet;
use std::time::Duration;

use dioxus::prelude::*;
use dioxus_sdk_time::sleep;
use portal_types::agents::{
    ActivityDetail, ActivityView, DetailSession, LivePhase, LiveState, UnavailableReason,
};

/// What the portal knows right now about the selected agent's activity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Live {
    /// No answer yet for this pane and session.
    Pending,
    /// Fresh telemetry for the selected session in the current serve epoch.
    Detail(LiveState),
    /// Detailed activity is not available, and why.
    Unavailable(Gap),
}

/// Why detailed activity is unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gap {
    /// The server reported the reason, such as an unsupported harness or expired telemetry.
    Server(UnavailableReason),
    /// The activity stream or request failed; telemetry is unknown.
    Disconnected,
    /// The activity belongs to another session than the selected conversation.
    SessionMismatch,
    /// The server sent a frame the portal could not accept.
    Invalid,
}

#[derive(Clone)]
struct ActivityState {
    live: Live,
    ttl: Option<u64>,
    epoch: Option<String>,
    revision: u64,
    generation: u64,
    sequence: u64,
}

impl Default for ActivityState {
    fn default() -> Self {
        Self {
            live: Live::Pending,
            ttl: None,
            epoch: None,
            revision: 0,
            generation: 0,
            sequence: 0,
        }
    }
}

/// `None` means an identical publication, not a fresh receipt or a renewed timer.
impl ActivityState {
    fn clear(&mut self, gap: Gap) -> Live {
        self.live = Live::Unavailable(gap);
        self.ttl = None;
        self.live.clone()
    }

    fn frame(
        &mut self,
        view: ActivityView,
        pane: &str,
        selected: &Option<DetailSession>,
    ) -> Result<Option<(Live, Option<u64>)>, Gap> {
        let result = self.reduce(view, pane, selected);
        match &result {
            Ok(Some((live, ttl))) => {
                self.live = live.clone();
                self.ttl = *ttl;
            }
            Err(gap) => {
                self.clear(*gap);
            }
            Ok(None) => {}
        }
        result
    }

    fn reduce(
        &mut self,
        view: ActivityView,
        pane: &str,
        selected: &Option<DetailSession>,
    ) -> Result<Option<(Live, Option<u64>)>, Gap> {
        if !valid(&view) || view.pane != pane {
            return Err(Gap::Invalid);
        }
        if self
            .epoch
            .as_ref()
            .is_some_and(|epoch| epoch != &view.epoch)
        {
            return Err(Gap::Invalid);
        }
        self.epoch.get_or_insert_with(|| view.epoch.clone());
        if self.revision > view.revision || self.generation > view.producer.generation {
            return Err(Gap::Invalid);
        }
        if selected.is_none() {
            return Ok(Some((Live::Pending, None)));
        }
        if !matches!(selected, Some(DetailSession::Available(key)) if Some(key) == view.session.as_ref())
        {
            return Ok(Some((Live::Unavailable(Gap::SessionMismatch), None)));
        }
        if self.revision == view.revision {
            return Ok(None);
        }
        self.revision = view.revision;
        if view.producer.generation > self.generation {
            self.sequence = 0;
        }
        self.generation = view.producer.generation;
        match view.detail {
            ActivityDetail::Unavailable { reason } => {
                Ok(Some((Live::Unavailable(Gap::Server(reason)), None)))
            }
            ActivityDetail::Available {
                sequence,
                valid_for_ms,
                state,
                ..
            } => {
                if sequence < self.sequence {
                    return Err(Gap::Invalid);
                }
                if valid_for_ms == 0 {
                    return Ok(Some((
                        Live::Unavailable(Gap::Server(UnavailableReason::Expired)),
                        None,
                    )));
                }
                // A newer authority revision can refresh native evidence even when the
                // producer sequence is unchanged. Only repeated publications are duplicates.
                self.sequence = sequence;
                Ok(Some((Live::Detail(state), Some(valid_for_ms))))
            }
        }
    }
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

fn valid(view: &ActivityView) -> bool {
    const SAFE: u64 = 9_007_199_254_740_991;
    if view.version != 1
        || !bounded(&view.pane, 128)
        || !bounded(&view.epoch, 256)
        || view.revision == 0
        || view.revision > SAFE
        || view.producer.generation > SAFE
        || view
            .session
            .as_ref()
            .is_some_and(|key| !bounded(key, 16 * 1024))
        || view
            .producer
            .id
            .as_ref()
            .is_some_and(|id| !bounded(id, 256))
    {
        return false;
    }
    let ActivityDetail::Available {
        sequence,
        received_at_ms,
        valid_for_ms,
        state,
    } = &view.detail
    else {
        return true;
    };
    let mut ids = BTreeSet::new();
    *sequence > 0
        && *sequence <= SAFE
        && *received_at_ms <= SAFE
        && *valid_for_ms <= 15_000
        && view.producer.generation > 0
        && view.producer.id.is_some()
        && view.session.is_some()
        && state.active_tools.len() <= 64
        && ((state.phase == LivePhase::ToolRunning) != state.active_tools.is_empty())
        && state.active_tools.iter().all(|tool| {
            bounded(&tool.call_id, 256)
                && bounded(&tool.name, 128)
                && ids.insert(&tool.call_id)
                && tool
                    .parent_call_id
                    .as_ref()
                    .is_none_or(|id| bounded(id, 256))
        })
}

/// Follow the selected pane's activity. Changing `session` clears detail until a frame for
/// the new session arrives. Heartbeats and activity frames never refresh history.
pub fn use_live_activity(
    address: portal_types::agents::AgentAddress,
    session: Signal<Option<DetailSession>>,
) -> Signal<Live> {
    let mut live = use_signal(|| Live::Pending);
    let mut expiry = use_signal(|| None::<(u64, u64)>);
    // Changing a receipt replaces the sleeping future; duplicate frames do not touch it.
    use_resource(move || {
        let expiry = expiry();
        async move {
            if let Some((_, milliseconds)) = expiry {
                sleep(Duration::from_millis(milliseconds)).await;
                live.set(Live::Unavailable(Gap::Server(UnavailableReason::Expired)));
            }
        }
    });
    use_resource(use_reactive(&address, move |address| {
        let selected = session();
        live.set(Live::Pending);
        expiry.set(None);
        async move {
            loop {
                let mut state = ActivityState::default();
                let mut apply = |view: ActivityView| -> Result<(), Gap> {
                    // Recheck the current selection before every async callback.
                    if *session.peek() != selected {
                        live.set(Live::Unavailable(Gap::SessionMismatch));
                        expiry.set(None);
                        return Err(Gap::SessionMismatch);
                    }
                    if let Some((next, ttl)) = state.frame(view, &address.pane, &selected)? {
                        expiry.set(ttl.map(|ttl| (state.revision, ttl)));
                        live.set(next);
                    }
                    Ok(())
                };
                // GET establishes the current serve epoch before opening this stream.
                let result = match crate::api::fetch_activity(&address).await {
                    Ok(view) => match apply(view) {
                        Ok(()) => crate::api::watch_activity(&address, apply).await,
                        Err(gap) => Err(gap),
                    },
                    Err(gap) => Err(gap),
                };
                expiry.set(None);
                live.set(state.clear(result.err().unwrap_or(Gap::Disconnected)));
                if *session.peek() != selected {
                    return;
                }
                sleep(Duration::from_secs(1)).await;
            }
        }
    }));
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use portal_types::agents::{ActiveTool, ActivityProducer};

    fn selected() -> Option<DetailSession> {
        Some(DetailSession::Available("native".into()))
    }

    fn view() -> ActivityView {
        ActivityView {
            version: 1,
            pane: "pane".into(),
            epoch: "serve".into(),
            revision: 1,
            session: Some("native".into()),
            coarse_state: Some("running".into()),
            producer: ActivityProducer {
                generation: 1,
                id: Some("producer".into()),
            },
            detail: ActivityDetail::Available {
                sequence: 1,
                received_at_ms: 100,
                valid_for_ms: 500,
                state: LiveState {
                    reasoning: None,
                    phase: LivePhase::Thinking,
                    active_tools: vec![],
                },
            },
        }
    }

    #[test]
    fn fresh_frame_requires_selected_native_identity() -> Result<(), Gap> {
        for session in [
            Some(DetailSession::Unavailable),
            Some(DetailSession::Available("other".into())),
        ] {
            assert_eq!(
                ActivityState::default().frame(view(), "pane", &session)?,
                Some((Live::Unavailable(Gap::SessionMismatch), None))
            );
        }
        assert!(matches!(
            ActivityState::default().frame(view(), "pane", &selected())?,
            Some((Live::Detail(_), Some(500)))
        ));
        Ok(())
    }

    #[test]
    fn unknown_selected_session_stays_pending_without_detail() -> Result<(), Gap> {
        assert_eq!(
            ActivityState::default().frame(view(), "pane", &None)?,
            Some((Live::Pending, None))
        );
        Ok(())
    }

    #[test]
    fn epoch_replacement_clears_detail_and_requires_a_new_get() -> Result<(), Gap> {
        let mut state = ActivityState::default();
        state.frame(view(), "pane", &selected())?;
        let mut replacement = view();
        replacement.epoch = "restarted".into();
        assert_eq!(
            state.frame(replacement.clone(), "pane", &selected()),
            Err(Gap::Invalid)
        );
        assert!(matches!(
            ActivityState::default().frame(replacement, "pane", &selected())?,
            Some((Live::Detail(_), _))
        ));
        Ok(())
    }

    #[test]
    fn duplicate_frames_never_renew_expiry_but_new_native_evidence_can() -> Result<(), Gap> {
        let mut state = ActivityState::default();
        state.frame(view(), "pane", &selected())?;
        assert_eq!(state.frame(view(), "pane", &selected())?, None);
        let mut refreshed = view();
        refreshed.revision += 1;
        assert!(matches!(
            state.frame(refreshed, "pane", &selected())?,
            Some((Live::Detail(_), Some(500)))
        ));
        assert_eq!(state.frame(view(), "pane", &selected()), Err(Gap::Invalid));
        Ok(())
    }

    #[test]
    fn invalid_state_and_unknown_identity_never_mean_idle() {
        let mut invalid = view();
        if let ActivityDetail::Available { state, .. } = &mut invalid.detail {
            state.active_tools.push(ActiveTool {
                call: None,
                call_id: "call".into(),
                name: "read".into(),
                parent_call_id: None,
            });
        }
        assert_eq!(
            ActivityState::default().frame(invalid, "pane", &selected()),
            Err(Gap::Invalid)
        );
        assert_eq!(
            ActivityState::default().frame(view(), "wrong-pane", &selected()),
            Err(Gap::Invalid)
        );
    }

    #[test]
    fn disconnect_clears_detail_and_its_expiry_without_inventing_idle() -> Result<(), Gap> {
        let mut state = ActivityState::default();
        state.frame(view(), "pane", &selected())?;
        assert!(matches!(state.live, Live::Detail(_)));
        assert_eq!(
            state.clear(Gap::Disconnected),
            Live::Unavailable(Gap::Disconnected)
        );
        assert_eq!(state.ttl, None);
        assert_eq!(state.epoch.as_deref(), Some("serve"));
        assert_eq!(state.frame(view(), "pane", &selected())?, None);
        assert_eq!(state.live, Live::Unavailable(Gap::Disconnected));
        Ok(())
    }

    #[test]
    fn expiry_and_unavailability_replace_only_live_detail() -> Result<(), Gap> {
        let mut expired = view();
        if let ActivityDetail::Available { valid_for_ms, .. } = &mut expired.detail {
            *valid_for_ms = 0;
        }
        assert_eq!(
            ActivityState::default().frame(expired, "pane", &selected())?,
            Some((
                Live::Unavailable(Gap::Server(UnavailableReason::Expired)),
                None
            ))
        );
        let mut missing = view();
        missing.detail = ActivityDetail::Unavailable {
            reason: UnavailableReason::NoProducer,
        };
        assert_eq!(
            ActivityState::default().frame(missing, "pane", &selected())?,
            Some((
                Live::Unavailable(Gap::Server(UnavailableReason::NoProducer)),
                None
            ))
        );
        Ok(())
    }
}
