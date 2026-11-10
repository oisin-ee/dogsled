//! Durable portal questions and in-process wake-ups for long polls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use portal_types::questions::{AskBody, Pending, Rejection, ResponseItem, Responses};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::error::{Error, Result};
use crate::ledger::{self, Ledger};

use crate::web_store::questions as records;
/// A durable question, its resolution, and the resolution time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) pending: Pending,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) answers: Option<Vec<ResponseItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) rejection: Option<Rejection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) answered_ms: Option<u64>,
}

impl Entry {
    fn resolved(&self) -> bool {
        self.answers.is_some() || self.rejection.is_some()
    }
}

enum Resolution {
    Answers(Vec<ResponseItem>),
    Rejected(Rejection),
}

/// What a long poll found.
#[derive(Debug, Clone, PartialEq)]
pub enum Poll {
    /// The owner answered or rejected the interview. The outcome is retained.
    Answered(Responses),
    /// Nothing changed before the time ended.
    Timeout,
    /// The question was withdrawn, or it is not there.
    Gone,
}

/// What an answer found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answered {
    /// The answers are saved.
    Saved,
    /// The question is not there: it was withdrawn or never existed.
    Gone,
    /// The question already holds an answer or rejection.
    Already,
}

/// The questions the portal holds, with the waiters of the long polls.
#[derive(Debug, Clone)]
pub struct Store {
    ledger: Ledger,
    waiters: Arc<Mutex<HashMap<String, watch::Sender<()>>>>,
}

impl Store {
    /// A store over the directory of the given ledger.
    #[must_use]
    pub fn new(ledger: Ledger) -> Self {
        Self {
            ledger,
            waiters: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Keep a new question and return its id.
    pub async fn create(&self, body: AskBody) -> Result<String> {
        let ledger = self.ledger.clone();
        run(move || {
            let id = ledger::new_id();
            let entry = Entry {
                pending: Pending {
                    id: id.clone(),
                    pane: body.pane,
                    session: body.session,
                    cwd: body.cwd,
                    created_ms: ledger::now_ms(),
                    questions: body.questions,
                },
                answers: None,
                rejection: None,
                answered_ms: None,
            };
            crate::store::write(&ledger, |tx| records::put(tx, &entry))?;
            Ok(id)
        })
        .await
    }

    /// The questions that wait for an answer, oldest first.
    pub async fn list(&self) -> Result<Vec<Pending>> {
        let ledger = self.ledger.clone();
        run(move || pending(&ledger)).await
    }

    /// Keep the owner's answers and wake the long poll of the question.
    pub async fn answer(&self, id: &str, responses: Vec<ResponseItem>) -> Result<Answered> {
        self.resolve(id, Resolution::Answers(responses)).await
    }

    /// Keep a rejection through the same lock and wake-up as submitted answers.
    pub async fn reject(&self, id: &str, rejection: Rejection) -> Result<Answered> {
        self.resolve(id, Resolution::Rejected(rejection)).await
    }

    async fn resolve(&self, id: &str, resolution: Resolution) -> Result<Answered> {
        if !valid_id(id) {
            return Ok(Answered::Gone);
        }
        let ledger = self.ledger.clone();
        let owned = id.to_owned();
        let outcome = run(move || {
            crate::store::write(&ledger, |tx| {
                let Some(mut entry) = records::get(tx, &owned)? else {
                    return Ok(Answered::Gone);
                };
                if entry.resolved() {
                    return Ok(Answered::Already);
                }
                match resolution {
                    Resolution::Answers(responses) => entry.answers = Some(responses),
                    Resolution::Rejected(rejection) => entry.rejection = Some(rejection),
                }
                entry.answered_ms = Some(ledger::now_ms());
                records::put(tx, &entry)?;
                Ok(Answered::Saved)
            })
        })
        .await?;
        if outcome == Answered::Saved {
            self.wake(id);
        }
        Ok(outcome)
    }

    /// Cancel only a pending question. A racing answer or rejection must survive.
    pub async fn withdraw_pending(&self, id: &str) -> Result<Answered> {
        if !valid_id(id) {
            return Ok(Answered::Gone);
        }
        let ledger = self.ledger.clone();
        let owned = id.to_owned();
        let outcome = run(move || {
            crate::store::write(&ledger, |tx| {
                let Some(entry) = records::get(tx, &owned)? else {
                    return Ok(Answered::Gone);
                };
                if entry.resolved() {
                    return Ok(Answered::Already);
                }
                records::remove(tx, &owned)?;
                Ok(Answered::Saved)
            })
        })
        .await?;
        if outcome == Answered::Saved {
            self.wake(id);
            self.forget(id);
        }
        Ok(outcome)
    }

    /// Withdraw a question and wake its long poll. Return whether it was there.
    pub async fn withdraw(&self, id: &str) -> Result<bool> {
        if !valid_id(id) {
            return Ok(false);
        }
        let ledger = self.ledger.clone();
        let owned = id.to_owned();
        let removed =
            run(move || crate::store::write(&ledger, |tx| records::remove(tx, &owned))).await?;
        self.wake(id);
        self.forget(id);
        Ok(removed)
    }

    /// Wait for the answers of a question, for at most `limit`.
    ///
    /// The answers stay in the store until the question is withdrawn, so a
    /// second poll gets them again.
    pub async fn wait(&self, id: &str, limit: Duration) -> Result<Poll> {
        if !valid_id(id) {
            return Ok(Poll::Gone);
        }
        let deadline = tokio::time::Instant::now() + limit;
        // The receiver comes first, so a write between the read and the sleep
        // is not missed.
        let mut receiver = self.sender(id).subscribe();
        let poll = loop {
            if let Some(poll) = self.take(id).await? {
                break poll;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break Poll::Timeout;
            }
            match tokio::time::timeout(remaining, receiver.changed()).await {
                Ok(Ok(())) => {}
                // The sender is gone, so no wake-up comes. The poll reads again
                // after a short sleep.
                Ok(Err(_)) => tokio::time::sleep(Duration::from_millis(50)).await,
                Err(_) => break Poll::Timeout,
            }
        };
        // The waiter lets go of the sender before the store drops it, so the
        // store keeps no entry for a question nobody waits on.
        drop(receiver);
        self.forget(id);
        Ok(poll)
    }

    /// Read the question once. `None` means it still waits.
    async fn take(&self, id: &str) -> Result<Option<Poll>> {
        let ledger = self.ledger.clone();
        let owned = id.to_owned();
        run(move || {
            crate::store::read(&ledger, |tx| {
                let Some(entry) = records::get(tx, &owned)? else {
                    return Ok(Some(Poll::Gone));
                };
                if !entry.resolved() {
                    return Ok(None);
                }
                Ok(Some(Poll::Answered(Responses {
                    id: owned,
                    responses: entry.answers.unwrap_or_default(),
                    rejection: entry.rejection,
                })))
            })
        })
        .await
    }

    /// The sender that wakes the waiters of a question.
    fn sender(&self, id: &str) -> watch::Sender<()> {
        self.waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(id.to_owned())
            .or_insert_with(|| watch::channel(()).0)
            .clone()
    }

    /// Wake the waiters of a question.
    fn wake(&self, id: &str) {
        let sender = self
            .waiters
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        if let Some(sender) = sender {
            sender.send_replace(());
        }
    }

    /// Drop the sender of a question that no waiter holds any more.
    fn forget(&self, id: &str) {
        let mut waiters = self.waiters.lock().unwrap_or_else(PoisonError::into_inner);
        let unused = waiters
            .get(id)
            .is_some_and(|sender| sender.receiver_count() == 0);
        if unused {
            waiters.remove(id);
        }
    }
}

/// The result of work that reads or writes the question files, run on a
/// blocking thread so it never holds a runtime thread while it waits for the
/// disk.
pub(crate) fn pending(ledger: &Ledger) -> Result<Vec<Pending>> {
    crate::store::read(ledger, records::pending)
}

async fn run<T: Send + 'static>(work: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result,
        Err(error) => Err(Error::Invalid(format!(
            "the portal's own work failed: {error}"
        ))),
    }
}

/// Whether an id belongs to the portal question namespace.
pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use portal_types::questions::QuestionSet;

    fn store() -> Store {
        let dir = std::env::temp_dir().join(format!("dogsled-questions-{}", ledger::new_id()));
        Store::new(Ledger::new(dir))
    }

    fn ask() -> AskBody {
        AskBody {
            questions: QuestionSet {
                title: None,
                description: None,
                questions: Vec::new(),
                extra: serde_json::Map::new(),
            },
            pane: "%1".to_owned(),
            session: "s".to_owned(),
            cwd: "/tmp".to_owned(),
        }
    }

    #[tokio::test]
    async fn a_hostile_id_is_gone() {
        let store = store();
        assert_eq!(
            store
                .wait("../x", Duration::from_millis(10))
                .await
                .expect("wait"),
            Poll::Gone
        );
        assert!(!store.withdraw("../x").await.expect("withdraw"));
    }

    #[tokio::test]
    async fn two_waiters_of_one_question_both_get_the_answers() {
        let store = store();
        let id = store.create(ask()).await.expect("create");
        let first = {
            let store = store.clone();
            let id = id.clone();
            tokio::spawn(async move { store.wait(&id, crate::test_support::WAIT_DEADLINE).await })
        };
        let second = {
            let store = store.clone();
            let id = id.clone();
            tokio::spawn(async move { store.wait(&id, crate::test_support::WAIT_DEADLINE).await })
        };
        // The waiters are up before the answer comes.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            store.answer(&id, Vec::new()).await.expect("answer"),
            Answered::Saved
        );
        assert!(matches!(
            first.await.expect("the waiter").expect("the poll"),
            Poll::Answered(_)
        ));
        assert!(matches!(
            second.await.expect("the waiter").expect("the poll"),
            Poll::Answered(_)
        ));
    }

    #[tokio::test]
    async fn the_answers_stay_until_the_question_is_withdrawn() {
        let store = store();
        let id = store.create(ask()).await.expect("create");
        assert_eq!(store.list().await.expect("list").len(), 1);
        assert_eq!(
            store
                .wait(&id, Duration::from_millis(20))
                .await
                .expect("wait"),
            Poll::Timeout
        );
        assert_eq!(
            store.answer(&id, Vec::new()).await.expect("answer"),
            Answered::Saved
        );
        assert_eq!(
            store.answer(&id, Vec::new()).await.expect("answer"),
            Answered::Already
        );
        // The question is answered, so it is not pending any more.
        assert_eq!(store.list().await.expect("list").len(), 0);
        // A poll that lost its answer gets it again.
        for _ in 0..2 {
            assert!(matches!(
                store
                    .wait(&id, Duration::from_millis(20))
                    .await
                    .expect("wait"),
                Poll::Answered(_)
            ));
        }
        // A withdrawn question is gone.
        assert!(store.withdraw(&id).await.expect("withdraw"));
        assert_eq!(
            store
                .wait(&id, Duration::from_millis(20))
                .await
                .expect("wait"),
            Poll::Gone
        );
    }
}
