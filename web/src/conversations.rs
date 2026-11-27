//! Shell-owned transcript snapshots survive keyed agent sessions.

use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use portal_types::agents::{AgentAddress, Conversation};
use portal_types::live::{ConversationCursor, ConversationUpdate, splice};

/// Keep complete transcripts for the sixteen most recently visited panes on each host.
const CAPACITY: usize = 16;

#[derive(Clone, Default)]
pub(crate) struct Conversations {
    hosts: BTreeMap<String, VecDeque<Snapshot>>,
}

#[derive(Clone)]
struct Snapshot {
    cursor: ConversationCursor,
    conversation: Rc<Conversation>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum UpdateResult {
    Current,
    Unavailable(String),
    NeedsReset,
}

impl Conversations {
    fn entry(&self, address: &AgentAddress) -> Option<&Snapshot> {
        self.hosts
            .get(&address.host)?
            .iter()
            .find(|entry| entry.conversation.pane == address.pane)
    }

    pub(crate) fn snapshot(&self, address: &AgentAddress) -> Option<Rc<Conversation>> {
        self.entry(address)
            .map(|entry| Rc::clone(&entry.conversation))
    }

    pub(crate) fn cursor(&self, address: &AgentAddress) -> Option<&ConversationCursor> {
        self.entry(address).map(|entry| &entry.cursor)
    }

    pub(crate) fn discard(&mut self, address: &AgentAddress) {
        if let Some(entries) = self.hosts.get_mut(&address.host) {
            entries.retain(|entry| entry.conversation.pane != address.pane);
        }
    }

    pub(crate) fn visit(&mut self, address: &AgentAddress) {
        if let Some(entries) = self.hosts.get_mut(&address.host)
            && let Some(index) = entries
                .iter()
                .position(|entry| entry.conversation.pane == address.pane)
            && let Some(entry) = entries.remove(index)
        {
            entries.push_back(entry);
        }
    }

    fn insert(
        &mut self,
        address: &AgentAddress,
        cursor: ConversationCursor,
        conversation: Conversation,
    ) {
        let entries = self.hosts.entry(address.host.clone()).or_default();
        entries.retain(|entry| entry.conversation.pane != address.pane);
        entries.push_back(Snapshot {
            cursor,
            conversation: Rc::new(conversation),
        });
        if entries.len() > CAPACITY {
            entries.pop_front();
        }
    }

    pub(crate) fn apply_update(
        &mut self,
        address: &AgentAddress,
        update: ConversationUpdate,
    ) -> UpdateResult {
        let (cursor, session, turns, truncated) = match update {
            ConversationUpdate::Confirm { cursor } => {
                return if self.cursor(address) == Some(&cursor) {
                    UpdateResult::Current
                } else {
                    UpdateResult::NeedsReset
                };
            }
            ConversationUpdate::Reset {
                cursor,
                session,
                turns,
                truncated,
            } => (cursor, session, turns, truncated),
            ConversationUpdate::Splice {
                base,
                cursor,
                drop_front,
                keep,
                turns,
                truncated,
            } => {
                let Some(entry) = self.entry(address) else {
                    return UpdateResult::NeedsReset;
                };
                if entry.cursor != base || base.token != cursor.token || base == cursor {
                    return UpdateResult::NeedsReset;
                }
                let Some(turns) = splice(&entry.conversation.turns, drop_front, keep, turns) else {
                    return UpdateResult::NeedsReset;
                };
                (cursor, entry.conversation.session.clone(), turns, truncated)
            }
            ConversationUpdate::Unavailable { error } => return UpdateResult::Unavailable(error),
        };
        self.insert(
            address,
            cursor,
            Conversation {
                pane: address.pane.clone(),
                session,
                turns,
                truncated,
            },
        );
        UpdateResult::Current
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use portal_types::agents::{Role, Source, TimeMeaning, Timestamp, Turn};

    fn turn(text: &str) -> Turn {
        Turn {
            id: text.to_owned(),
            role: Role::Assistant,
            text: text.to_owned(),
            source: Source::Harness {
                harness: "pi".into(),
                model: Some("recorded-model".into()),
                provider: Some("recorded-provider".into()),
            },
            timestamp: Some(Timestamp {
                ms: 42,
                meaning: TimeMeaning::Message,
            }),
            tool: None,
            end_turn: None,
        }
    }

    fn cursor(revision: u64) -> ConversationCursor {
        ConversationCursor {
            token: "epoch-session".into(),
            revision,
        }
    }

    fn reset(session: &str, turns: Vec<Turn>) -> ConversationUpdate {
        ConversationUpdate::Reset {
            cursor: cursor(1),
            session: session.into(),
            turns,
            truncated: false,
        }
    }

    #[test]
    fn apply_update_splices_a_bounded_window_and_preserves_turn_provenance() {
        let pane = AgentAddress::local("pane");
        let mut cache = Conversations::default();
        let kept = turn("keep");
        cache.apply_update(
            &pane,
            reset("native", vec![turn("drop"), kept.clone(), turn("replace")]),
        );
        let new = turn("new");
        assert_eq!(
            cache.apply_update(
                &pane,
                ConversationUpdate::Splice {
                    base: cursor(1),
                    cursor: cursor(2),
                    drop_front: 1,
                    keep: 1,
                    turns: vec![new.clone()],
                    truncated: true,
                }
            ),
            UpdateResult::Current
        );
        assert_eq!(cache.cursor(&pane), Some(&cursor(2)));
        assert_eq!(
            cache.snapshot(&pane).as_deref(),
            Some(&Conversation {
                pane: "pane".into(),
                session: "native".into(),
                turns: vec![kept, new],
                truncated: true,
            })
        );
    }

    #[test]
    fn apply_update_rejects_invalid_bases_and_retains_history_on_unavailability()
    -> Result<(), String> {
        let pane = AgentAddress::local("pane");
        let mut cache = Conversations::default();
        cache.apply_update(&pane, reset("native", vec![turn("retained")]));
        let original = cache.snapshot(&pane).ok_or("missing seeded snapshot")?;
        for (base, next, keep) in [
            (cursor(0), cursor(2), 1),
            (cursor(1), cursor(2), 2),
            (
                cursor(1),
                ConversationCursor {
                    token: "other".into(),
                    revision: 2,
                },
                1,
            ),
        ] {
            assert_eq!(
                cache.apply_update(
                    &pane,
                    ConversationUpdate::Splice {
                        base,
                        cursor: next,
                        drop_front: 0,
                        keep,
                        turns: vec![turn("wrong")],
                        truncated: false,
                    }
                ),
                UpdateResult::NeedsReset
            );
        }
        assert_eq!(
            cache.apply_update(&pane, ConversationUpdate::Confirm { cursor: cursor(2) }),
            UpdateResult::NeedsReset
        );
        assert_eq!(
            cache.apply_update(
                &pane,
                ConversationUpdate::Unavailable {
                    error: "read failed".into()
                }
            ),
            UpdateResult::Unavailable("read failed".into())
        );
        assert_eq!(
            cache.apply_update(&pane, ConversationUpdate::Confirm { cursor: cursor(1) }),
            UpdateResult::Current
        );
        assert!(
            cache
                .snapshot(&pane)
                .is_some_and(|current| Rc::ptr_eq(&current, &original))
        );
        assert_eq!(cache.cursor(&pane), Some(&cursor(1)));
        cache.apply_update(&pane, reset("replacement", Vec::new()));
        assert_eq!(
            cache.snapshot(&pane).as_deref(),
            Some(&Conversation {
                pane: "pane".into(),
                session: "replacement".into(),
                turns: Vec::new(),
                truncated: false,
            })
        );
        Ok(())
    }

    #[test]
    fn revisiting_protects_the_pane_from_lru_eviction_without_sharing_history_across_hosts() {
        let local = AgentAddress::local("0");
        let peer = AgentAddress {
            host: "peer".into(),
            pane: local.pane.clone(),
        };
        let mut cache = Conversations::default();
        cache.apply_update(&peer, reset("peer-native", vec![turn("remote")]));
        for index in 0..CAPACITY {
            cache.apply_update(
                &AgentAddress::local(index.to_string()),
                reset("native", vec![turn("local")]),
            );
        }
        cache.visit(&local);
        cache.apply_update(&AgentAddress::local("next"), reset("native", Vec::new()));
        assert_eq!(
            cache
                .snapshot(&local)
                .as_ref()
                .map(|snapshot| snapshot.session.as_str()),
            Some("native")
        );
        assert_eq!(
            cache
                .snapshot(&peer)
                .as_ref()
                .map(|snapshot| snapshot.session.as_str()),
            Some("peer-native")
        );
        assert!(cache.cursor(&AgentAddress::local("1")).is_none());
    }
}
