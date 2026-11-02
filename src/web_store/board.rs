use rusqlite::Transaction;
use rusqlite::types::Value as Sql;
use serde::{Deserialize, Serialize};

use crate::board::{TodoChild, TodoItem, TodoSection, TodoSections};
use crate::error::Result;
use crate::rows::{self, Columns, required, text};
use crate::sidebar::model::Saved;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Dock {
    pub pane_id: String,
    pub terminal_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab_id: Option<String>,
}

pub(crate) fn dock(tx: &Transaction<'_>, tab: &str) -> Result<Option<Dock>> {
    let mut statement = tx.prepare_cached("SELECT * FROM board_docks WHERE tab = ?1")?;
    let mut rows = statement.query([tab])?;
    rows.next()?
        .map(|row| {
            Ok(Dock {
                pane_id: required(row, "pane_id")?,
                terminal_id: required(row, "terminal_id")?,
                tab_id: text(row, "tab_id")?,
            })
        })
        .transpose()
}

pub(crate) fn put_dock(tx: &Transaction<'_>, tab: &str, dock: &Dock) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("tab", Some(tab));
    columns.text("pane_id", Some(&dock.pane_id));
    columns.text("terminal_id", Some(&dock.terminal_id));
    columns.text("tab_id", dock.tab_id.as_deref());
    columns.upsert(tx, "board_docks", "tab")
}

pub(crate) fn remove_dock(tx: &Transaction<'_>, tab: &str) -> Result<()> {
    tx.execute("DELETE FROM board_docks WHERE tab = ?1", [tab])?;
    Ok(())
}

pub(crate) fn sidebar(tx: &Transaction<'_>, owner: &str) -> Result<Option<Saved>> {
    let mut statement = tx.prepare_cached("SELECT * FROM sidebar_state WHERE owner = ?1")?;
    let mut rows = statement.query([owner])?;
    rows.next()?
        .map(|row| {
            Ok(Saved {
                focus: rows::variant(required(row, "focus")?)?,
                collapsed: rows::list(
                    tx,
                    "SELECT panel FROM sidebar_collapsed WHERE owner = ?1 ORDER BY position",
                    owner,
                    |row| row.get(0),
                )?
                .into_iter()
                .map(rows::variant)
                .collect::<Result<_>>()?,
                fold_open: row.get("fold_open")?,
                selected_card: text(row, "selected_card")?,
            })
        })
        .transpose()
}

pub(crate) fn put_sidebar(tx: &Transaction<'_>, owner: &str, saved: &Saved) -> Result<()> {
    let mut columns = Columns::default();
    columns.text("owner", Some(owner));
    columns.text("focus", Some(&rows::name(&saved.focus)?));
    columns.flag("fold_open", saved.fold_open);
    columns.text("selected_card", saved.selected_card.as_deref());
    columns.upsert(tx, "sidebar_state", "owner")?;
    rows::replace_list(
        tx,
        "sidebar_collapsed",
        "owner",
        "panel",
        owner,
        saved
            .collapsed
            .iter()
            .map(|panel| Ok(Sql::Text(rows::name(panel)?)))
            .collect::<Result<Vec<_>>>()?,
    )
}

pub(crate) fn remove_sidebar(tx: &Transaction<'_>, owner: &str) -> Result<()> {
    tx.execute("DELETE FROM sidebar_state WHERE owner = ?1", [owner])?;
    Ok(())
}

pub(crate) fn todo(tx: &Transaction<'_>, lane: &str, pane: &str) -> Result<Option<TodoSections>> {
    let id = format!("{lane}:{pane}");
    let mut statement = tx.prepare_cached("SELECT * FROM todo_snapshots WHERE id = ?1")?;
    let mut rows = statement.query([&id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    let mut sections = Vec::new();
    let mut statement =
        tx.prepare_cached("SELECT * FROM todo_sections WHERE snapshot = ?1 ORDER BY position")?;
    let mut section_rows = statement.query([&id])?;
    while let Some(section) = section_rows.next()? {
        let section_id = required(section, "id")?;
        let mut items = Vec::new();
        let mut statement =
            tx.prepare_cached("SELECT * FROM todo_items WHERE section = ?1 ORDER BY position")?;
        let mut item_rows = statement.query([&section_id])?;
        while let Some(item) = item_rows.next()? {
            let item_id = required(item, "id")?;
            let mut children = Vec::new();
            let mut statement =
                tx.prepare_cached("SELECT * FROM todo_children WHERE item = ?1 ORDER BY position")?;
            let mut child_rows = statement.query([&item_id])?;
            while let Some(child) = child_rows.next()? {
                children.push(TodoChild {
                    status: rows::variant(required(child, "status")?)?,
                    text: required(child, "text")?,
                    description: text(child, "description")?,
                    unit: text(child, "unit")?,
                });
            }
            items.push(TodoItem {
                status: rows::variant(required(item, "status")?)?,
                text: required(item, "text")?,
                description: text(item, "description")?,
                review: item.get("review")?,
                unit: text(item, "unit")?,
                children,
            });
        }
        sections.push(TodoSection {
            status: rows::variant(required(section, "status")?)?,
            count: super::index(section, "count")?,
            items,
        });
    }
    Ok(Some(TodoSections {
        session: required(row, "session")?,
        branch: text(row, "branch")?,
        revision: rows::unsigned(row.get("revision")?)?,
        current: required(row, "current")?,
        pane: required(row, "pane")?,
        sections,
    }))
}

pub(crate) fn todos(tx: &Transaction<'_>, lane: &str) -> Result<Vec<TodoSections>> {
    let panes = rows::list(
        tx,
        "SELECT pane FROM todo_snapshots WHERE lane = ?1 ORDER BY rowid",
        lane,
        |row| row.get::<_, String>(0),
    )?;
    panes
        .into_iter()
        .map(|pane| {
            todo(tx, lane, &pane)?
                .ok_or_else(|| crate::error::Error::Invalid("missing todo snapshot".into()))
        })
        .collect()
}

pub(crate) fn put_todo(tx: &Transaction<'_>, lane: &str, todo: &TodoSections) -> Result<()> {
    crate::board::validate_todo(todo)?;
    let id = format!("{lane}:{}", todo.pane);
    let mut columns = Columns::default();
    columns.text("id", Some(&id));
    columns.text("lane", Some(lane));
    columns.text("pane", Some(&todo.pane));
    columns.text("session", Some(&todo.session));
    columns.text("branch", todo.branch.as_deref());
    columns.number("revision", Some(todo.revision))?;
    columns.text("current", Some(&todo.current));
    columns.upsert(tx, "todo_snapshots", "id")?;
    tx.execute("DELETE FROM todo_sections WHERE snapshot = ?1", [&id])?;
    for (position, section) in todo.sections.iter().enumerate() {
        let section_id = format!("{id}:{position}");
        let mut columns = Columns::default();
        columns.text("id", Some(&section_id));
        columns.text("snapshot", Some(&id));
        columns.number("position", Some(position as u64))?;
        columns.text("status", Some(&rows::name(&section.status)?));
        columns.number("count", Some(section.count as u64))?;
        columns.upsert(tx, "todo_sections", "id")?;
        for (position, item) in section.items.iter().enumerate() {
            let item_id = format!("{section_id}:{position}");
            let mut columns = Columns::default();
            columns.text("id", Some(&item_id));
            columns.text("section", Some(&section_id));
            columns.number("position", Some(position as u64))?;
            columns.text("status", Some(&rows::name(&item.status)?));
            columns.text("text", Some(&item.text));
            columns.text("description", item.description.as_deref());
            columns.flag("review", item.review);
            columns.text("unit", item.unit.as_deref());
            columns.upsert(tx, "todo_items", "id")?;
            for (position, child) in item.children.iter().enumerate() {
                tx.execute("INSERT INTO todo_children(item, position, status, text, description, unit) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![item_id, rows::integer(position as u64)?, rows::name(&child.status)?, child.text, child.description, child.unit])?;
            }
        }
    }
    Ok(())
}

pub(crate) fn remove_todo(tx: &Transaction<'_>, lane: &str, pane: &str) -> Result<()> {
    tx.execute(
        "DELETE FROM todo_snapshots WHERE id = ?1",
        [format!("{lane}:{pane}")],
    )?;
    Ok(())
}

pub(crate) fn cache_todos(tx: &Transaction<'_>, todos: &[TodoSections]) -> Result<()> {
    tx.execute("DELETE FROM todo_snapshots WHERE lane = 'cached'", [])?;
    for todo in todos {
        put_todo(tx, "cached", todo)?;
    }
    Ok(())
}
