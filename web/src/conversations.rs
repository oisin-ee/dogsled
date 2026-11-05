//! Shell-owned transcript snapshots survive keyed agent sessions.

use std::collections::{BTreeMap, VecDeque};

use dioxus::prelude::*;
use portal_types::agents::{Conversation, DetailSession, Turn};

/// Keep complete transcripts for the sixteen most recently visited panes.
const CAPACITY: usize = 16;

#[derive(Clone, Default)]
pub struct Conversations {
    entries: VecDeque<Conversation>,
    sessions: BTreeMap<String, DetailSession>,
    generations: BTreeMap<String, u64>,
}

impl Conversations {
    pub fn get(&self, pane: &str) -> Option<&Vec<Turn>> {
        self.entries
            .iter()
            .find(|entry| entry.pane == pane)
            .map(|entry| &entry.turns)
    }

    pub fn access(&mut self, pane: &str) -> Option<&Vec<Turn>> {
        if let Some(index) = self.entries.iter().position(|entry| entry.pane == pane)
            && let Some(entry) = self.entries.remove(index)
        {
            self.entries.push_back(entry);
        }
        self.get(pane)
    }

    /// Keep completeness and provenance with the session-qualified history.
    pub fn get_session(&self, pane: &str, session: &str) -> Option<&Conversation> {
        self.entries
            .iter()
            .find(|entry| entry.pane == pane && entry.session == session)
    }

    fn begin_request(&mut self, pane: &str) -> u64 {
        let generation = self.generations.entry(pane.into()).or_default();
        *generation += 1;
        *generation
    }

    pub fn select_session(&mut self, pane: &str, session: &str) {
        let selected = DetailSession::Available(session.into());
        if self.sessions.get(pane) != Some(&selected) {
            self.begin_request(pane);
            self.entries.retain(|entry| entry.pane != pane);
            self.sessions.insert(pane.into(), selected);
        }
    }

    pub fn unavailable(&mut self, pane: &str) {
        self.begin_request(pane);
        self.entries.retain(|entry| entry.pane != pane);
        self.sessions
            .insert(pane.into(), DetailSession::Unavailable);
    }

    fn apply_response(
        &mut self,
        pane: &str,
        generation: u64,
        result: Result<Option<Conversation>, String>,
    ) -> Result<Option<String>, String> {
        if self.generations.get(pane) != Some(&generation) {
            return Ok(None);
        }
        let session = result
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .filter(|conversation| conversation.pane == pane && !conversation.session.is_empty())
            .map(|conversation| conversation.session.clone());
        if let Some(session) = &session {
            self.select_session(pane, session);
        }
        self.apply(pane, result)?;
        Ok(session)
    }

    fn apply(
        &mut self,
        pane: &str,
        result: Result<Option<Conversation>, String>,
    ) -> Result<(), String> {
        match result? {
            Some(conversation) if conversation.pane != pane => {
                Err("The conversation response belongs to another agent.".to_owned())
            }
            Some(conversation) => {
                if conversation.session.is_empty()
                    || self.sessions.get(pane).is_some_and(|session| {
                        session != &DetailSession::Available(conversation.session.clone())
                    })
                {
                    return Ok(());
                }
                self.sessions.insert(
                    pane.into(),
                    DetailSession::Available(conversation.session.clone()),
                );
                self.insert(conversation);
                Ok(())
            }
            None => {
                self.entries.retain(|entry| entry.pane != pane);
                Err("This agent is no longer available. Select another agent.".to_owned())
            }
        }
    }

    fn insert(&mut self, conversation: Conversation) {
        self.entries.retain(|entry| entry.pane != conversation.pane);
        self.entries.push_back(conversation);
        while self.entries.len() > CAPACITY {
            self.entries.pop_front();
        }
    }
}

/// Refresh only the transcript. Metadata and terminal reads have separate lifetimes.
pub fn use_conversation(
    pane: String,
    refresh: Signal<u64>,
    mut session: Signal<Option<DetailSession>>,
) -> Resource<Result<(), String>> {
    let mut conversations = use_context::<Signal<Conversations>>();
    use_hook(|| {
        let mut cache = conversations.write();
        cache.begin_request(&pane);
        cache.sessions.remove(&pane);
        cache.access(&pane);
    });
    use_resource(move || {
        let _ = refresh();
        let pane = pane.clone();
        let generation = conversations.write().begin_request(&pane);
        async move {
            let result = crate::api::fetch_conversation(&pane).await;
            let accepted = conversations
                .write()
                .apply_response(&pane, generation, result)?;
            if let Some(key) = accepted {
                session.set(Some(DetailSession::Available(key)));
            }
            Ok(())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use portal_types::agents::{Role, Source, TimeMeaning, Timestamp};

    fn snapshot(pane: &str, text: &str) -> Conversation {
        Conversation {
            pane: pane.to_owned(),
            session: format!("{pane}-session"),
            turns: vec![Turn {
                id: format!("{pane}-message"),
                role: Role::Assistant,
                text: text.to_owned(),
                source: Source::Harness {
                    harness: "pi".to_owned(),
                    model: Some("recorded-model".to_owned()),
                    provider: Some("recorded-provider".to_owned()),
                },
                timestamp: Some(Timestamp {
                    ms: 42,
                    meaning: TimeMeaning::Message,
                }),
                tool: None,
            }],
            truncated: false,
        }
    }

    #[test]
    fn session_switch_discards_old_text_and_rejects_late_responses() -> Result<(), String> {
        let mut cache = Conversations::default();
        let old = snapshot("pane", "old reply");
        cache.apply("pane", Ok(Some(old.clone())))?;
        cache.select_session("pane", "new-session");
        assert_eq!(cache.get("pane"), None);
        cache.apply("pane", Ok(Some(old)))?;
        assert_eq!(cache.get("pane"), None);
        let mut new = snapshot("pane", "new reply");
        new.session = "new-session".into();
        cache.apply("pane", Ok(Some(new.clone())))?;
        assert_eq!(cache.get("pane"), Some(&new.turns));
        Ok(())
    }

    #[test]
    fn polled_session_switch_accepts_current_response_and_rejects_late_response()
    -> Result<(), String> {
        let mut cache = Conversations::default();
        let first = cache.begin_request("pane");
        cache.apply_response("pane", first, Ok(Some(snapshot("pane", "old reply"))))?;
        let old_request = cache.begin_request("pane");
        let current_request = cache.begin_request("pane");
        let mut new = snapshot("pane", "new reply");
        new.session = "new-session".into();
        cache.apply_response("pane", current_request, Ok(Some(new.clone())))?;
        assert_eq!(
            cache.apply_response(
                "pane",
                old_request,
                Ok(Some(snapshot("pane", "late old reply")))
            )?,
            None
        );
        assert_eq!(cache.get_session("pane", "new-session"), Some(&new));
        assert_eq!(cache.get_session("pane", "pane-session"), None);
        Ok(())
    }

    #[test]
    fn unavailable_session_rejects_late_cached_content() -> Result<(), String> {
        let mut cache = Conversations::default();
        let old = snapshot("pane", "old reply");
        cache.apply("pane", Ok(Some(old.clone())))?;
        cache.unavailable("pane");
        cache.apply("pane", Ok(Some(old)))?;
        assert_eq!(cache.get("pane"), None);
        Ok(())
    }

    #[test]
    fn switching_panes_preserves_each_transcript_and_replaces_refreshes() -> Result<(), String> {
        let mut cache = Conversations::default();
        let first = snapshot("first", "first reply");
        let second = snapshot("second", "second reply");
        cache.apply("first", Ok(Some(first.clone())))?;
        cache.apply("second", Ok(Some(second.clone())))?;
        assert_eq!(cache.access("first"), Some(&first.turns));
        assert_eq!(cache.get("second"), Some(&second.turns));
        let updated = snapshot("first", "new reply");
        cache.apply("first", Ok(Some(updated.clone())))?;
        assert_eq!(cache.get("first"), Some(&updated.turns));
        assert_eq!(cache.get("second"), Some(&second.turns));
        assert_eq!(cache.entries.len(), 2);
        Ok(())
    }

    #[test]
    fn an_empty_transcript_is_loaded_but_an_unvisited_pane_is_not() -> Result<(), String> {
        let mut cache = Conversations::default();
        cache.apply(
            "empty",
            Ok(Some(Conversation {
                pane: "empty".to_owned(),
                session: "empty-session".into(),
                turns: Vec::new(),
                truncated: false,
            })),
        )?;
        assert_eq!(cache.access("empty"), Some(&Vec::new()));
        assert_eq!(cache.access("unvisited"), None);
        Ok(())
    }

    #[test]
    fn revisiting_a_pane_protects_it_from_bounded_lru_eviction() -> Result<(), String> {
        let mut cache = Conversations::default();
        for index in 0..16 {
            let pane = index.to_string();
            cache.apply(&pane, Ok(Some(snapshot(&pane, "reply"))))?;
        }
        cache.access("0");
        cache.apply("next", Ok(Some(snapshot("next", "reply"))))?;
        assert_eq!(cache.entries.len(), CAPACITY);
        assert!(cache.get("0").is_some());
        assert!(cache.get("1").is_none());
        assert!(cache.get("next").is_some());
        Ok(())
    }

    #[test]
    fn mismatched_responses_do_not_replace_or_create_a_snapshot() -> Result<(), String> {
        let mut cache = Conversations::default();
        let first = snapshot("first", "original reply");
        cache.apply("first", Ok(Some(first.clone())))?;
        let mismatch = snapshot("other", "wrong reply");
        assert!(cache.apply("first", Ok(Some(mismatch.clone()))).is_err());
        assert_eq!(cache.get("first"), Some(&first.turns));
        assert_eq!(cache.get("other"), None);
        assert!(cache.apply("new", Ok(Some(mismatch))).is_err());
        assert_eq!(cache.get("new"), None);
        Ok(())
    }

    #[test]
    fn missing_agent_invalidates_only_the_requested_pane() -> Result<(), String> {
        let mut cache = Conversations::default();
        let second = snapshot("second", "second reply");
        cache.apply("first", Ok(Some(snapshot("first", "first reply"))))?;
        cache.apply("second", Ok(Some(second.clone())))?;
        assert!(cache.apply("first", Ok(None)).is_err());
        assert_eq!(cache.get("first"), None);
        assert_eq!(cache.get("second"), Some(&second.turns));
        assert!(cache.apply("unvisited", Ok(None)).is_err());
        assert_eq!(cache.entries.len(), 1);
        Ok(())
    }

    #[test]
    fn refresh_failure_retains_completeness_and_provenance_for_only_the_selected_session()
    -> Result<(), String> {
        let mut cache = Conversations::default();
        let mut history = snapshot("pane", "retained reply");
        history.truncated = true;
        cache.apply("pane", Ok(Some(history.clone())))?;
        let generation = cache.begin_request("pane");
        assert!(
            cache
                .apply_response("pane", generation, Err("read failed".into()))
                .is_err()
        );
        assert_eq!(cache.get_session("pane", "pane-session"), Some(&history));
        assert_eq!(cache.get_session("pane", "other-session"), None);
        cache.select_session("pane", "other-session");
        assert_eq!(cache.get_session("pane", "pane-session"), None);
        assert_eq!(cache.get_session("pane", "other-session"), None);
        Ok(())
    }

    #[test]
    fn refresh_errors_retain_text_without_creating_a_false_empty_snapshot() -> Result<(), String> {
        let mut cache = Conversations::default();
        let first = snapshot("first", "retained reply");
        cache.apply("first", Ok(Some(first.clone())))?;
        let error = "Transcript temporarily unavailable".to_owned();
        assert_eq!(cache.apply("first", Err(error.clone())), Err(error.clone()));
        assert_eq!(cache.get("first"), Some(&first.turns));
        assert_eq!(cache.apply("unvisited", Err(error.clone())), Err(error));
        assert_eq!(cache.get("unvisited"), None);
        Ok(())
    }
}
