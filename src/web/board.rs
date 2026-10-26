//! The board API's view of dogsled's board: the wire types, the grouping by
//! repository, and the JSON payload the API serves.

use crate::board;
use portal_types::{
    BoardView, GroupView, HostView, RepoGroup, RowView, TodoChildView, TodoItemView,
    TodoSectionView, TodoStatusView, TodoView,
};

/// The whole board as the API serves it: the hosts, the rows grouped by
/// repository, and the todos that belong to each group.
pub fn view(board: &board::Board, todos: &[board::TodoSections], generated_ms: u64) -> BoardView {
    BoardView {
        hosts: board.hosts.iter().map(HostView::from).collect(),
        repos: group(board, todos),
        generated_ms,
    }
}

/// Group the rows by repository.
///
/// Named repositories keep the order the board first mentions them. Rows without
/// a repository, and todos whose pane belongs to no row, share one trailing group.
/// A todo belongs to the repository of the row whose pane matches its own.
pub fn group(board: &board::Board, todos: &[board::TodoSections]) -> Vec<RepoGroup> {
    let mut groups: Vec<RepoGroup> = Vec::new();
    let mut named: Vec<(String, usize)> = Vec::new();
    for row in &board.rows {
        if let Some(repository) = &row.repository
            && !named.iter().any(|(name, _)| name == repository)
        {
            named.push((repository.clone(), groups.len()));
            groups.push(RepoGroup {
                repository: Some(repository.clone()),
                rows: Vec::new(),
                todos: Vec::new(),
            });
        }
    }
    let unsorted = if board.rows.iter().any(|row| row.repository.is_none())
        || todos
            .iter()
            .any(|todo| repository_of(board, todo).is_none())
    {
        let index = groups.len();
        groups.push(RepoGroup {
            repository: None,
            rows: Vec::new(),
            todos: Vec::new(),
        });
        Some(index)
    } else {
        None
    };
    for row in &board.rows {
        let index = match &row.repository {
            Some(repository) => index_of(&named, repository),
            None => match unsorted {
                Some(index) => index,
                None => continue,
            },
        };
        groups[index].rows.push(RowView::from(row));
    }
    for todo in todos {
        let index = match repository_of(board, todo) {
            Some(repository) => index_of(&named, &repository),
            None => match unsorted {
                Some(index) => index,
                None => continue,
            },
        };
        groups[index].todos.push(TodoView::from(todo));
    }
    groups
}

/// The repository of the row whose pane matches the todo's own.
fn repository_of(board: &board::Board, todo: &board::TodoSections) -> Option<String> {
    board
        .rows
        .iter()
        .find(|row| row.pane.as_deref() == Some(todo.pane.as_str()))
        .and_then(|row| row.repository.clone())
}

/// The group index of a named repository.
fn index_of(named: &[(String, usize)], repository: &str) -> usize {
    named
        .iter()
        .find(|(name, _)| name == repository)
        .map_or(0, |(_, index)| *index)
}

impl From<board::Group> for GroupView {
    fn from(group: board::Group) -> Self {
        match group {
            board::Group::ToLand => Self::ToLand,
            board::Group::Attention => Self::Attention,
            board::Group::Running => Self::Running,
            board::Group::Waiting => Self::Waiting,
        }
    }
}

impl From<&board::Row> for RowView {
    fn from(row: &board::Row) -> Self {
        Self {
            group: row.group.into(),
            kind: row.kind.clone(),
            id: row.id.clone(),
            name: row.name.clone(),
            what: row.what.clone(),
            host: row.host.clone(),
            pane: row.pane.clone(),
            state: row.state.clone(),
            activity: row.activity.clone(),
            age_ms: row.age_ms,
            quiet_ms: row.quiet_ms,
            ahead: row.ahead,
            dirty: row.dirty,
            repository: row.repository.clone(),
        }
    }
}

impl From<&board::HostLine> for HostView {
    fn from(host: &board::HostLine) -> Self {
        Self {
            label: host.label.clone(),
            service: host.service.clone(),
            reachable: host.reachable,
            busy: host.busy,
            cores: host.cores,
            eligible: host.eligible,
            reason: host.reason.clone(),
            retry_ms: host.retry_ms,
        }
    }
}

impl From<board::TodoStatus> for TodoStatusView {
    fn from(status: board::TodoStatus) -> Self {
        match status {
            board::TodoStatus::InProgress => Self::InProgress,
            board::TodoStatus::Pending => Self::Pending,
            board::TodoStatus::Completed => Self::Completed,
            board::TodoStatus::Abandoned => Self::Abandoned,
        }
    }
}

impl From<&board::TodoChild> for TodoChildView {
    fn from(child: &board::TodoChild) -> Self {
        Self {
            status: child.status.into(),
            text: child.text.clone(),
            description: child.description.clone(),
            unit: child.unit.clone(),
        }
    }
}

impl From<&board::TodoItem> for TodoItemView {
    fn from(item: &board::TodoItem) -> Self {
        Self {
            status: item.status.into(),
            text: item.text.clone(),
            description: item.description.clone(),
            review: item.review,
            unit: item.unit.clone(),
            children: item.children.iter().map(TodoChildView::from).collect(),
        }
    }
}

impl From<&board::TodoSection> for TodoSectionView {
    fn from(section: &board::TodoSection) -> Self {
        Self {
            status: section.status.into(),
            count: section.count,
            items: section.items.iter().map(TodoItemView::from).collect(),
        }
    }
}

impl From<&board::TodoSections> for TodoView {
    fn from(todo: &board::TodoSections) -> Self {
        Self {
            session: todo.session.clone(),
            branch: todo.branch.clone(),
            revision: todo.revision,
            current: todo.current.clone(),
            pane: todo.pane.clone(),
            sections: todo.sections.iter().map(TodoSectionView::from).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, pane: Option<&str>, repository: Option<&str>) -> board::Row {
        board::Row {
            group: board::Group::Running,
            kind: "agent".into(),
            id: format!("r-{name}"),
            name: name.into(),
            what: "pi".into(),
            host: "local".into(),
            pane: pane.map(str::to_owned),
            state: "working".into(),
            activity: "step".into(),
            age_ms: 10,
            quiet_ms: None,
            ahead: None,
            dirty: false,
            repository: repository.map(str::to_owned),
        }
    }

    fn todos(pane: &str) -> board::TodoSections {
        board::TodoSections {
            session: "s".into(),
            branch: None,
            revision: 1,
            current: "c".into(),
            pane: pane.into(),
            sections: vec![board::TodoSection {
                status: board::TodoStatus::Pending,
                count: 0,
                items: Vec::new(),
            }],
        }
    }

    #[test]
    fn rows_group_by_repository_in_board_order() {
        let board = board::Board {
            hosts: Vec::new(),
            rows: vec![
                row("a", Some("pane-a"), Some("host/owner/one")),
                row("b", Some("pane-b"), Some("host/owner/two")),
                row("c", None, None),
            ],
        };
        let groups = group(&board, &[]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].repository.as_deref(), Some("host/owner/one"));
        assert_eq!(groups[1].repository.as_deref(), Some("host/owner/two"));
        assert_eq!(groups[2].repository, None);
        assert_eq!(groups[0].rows[0].name, "a");
        assert_eq!(groups[1].rows[0].name, "b");
        assert_eq!(groups[2].rows[0].name, "c");
    }

    #[test]
    fn the_unsorted_group_trails_the_named_ones() {
        let board = board::Board {
            hosts: Vec::new(),
            rows: vec![row("a", None, None), row("b", None, Some("host/owner/one"))],
        };
        let groups = group(&board, &[]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].repository.as_deref(), Some("host/owner/one"));
        assert_eq!(groups[1].repository, None);
    }

    #[test]
    fn a_todo_belongs_to_the_repository_of_its_pane() {
        let board = board::Board {
            hosts: Vec::new(),
            rows: vec![
                row("a", Some("pane-a"), Some("host/owner/one")),
                row("b", Some("pane-b"), Some("host/owner/two")),
            ],
        };
        let groups = group(&board, &[todos("pane-b"), todos("pane-a")]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].todos[0].pane, "pane-a");
        assert_eq!(groups[1].todos[0].pane, "pane-b");
    }

    #[test]
    fn a_todo_without_a_row_lands_in_the_unsorted_group() {
        let board = board::Board {
            hosts: Vec::new(),
            rows: vec![row("a", Some("pane-a"), Some("host/owner/one"))],
        };
        let groups = group(&board, &[todos("pane-gone")]);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].repository.as_deref(), Some("host/owner/one"));
        assert_eq!(groups[0].todos.len(), 0);
        assert_eq!(groups[1].repository, None);
        assert_eq!(groups[1].todos[0].pane, "pane-gone");
    }

    #[test]
    fn rows_keep_the_board_order_inside_a_group() {
        let board = board::Board {
            hosts: Vec::new(),
            rows: vec![
                row("first", None, Some("host/owner/one")),
                row("second", None, Some("host/owner/one")),
            ],
        };
        let groups = group(&board, &[]);
        assert_eq!(groups.len(), 1);
        let names: Vec<&str> = groups[0].rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["first", "second"]);
    }
}
