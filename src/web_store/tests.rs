use portal_types::questions::{Pending, Rejection, ResponseItem};
use portal_types::reviews::{Annotation, Decision, Review, Revision};
use serde_json::json;

use super::{board, inbox, questions, reviews};
use crate::board::{TodoChild, TodoItem, TodoSection, TodoSections, TodoStatus};
use crate::error::Result;
use crate::ledger::Ledger;
use crate::store::{read, write};

fn ledger() -> Ledger {
    Ledger::new(std::env::temp_dir().join(crate::ledger::new_id()))
}

#[test]
fn inbox_terminals_readers_and_records_round_trip_and_reject_malformed_updates() -> Result<()> {
    let ledger = ledger();
    let reader = crate::inbox::Reader {
        harness: "pi".into(),
        pane: "p".into(),
        registered_ms: 12,
    };
    let record: crate::inbox::Record = serde_json::from_value(json!({
        "id":"record", "terminal":"terminal", "pane":"pane", "from":{"name":"lead","pane":"owner","terminal":"sender","host":"host"},
        "kind":"task", "text":"message", "created_ms":10,"state":"claimed","attempts":2,"next_ms":13,"reason":"reason","done_ms":14
    }))?;
    write(&ledger, |tx| {
        inbox::put_reader(tx, "terminal", &reader)?;
        inbox::put_records(tx, "terminal", std::slice::from_ref(&record))
    })?;
    assert_eq!(
        read(&ledger, |tx| inbox::reader(tx, "terminal"))?,
        Some(reader)
    );
    assert_eq!(
        read(&ledger, |tx| inbox::records(tx, "terminal"))?,
        vec![record.clone()]
    );
    assert_eq!(read(&ledger, inbox::terminals)?, ["terminal"]);
    let mut bad = record.clone();
    bad.text = "x".repeat(crate::inbox::MAX_TEXT + 1);
    assert!(write(&ledger, |tx| inbox::put_records(tx, "terminal", &[bad])).is_err());
    assert_eq!(
        read(&ledger, |tx| inbox::records(tx, "terminal"))?,
        vec![record.clone()]
    );
    write(&ledger, |tx| {
        inbox::put_records(
            tx,
            "other",
            &[crate::inbox::Record {
                terminal: "other".into(),
                ..record
            }],
        )
    })?;
    assert_eq!(read(&ledger, |tx| inbox::records(tx, "terminal"))?.len(), 1);
    write(&ledger, |tx| inbox::put_records(tx, "terminal", &[]))?;
    assert_eq!(read(&ledger, |tx| inbox::records(tx, "terminal"))?, []);
    assert!(
        write(&ledger, |tx| inbox::put_reader(
            tx,
            "../bad",
            &crate::inbox::Reader {
                harness: "pi".into(),
                pane: "p".into(),
                registered_ms: 1
            }
        ))
        .is_err()
    );
    assert!(read(&ledger, |tx| inbox::reader(tx, "../bad"))?.is_none());
    write(&ledger, |tx| inbox::remove_terminal(tx, "terminal"))?;
    assert!(read(&ledger, |tx| inbox::reader(tx, "terminal"))?.is_none());
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}

#[test]
fn sent_and_failure_tables_round_trip_and_clear_updates() -> Result<()> {
    let ledger = ledger();
    let mut sent: crate::inbox::Sent = serde_json::from_value(json!({
        "id":"sent", "host":"remote", "host_label":"Remote", "terminal":"target", "pane":"p", "name":"worker", "harness":"pi",
        "from":{"name":"lead","pane":"o","terminal":"sender","host":"local"}, "kind":"task", "created_ms":1,
        "state":"failed", "reason":"gone", "check":{"attempts":3,"next_ms":99}
    }))?;
    write(&ledger, |tx| {
        inbox::put_sent(tx, std::slice::from_ref(&sent))?;
        inbox::put_failure(tx, "event", "failed")
    })?;
    assert_eq!(read(&ledger, inbox::sent)?, vec![sent.clone()]);
    assert_eq!(read(&ledger, inbox::failures)?["event"], "failed");
    sent.name = None;
    sent.harness = None;
    sent.reason = None;
    sent.from = crate::inbox::Sender::default();
    write(&ledger, |tx| {
        inbox::put_sent(tx, std::slice::from_ref(&sent))?;
        inbox::put_failure(tx, "event", "changed")
    })?;
    assert_eq!(read(&ledger, inbox::sent)?, vec![sent.clone()]);
    assert_eq!(read(&ledger, inbox::failures)?["event"], "changed");
    assert!(write(&ledger, |tx| inbox::put_failure(tx, "", "bad")).is_err());
    assert_eq!(read(&ledger, inbox::failures)?.len(), 1);
    let mut bad = sent.clone();
    bad.created_ms = u64::MAX;
    assert!(write(&ledger, |tx| inbox::put_sent(tx, &[bad])).is_err());
    assert_eq!(read(&ledger, inbox::sent)?, vec![sent]);
    write(&ledger, |tx| inbox::put_sent(tx, &[]))?;
    assert_eq!(read(&ledger, inbox::sent)?, []);
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}

#[test]
fn docks_and_sidebar_lists_round_trip_and_refuse_malformed_input() -> Result<()> {
    use crate::sidebar::model::{Panel, Saved};
    let ledger = ledger();
    let dock = board::Dock {
        pane_id: "pane".into(),
        terminal_id: "terminal".into(),
        tab_id: Some("tab".into()),
    };
    let mut saved = Saved {
        focus: Panel::Todos,
        collapsed: vec![Panel::Hosts, Panel::Agents],
        fold_open: true,
        selected_card: Some("card".into()),
    };
    write(&ledger, |tx| {
        board::put_dock(tx, "tab", &dock)?;
        board::put_sidebar(tx, "owner", &saved)
    })?;
    assert_eq!(
        read(&ledger, |tx| board::dock(tx, "tab"))?,
        Some(dock.clone())
    );
    assert_eq!(
        read(&ledger, |tx| board::sidebar(tx, "owner"))?,
        Some(saved.clone())
    );
    saved.collapsed.clear();
    saved.selected_card = None;
    write(&ledger, |tx| board::put_sidebar(tx, "owner", &saved))?;
    assert_eq!(
        read(&ledger, |tx| board::sidebar(tx, "owner"))?,
        Some(saved.clone())
    );
    assert!(write(&ledger, |tx| board::put_sidebar(tx, "", &saved)).is_err());
    assert!(
        write(&ledger, |tx| board::put_dock(
            tx,
            "tab",
            &board::Dock {
                pane_id: String::new(),
                ..dock.clone()
            }
        ))
        .is_err()
    );
    assert_eq!(read(&ledger, |tx| board::dock(tx, "tab"))?, Some(dock));
    assert!(read(&ledger, |tx| board::sidebar(tx, ""))?.is_none());
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}

fn todo() -> TodoSections {
    TodoSections {
        session: "session".into(),
        branch: Some("branch".into()),
        revision: 3,
        current: "current".into(),
        pane: "pane".into(),
        sections: [
            TodoStatus::InProgress,
            TodoStatus::Pending,
            TodoStatus::Completed,
            TodoStatus::Abandoned,
        ]
        .into_iter()
        .map(|status| TodoSection {
            status,
            count: 1,
            items: vec![TodoItem {
                status,
                text: "item".into(),
                description: Some("description".into()),
                review: true,
                unit: Some("unit".into()),
                children: vec![TodoChild {
                    status,
                    text: "child".into(),
                    description: Some("child description".into()),
                    unit: Some("child unit".into()),
                }],
            }],
        })
        .collect(),
    }
}

#[test]
fn todo_snapshot_sections_items_and_children_round_trip_and_clear_updates() -> Result<()> {
    let ledger = ledger();
    let mut todo = todo();
    write(&ledger, |tx| {
        board::put_todo(tx, "published", &todo)?;
        board::cache_todos(tx, std::slice::from_ref(&todo))
    })?;
    for lane in ["published", "cached"] {
        assert_eq!(
            read(&ledger, |tx| board::todo(tx, lane, "pane"))?,
            Some(todo.clone())
        );
    }
    let mut bad = todo.clone();
    bad.sections[0].items[0].status = TodoStatus::Pending;
    assert!(write(&ledger, |tx| board::put_todo(tx, "published", &bad)).is_err());
    assert_eq!(
        read(&ledger, |tx| board::todo(tx, "published", "pane"))?,
        Some(todo.clone())
    );
    todo.branch = None;
    todo.sections[0].items[0].children.clear();
    todo.sections[1].items.clear();
    write(&ledger, |tx| board::put_todo(tx, "published", &todo))?;
    assert_eq!(
        read(&ledger, |tx| board::todo(tx, "published", "pane"))?,
        Some(todo)
    );
    write(&ledger, |tx| board::cache_todos(tx, &[]))?;
    assert_eq!(read(&ledger, |tx| board::todos(tx, "cached"))?, []);
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}

#[test]
fn question_forms_answers_and_attachments_round_trip_and_clear_updates() -> Result<()> {
    let ledger = ledger();
    let mut entry = crate::web::store::Entry {
        pending: Pending {
            id: "question".into(),
            pane: "pane".into(),
            session: "session".into(),
            cwd: "/repo".into(),
            created_ms: 2,
            questions: serde_json::from_value(
                json!({"title":"Form","description":"description","extra":{"unknown":[1,true]},"questions":[{"id":"q","type":"text","question":"Why?","media":{"type":"chart","chart":{"data":{"a":[1,2]},"options":{"legend":true}}}}]}),
            )?,
        },
        answers: Some(vec![ResponseItem {
            id: "q".into(),
            value: json!({"custom":["answer",42]}),
            attachments: Some(vec!["/image.png".into(), "/other.png".into()]),
        }]),
        rejection: None,
        answered_ms: Some(4),
    };
    write(&ledger, |tx| questions::put(tx, &entry))?;
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| questions::get(tx, "question"))?)?,
        json!(entry)
    );
    entry.answers = Some(vec![ResponseItem {
        id: "q".into(),
        value: json!(null),
        attachments: None,
    }]);
    write(&ledger, |tx| questions::put(tx, &entry))?;
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| questions::get(tx, "question"))?)?,
        json!(entry)
    );
    entry.answers = None;
    entry.rejection = Some(Rejection {
        reason: "no".into(),
    });
    write(&ledger, |tx| questions::put(tx, &entry))?;
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| questions::get(tx, "question"))?)?,
        json!(entry)
    );
    let mut bad = entry.clone();
    bad.pending.id = "../bad".into();
    assert!(write(&ledger, |tx| questions::put(tx, &bad)).is_err());
    assert!(read(&ledger, |tx| questions::get(tx, "../bad"))?.is_none());
    write(&ledger, |tx| questions::remove(tx, "question"))?;
    assert!(read(&ledger, |tx| questions::get(tx, "question"))?.is_none());
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}

#[test]
fn reviews_revisions_annotations_and_images_round_trip_and_clear_updates() -> Result<()> {
    let ledger = ledger();
    let mut record = crate::web::reviews::Record {
        delivered: true,
        review: Review {
            id: "review".into(),
            pane: "pane".into(),
            session: "session".into(),
            cwd: "/repo".into(),
            path: "plan.md".into(),
            general: "general".into(),
            revisions: vec![
                Revision {
                    n: 1,
                    content: "plan".into(),
                    submitted_ms: 1,
                },
                Revision {
                    n: 2,
                    content: "new plan".into(),
                    submitted_ms: 2,
                },
            ],
            annotations: vec![Annotation {
                id: "annotation".into(),
                revision: 2,
                start_line: 1,
                end_line: 2,
                quote: "quote".into(),
                comment: "comment".into(),
                images: vec!["/one.png".into(), "/two.png".into()],
            }],
            decision: Some(Decision::Approved {
                notes: Some("notes".into()),
            }),
        },
    };
    write(&ledger, |tx| reviews::put(tx, &record))?;
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| reviews::get(tx, "review"))?)?,
        json!(record)
    );
    let mut bad = record.clone();
    bad.review.annotations[0].end_line = 0;
    assert!(write(&ledger, |tx| reviews::put(tx, &bad)).is_err());
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| reviews::get(tx, "review"))?)?,
        json!(record)
    );
    record.review.revisions.remove(0);
    record.review.annotations[0].images.clear();
    record.review.decision = Some(Decision::ChangesRequested {
        message: "changes".into(),
    });
    write(&ledger, |tx| reviews::put(tx, &record))?;
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| reviews::get(tx, "review"))?)?,
        json!(record)
    );
    record.review.annotations.clear();
    record.review.decision = None;
    record.delivered = false;
    write(&ledger, |tx| reviews::put(tx, &record))?;
    assert_eq!(
        serde_json::to_value(read(&ledger, |tx| reviews::get(tx, "review"))?)?,
        json!(record)
    );
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}

#[test]
fn attachment_metadata_round_trips_and_rejects_malformed_input() -> Result<()> {
    let ledger = ledger();
    let mut attachment = reviews::Attachment {
        name: "image.png".into(),
        path: "/image.png".into(),
        media_type: "image/png".into(),
        size: 20,
        created_ms: 1,
    };
    write(&ledger, |tx| reviews::put_attachment(tx, &attachment))?;
    assert_eq!(
        read(&ledger, |tx| reviews::attachment(tx, "image.png"))?,
        Some(attachment.clone())
    );
    attachment.size = 21;
    write(&ledger, |tx| reviews::put_attachment(tx, &attachment))?;
    let mut bad = attachment.clone();
    bad.size = 0;
    assert!(write(&ledger, |tx| reviews::put_attachment(tx, &bad)).is_err());
    assert_eq!(
        read(&ledger, |tx| reviews::attachment(tx, "image.png"))?,
        Some(attachment)
    );
    std::fs::remove_dir_all(ledger.dir)?;
    Ok(())
}
