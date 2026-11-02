//! Typed host-local portal, inbox, board, and sidebar state.

pub(crate) mod board;
pub(crate) mod command;
pub(crate) mod inbox;
pub(crate) mod questions;
pub(crate) mod reviews;

#[cfg(test)]
mod tests;

fn index(row: &rusqlite::Row<'_>, column: &str) -> crate::error::Result<usize> {
    let value = crate::rows::unsigned(row.get(column)?)?;
    usize::try_from(value)
        .map_err(|_| crate::error::Error::Invalid(format!("{column} exceeds an index")))
}

pub(crate) const SCHEMA: &str = concat!(
    "
CREATE TABLE inbox_terminals (id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 64)) STRICT;
CREATE TABLE inbox_readers (
    terminal TEXT PRIMARY KEY NOT NULL REFERENCES inbox_terminals(id) ON DELETE CASCADE,
    harness TEXT NOT NULL, pane TEXT NOT NULL, registered_ms INTEGER NOT NULL CHECK(registered_ms >= 0)
) STRICT;
CREATE TABLE inbox_records (
    id TEXT PRIMARY KEY NOT NULL, record_id TEXT NOT NULL CHECK(length(record_id) > 0),
    terminal TEXT NOT NULL REFERENCES inbox_terminals(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), pane TEXT NOT NULL,
    sender_name TEXT, sender_pane TEXT, sender_terminal TEXT, sender_host TEXT NOT NULL,
    kind TEXT NOT NULL, text TEXT NOT NULL CHECK(length(CAST(text AS BLOB)) <= 65536),
    created_ms INTEGER NOT NULL CHECK(created_ms >= 0),
    state TEXT NOT NULL CHECK(state IN ('pending','claimed','delivered','failed')),
    attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
    next_ms INTEGER NOT NULL CHECK(next_ms >= 0), reason TEXT, done_ms INTEGER CHECK(done_ms >= 0),
    UNIQUE(terminal, position)
) STRICT;
CREATE INDEX inbox_records_state ON inbox_records(state, next_ms);
CREATE TABLE inbox_sent (
    seq INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE CHECK(length(id) > 0),
    host TEXT NOT NULL, host_label TEXT NOT NULL, terminal TEXT NOT NULL, pane TEXT NOT NULL,
    name TEXT, harness TEXT, sender_name TEXT, sender_pane TEXT, sender_terminal TEXT, sender_host TEXT NOT NULL,
    kind TEXT NOT NULL, created_ms INTEGER NOT NULL CHECK(created_ms >= 0),
    state TEXT NOT NULL CHECK(state IN ('pending','claimed','delivered','failed')),
    reason TEXT, attempts INTEGER NOT NULL CHECK(attempts BETWEEN 0 AND 4294967295),
    next_ms INTEGER NOT NULL CHECK(next_ms >= 0)
) STRICT;
CREATE INDEX inbox_sent_due ON inbox_sent(state, next_ms);
CREATE TABLE notification_failures (id TEXT PRIMARY KEY NOT NULL CHECK(length(id) > 0), reason TEXT NOT NULL) STRICT;
",
    "CREATE TABLE board_docks (
    tab TEXT PRIMARY KEY NOT NULL CHECK(length(tab) > 0),
    pane_id TEXT NOT NULL CHECK(length(pane_id) > 0), terminal_id TEXT NOT NULL CHECK(length(terminal_id) > 0),
    tab_id TEXT
) STRICT;
CREATE TABLE sidebar_state (
    owner TEXT PRIMARY KEY NOT NULL CHECK(length(owner) > 0),
    focus TEXT NOT NULL CHECK(focus IN ('hosts','agents','todos')),
    fold_open INTEGER NOT NULL CHECK(fold_open IN (0,1)), selected_card TEXT
) STRICT;
CREATE TABLE sidebar_collapsed (
    owner TEXT NOT NULL REFERENCES sidebar_state(owner) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), panel TEXT NOT NULL CHECK(panel IN ('hosts','agents','todos')),
    PRIMARY KEY(owner, position)
) STRICT;
CREATE TABLE todo_snapshots (
    id TEXT PRIMARY KEY NOT NULL, pane TEXT NOT NULL CHECK(length(pane) > 0),
    lane TEXT NOT NULL CHECK(lane IN ('published','cached')),
    session TEXT NOT NULL CHECK(length(session) > 0), branch TEXT,
    revision INTEGER NOT NULL CHECK(revision > 0), current TEXT NOT NULL, UNIQUE(lane, pane)
) STRICT;
CREATE TABLE todo_sections (
    id TEXT PRIMARY KEY NOT NULL, snapshot TEXT NOT NULL REFERENCES todo_snapshots(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position BETWEEN 0 AND 3),
    status TEXT NOT NULL CHECK(status IN ('in_progress','pending','completed','abandoned')),
    count INTEGER NOT NULL CHECK(count >= 0), UNIQUE(snapshot, position)
) STRICT;
CREATE TABLE todo_items (
    id TEXT PRIMARY KEY NOT NULL, section TEXT NOT NULL REFERENCES todo_sections(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0),
    status TEXT NOT NULL CHECK(status IN ('in_progress','pending','completed','abandoned')),
    text TEXT NOT NULL, description TEXT, review INTEGER NOT NULL CHECK(review IN (0,1)), unit TEXT,
    UNIQUE(section, position)
) STRICT;
CREATE TABLE todo_children (
    item TEXT NOT NULL REFERENCES todo_items(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0),
    status TEXT NOT NULL CHECK(status IN ('in_progress','pending','completed','abandoned')),
    text TEXT NOT NULL, description TEXT, unit TEXT, PRIMARY KEY(item, position)
) STRICT;
",
    "CREATE TABLE web_questions (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) BETWEEN 1 AND 128),
    pane TEXT NOT NULL, session TEXT NOT NULL, cwd TEXT NOT NULL,
    created_ms INTEGER NOT NULL CHECK(created_ms >= 0),
    state TEXT NOT NULL CHECK(state IN ('pending','answered','rejected')),
    answered_ms INTEGER CHECK(answered_ms >= 0), rejection_reason TEXT, form_message BLOB NOT NULL,
    CHECK((state = 'pending') = (answered_ms IS NULL)),
    CHECK((state = 'rejected') = (rejection_reason IS NOT NULL))
) STRICT;
CREATE INDEX web_questions_pending ON web_questions(state, created_ms, id);
CREATE TABLE web_answers (
    id TEXT PRIMARY KEY NOT NULL, question TEXT NOT NULL REFERENCES web_questions(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), response_id TEXT NOT NULL, value_message BLOB NOT NULL,
    has_attachments INTEGER NOT NULL CHECK(has_attachments IN (0,1)), UNIQUE(question, position)
) STRICT;
CREATE TABLE web_answer_attachments (
    answer TEXT NOT NULL REFERENCES web_answers(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), path TEXT NOT NULL, PRIMARY KEY(answer, position)
) STRICT;
",
    "CREATE TABLE web_reviews (
    id TEXT PRIMARY KEY NOT NULL CHECK(length(id) > 0), pane TEXT NOT NULL, session TEXT NOT NULL,
    cwd TEXT NOT NULL, path TEXT NOT NULL, general TEXT NOT NULL,
    decision TEXT CHECK(decision IN ('approved','changes_requested')), notes TEXT, message TEXT,
    delivered INTEGER NOT NULL CHECK(delivered IN (0,1)), latest_ms INTEGER NOT NULL CHECK(latest_ms >= 0),
    CHECK(decision = 'approved' OR notes IS NULL),
    CHECK((decision = 'changes_requested') = (message IS NOT NULL))
) STRICT;
CREATE INDEX web_reviews_open ON web_reviews(session, path, latest_ms) WHERE decision IS NULL;
CREATE INDEX web_reviews_pending ON web_reviews(decision, latest_ms);
CREATE INDEX web_reviews_latest ON web_reviews(latest_ms DESC, id);
CREATE INDEX web_reviews_delivery ON web_reviews(session, delivered) WHERE decision IS NOT NULL;
CREATE TABLE web_review_revisions (
    review TEXT NOT NULL REFERENCES web_reviews(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), n INTEGER NOT NULL CHECK(n BETWEEN 1 AND 4294967295),
    content TEXT NOT NULL, submitted_ms INTEGER NOT NULL CHECK(submitted_ms >= 0), PRIMARY KEY(review, position)
) STRICT;
CREATE TABLE web_review_annotations (
    id TEXT PRIMARY KEY NOT NULL, review TEXT NOT NULL REFERENCES web_reviews(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), annotation_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK(revision BETWEEN 1 AND 4294967295),
    start_line INTEGER NOT NULL CHECK(start_line > 0), end_line INTEGER NOT NULL CHECK(end_line >= start_line),
    quote TEXT NOT NULL, comment TEXT NOT NULL, UNIQUE(review, position)
) STRICT;
CREATE TABLE web_annotation_images (
    annotation TEXT NOT NULL REFERENCES web_review_annotations(id) ON DELETE CASCADE,
    position INTEGER NOT NULL CHECK(position >= 0), path TEXT NOT NULL, PRIMARY KEY(annotation, position)
) STRICT;
CREATE TABLE web_attachments (
    name TEXT PRIMARY KEY NOT NULL CHECK(length(name) > 0), path TEXT NOT NULL,
    media_type TEXT NOT NULL CHECK(media_type IN ('image/png','image/jpeg','image/webp')),
    size INTEGER NOT NULL CHECK(size > 0 AND size <= 10485760), created_ms INTEGER NOT NULL CHECK(created_ms >= 0)
) STRICT;
"
);
