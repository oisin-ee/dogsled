//! Shell-owned transcript snapshots survive keyed agent sessions.

use std::collections::VecDeque;

use dioxus::prelude::*;
use portal_types::agents::{Conversation, Turn};

/// Keep complete transcripts for the sixteen most recently visited panes.
const CAPACITY: usize = 16;

#[derive(Clone, Default)]
pub struct Conversations {
    entries: VecDeque<Conversation>,
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
pub fn use_conversation(pane: String, refresh: Signal<u64>) -> Resource<Result<(), String>> {
    let mut conversations = use_context::<Signal<Conversations>>();
    use_hook(|| {
        conversations.write().access(&pane);
    });
    use_resource(move || {
        let _ = refresh();
        let pane = pane.clone();
        async move {
            let result = crate::api::fetch_conversation(&pane).await;
            // Do not subscribe this resource to the cache it updates.
            if let Ok(Some(conversation)) = &result
                && conversation.pane == pane
                && conversations.peek().get(&pane) == Some(&conversation.turns)
            {
                return Ok(());
            }
            conversations.write().apply(&pane, result)
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use portal_types::agents::Role;

    fn snapshot(pane: &str, text: &str) -> Conversation {
        Conversation {
            pane: pane.to_owned(),
            turns: vec![Turn {
                role: Role::Assistant,
                text: text.to_owned(),
            }],
        }
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
                turns: Vec::new(),
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
