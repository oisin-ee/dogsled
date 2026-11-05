//! Composition smoke tests for the official controls and console surfaces.

use dioxus::core::DynamicNode;
use dioxus::prelude::*;

use crate::components::{button::Button, input::Input, label::Label, textarea::Textarea};

fn root() -> Element {
    let subject = String::from("Foundation check");
    let message = String::from("Keep the existing portal layout");
    rsx! {
        form {
            Label { html_for: "smoke-subject", "Subject" }
            Input {
                id: "smoke-subject",
                name: "subject",
                r#type: "text",
                value: subject,
                required: true,
            }
            Label { html_for: "smoke-message", "Message" }
            Textarea {
                id: "smoke-message",
                name: "message",
                value: message,
                aria_describedby: "smoke-help",
            }
            p { id: "smoke-help", "Describe the requested change" }
            Button {
                r#type: "submit",
                name: "action",
                value: "send",
                disabled: true,
                aria_label: "Send message",
                "Send"
            }
        }
    }
}

fn composer_root() -> Element {
    let draft = use_signal(String::new);
    let images = use_signal(Vec::<String>::new);
    let uploading = use_signal(|| false);
    rsx! {
        crate::agent_view::Composer { draft, images, uploading, disabled: false, busy: false,
            on_error: |_| {}, on_send: |_| {} }
    }
}

fn attached_composer_root() -> Element {
    let draft = use_signal(String::new);
    let images = use_signal(|| vec!["/tmp/dogsled-attachments/example.png".to_owned()]);
    let uploading = use_signal(|| false);
    rsx! {
        crate::agent_view::Composer { draft, images, uploading, disabled: true, busy: false,
            on_error: |_| {}, on_send: |_| {} }
    }
}

fn empty_inbox_root() -> Element {
    rsx! {
        crate::components::sidebar::SidebarProvider {
            crate::app::InboxSidebar { count: Some(0), loading: false, error: None, items: Vec::new(),
                elapsed_ms: 0, agents: Vec::new(), reachable: true }
        }
    }
}

#[test]
fn composer_has_only_accessible_icon_actions_and_an_unprompted_field() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(composer_root));
    assert!(html.contains(r#"aria-label="Message this agent""#));
    assert!(html.contains(r#"aria-label="Attach images""#));
    assert!(html.contains(r#"aria-label="Send message""#));
    assert!(html.contains(r#"accept="image/png,image/jpeg,image/webp""#));
    assert!(!html.contains("placeholder="));
    let text: String = html
        .split('<')
        .skip(1)
        .filter_map(|element| element.split_once('>').map(|(_, text)| text))
        .collect();
    assert!(text.trim().is_empty(), "unexpected composer text: {text}");
}

#[test]
fn attached_composer_shows_preview_removal_and_disabled_controls() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(attached_composer_root));
    assert!(html.contains(r#"src="/api/attachments/example.png""#));
    assert!(html.contains(r#"aria-label="Remove image 1""#));
    for button in html.split("<button").skip(1) {
        let attributes = button.split('>').next().unwrap_or_default();
        assert!(
            attributes.contains("disabled"),
            "enabled action: {attributes}"
        );
    }
    assert!(!html.contains("placeholder="));
}

#[test]
fn empty_inbox_keeps_its_heading_and_zero_count() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(empty_inbox_root));
    assert!(html.contains("Waiting on you"));
    assert!(html.contains(">0</span>"));
    assert!(html.contains("Nothing is waiting on you."));
    assert!(!html.contains("portal-inbox"));
    assert!(html.contains(r#"data-part="column-empty""#), "{html}");
    assert!(html.contains(r#"role="status""#), "{html}");
    assert!(html.contains(r#"data-slot="item""#), "{html}");
    assert_badge_class(&html, "inbox-count");
}

#[test]
fn message_keeps_text_and_appends_only_attached_references() {
    use crate::agent_view::message_with_images;
    assert_eq!(message_with_images("Review this.", &[]), "Review this.");
    let images = vec!["/tmp/a.png".to_owned(), "/tmp/b.webp".to_owned()];
    assert_eq!(
        message_with_images("Review this.", &images),
        "Review this.\n\nReference images:\n- /tmp/a.png\n- /tmp/b.webp"
    );
    assert_eq!(
        message_with_images("", &images),
        "\n\nReference images:\n- /tmp/a.png\n- /tmp/b.webp"
    );
}

fn review_root(props: (u32, u32, bool)) -> Element {
    use portal_types::reviews::{Annotation, Review, Revision};
    let width = use_signal(|| props.1);
    use_context_provider(|| crate::app::ConsoleWidth(width));
    let initial = Review {
        id: "review-fixture".to_owned(),
        pane: "pane".to_owned(),
        session: "session".to_owned(),
        cwd: "/repo/dogsled".to_owned(),
        path: "plans/fallback.md".to_owned(),
        revisions: (1..=props.0)
            .map(|n| Revision {
                n,
                content: "# Typed **SQLite** state store\n\nKeep the ledger.".to_owned(),
                submitted_ms: 0,
            })
            .collect(),
        annotations: if props.2 {
            vec![Annotation {
                id: "note".to_owned(),
                revision: props.0,
                start_line: 3,
                end_line: 3,
                quote: "Keep the ledger.".to_owned(),
                comment: "Keep it typed.".to_owned(),
                images: Vec::new(),
            }]
        } else {
            Vec::new()
        },
        general: String::new(),
        decision: None,
    };
    rsx! { crate::review_view::ReviewEditor { initial, embedded: true,
    lead: "Move state into typed SQLite with a very long task title", role: "lead",
    repository: "github.com/oisin-ee/dogsled", on_close: |_| {}, on_done: |_| {} } }
}

fn question_root() -> Element {
    use portal_types::questions::{Pending, QuestionSet};
    let questions: QuestionSet = match serde_json::from_value(serde_json::json!({
        "title": "Choose the renderer", "questions": [
            {"id": "ui", "type": "single", "question": "Which renderer?", "recommended": "Dioxus",
             "content": {"source": "Use **Rust** and `WASM`.", "lang": "md"},
             "options": [{"label": "Dioxus", "content": {"source": "Keep **one** language.", "lang": "markdown"}}]},
            {"id": "notes", "type": "text", "question": "Anything else?"}
        ]
    })) {
        Ok(questions) => questions,
        Err(error) => return rsx! { "Invalid question fixture: {error}" },
    };
    let pending = Pending {
        id: "question-fixture".to_owned(),
        pane: "pane".to_owned(),
        session: "session".to_owned(),
        cwd: "/repo/dogsled".to_owned(),
        created_ms: 0,
        questions,
    };
    rsx! { crate::question_form::AskForm { pending, elapsed_ms: 120_000, on_done: |_| {} } }
}

#[test]
fn embedded_review_has_one_header_and_revision_aware_tabs() -> Result<(), Box<dyn std::error::Error>>
{
    for (revisions, width) in [(1, 1440), (2, 1440), (1, 1024), (2, 1024)] {
        let mut dom = VirtualDom::new_with_props(review_root, (revisions, width, false));
        dom.rebuild_in_place();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("Typed SQLite state store"), "{html}");
        assert!(html.contains(r#"class="review-lead-title""#), "{html}");
        assert!(
            html.contains(">Move state into typed SQLite with a very long task title</span>"),
            "{html}"
        );
        assert!(html.contains(">· lead</span>"), "{html}");
        assert!(html.contains(">· dogsled</span>"), "{html}");
        assert!(!html.contains("github.com/oisin-ee/dogsled"), "{html}");
        assert!(
            html.contains(r#"class="review-prose markdown-body""#),
            "{html}"
        );
        assert!(html.contains(r#"aria-label="Close plan review""#), "{html}");
        assert!(html.contains(">Plan</button>"), "{html}");
        assert_eq!(html.contains(">Changes</button>"), revisions > 1, "{html}");
        assert_eq!(html.contains(">Notes 0</button>"), width < 1100, "{html}");
        assert_eq!(html.matches("Approve plan").count(), 1, "{html}");
        if width >= 1100 {
            assert!(
                html.contains("Select text in the plan to add a note."),
                "{html}"
            );
        }
        let field = html
            .split("<textarea")
            .skip(1)
            .find(|field| field.contains("review-general"))
            .ok_or("Missing general note")?;
        let attributes = field
            .split('>')
            .next()
            .ok_or("Missing textarea attributes")?;
        assert!(
            attributes.contains(r#"aria-label="Note for the whole plan""#),
            "{html}"
        );
        assert!(!attributes.contains("placeholder="), "{html}");
    }
    Ok(())
}

#[test]
fn review_notes_use_accessible_icon_controls() {
    let mut dom = VirtualDom::new_with_props(review_root, (1, 1440, true));
    dom.rebuild_in_place();
    let html = dioxus_ssr::render(&dom);
    for expected in [
        "<blockquote>Keep the ledger.</blockquote>",
        "Keep it typed.",
        r#"aria-label="Edit note""#,
        r#"aria-label="Delete note""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
}

#[test]
fn question_form_counts_answers_and_renders_markdown_content() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(question_root));
    for expected in [
        "1 of 2 answered",
        "Send answers",
        "2m",
        "Recommended",
        "Other",
        "<strong>Rust</strong>",
        "<code>WASM</code>",
        "<strong>one</strong>",
        "question-decision",
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
    assert!(!html.contains("**Rust**"), "{html}");
    assert!(!html.contains("**one**"), "{html}");
    assert_badge_class(&html, "recommended-badge");
}

fn badge_root() -> Element {
    rsx! { crate::components::badge::Badge { "Component class baseline" } }
}

fn assert_badge_class(html: &str, part: &str) {
    let baseline = dioxus_ssr::render(&VirtualDom::prebuilt(badge_root));
    let class = baseline
        .split("class=\"")
        .nth(1)
        .and_then(|value| value.split('"').next());
    let tag = html.split('<').find(|tag| {
        tag.starts_with("span ")
            && tag
                .split('>')
                .next()
                .is_some_and(|attributes| attributes.contains(&format!("data-part=\"{part}\"")))
    });
    assert!(class.is_some_and(|class| tag.is_some_and(|tag| tag.contains(&format!("class=\"{class}\"")))), "missing official badge class for {part}: {html}");
}

fn todos_root() -> Element {
    use portal_types::{TodoChildView, TodoItemView, TodoSectionView, TodoStatusView, TodoView};
    let item = |status, text: &str| TodoItemView {
        status,
        text: text.to_owned(),
        description: Some(format!("{text} description")),
        review: false,
        unit: Some("worker".to_owned()),
        children: vec![TodoChildView {
            status,
            text: format!("{text} child"),
            description: Some(format!("{text} child description")),
            unit: Some("worker".to_owned()),
        }],
    };
    let todos = vec![TodoView {
        session: "session".to_owned(),
        branch: None,
        revision: 1,
        current: "Work".to_owned(),
        pane: "pane".to_owned(),
        sections: vec![
            TodoSectionView {
                status: TodoStatusView::Pending,
                count: 0,
                items: vec![],
            },
            TodoSectionView {
                status: TodoStatusView::InProgress,
                count: 1,
                items: vec![item(TodoStatusView::InProgress, "Active")],
            },
            TodoSectionView {
                status: TodoStatusView::Completed,
                count: 1,
                items: vec![item(TodoStatusView::Completed, "Done")],
            },
            TodoSectionView {
                status: TodoStatusView::Abandoned,
                count: 1,
                items: vec![item(TodoStatusView::Abandoned, "Dropped")],
            },
        ],
    }];
    rsx! { crate::agent_view::Todos { todos, workers: vec![("worker".to_owned(), "Assigned worker".to_owned())] } }
}

#[test]
fn todos_count_all_items_and_hide_empty_sections() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(todos_root));
    assert!(html.contains(">1 of 3</span>"), "{html}");
    assert!(!html.contains("Pending · 0"), "{html}");
    // AccordionContent mounts its body in the browser; SSR covers the section triggers.
    for text in ["In progress · 1", "Completed · 1", "Abandoned · 1"] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
}

fn empty_todos_root() -> Element {
    rsx! { crate::agent_view::Todos { todos: Vec::new(), workers: Vec::new() } }
}

#[test]
fn empty_todos_use_muted_column_copy_without_sections() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(empty_todos_root));
    assert!(html.contains(">0 of 0</span>"), "{html}");
    assert!(
        html.contains("No todos are recorded for this agent."),
        "{html}"
    );
    assert!(html.contains(r#"data-part="column-empty""#), "{html}");
    assert!(html.contains(r#"role="status""#), "{html}");
    assert!(html.contains(r#"data-slot="item""#), "{html}");
    assert!(!html.contains("Pending"), "{html}");
}

fn history_turn(
    id: &str,
    role: portal_types::agents::Role,
    text: &str,
) -> portal_types::agents::Turn {
    use portal_types::agents::{Source, Turn};
    Turn {
        id: id.into(),
        role,
        text: text.into(),
        source: Source::Harness {
            harness: "pi".into(),
            model: None,
            provider: None,
        },
        timestamp: None,
        tool: None,
    }
}

fn history(turns: Vec<portal_types::agents::Turn>) -> portal_types::agents::Conversation {
    portal_types::agents::Conversation {
        pane: "pane".into(),
        session: "session".into(),
        turns,
        truncated: false,
    }
}

fn timeline_root(snapshot: portal_types::agents::Conversation) -> Element {
    rsx! { crate::agent_view::Conversation { snapshot: Some(snapshot), loading: false, assistant: "Selected assistant" } }
}

fn render_timeline(snapshot: portal_types::agents::Conversation) -> String {
    let mut dom = VirtualDom::new_with_props(timeline_root, snapshot);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

#[test]
fn timeline_attributes_native_input_assistant_deliveries_and_extension_context() {
    use portal_types::agents::{Role, Source};
    let owner = history_turn("owner", Role::User, "Owner input.");
    let mut assistant = history_turn("reply", Role::Assistant, "**Actual** reply.");
    assistant.source = Source::Harness {
        harness: "pi".into(),
        model: Some("recorded-model".into()),
        provider: Some("recorded-provider".into()),
    };
    // Even if a source arrives with a user role, attribution must not turn it into You.
    let mut delivery = history_turn("delivery", Role::User, "Worker report.");
    delivery.source = Source::Dogsled {
        sender: "Layout reviewer with a very long name".into(),
        delivery_kind: "task".into(),
    };
    let mut context = history_turn("context", Role::User, "Gate context.");
    context.source = Source::Custom {
        custom_type: "dogsled-gate".into(),
    };
    let html = render_timeline(history(vec![owner, assistant, delivery, context]));
    for text in [
        ">You</strong>",
        ">Selected assistant</strong>",
        "pi · recorded-model · recorded-provider",
        ">Layout reviewer with a very long name</strong>",
        "Dogsled · task",
        ">dogsled-gate</strong>",
        "Extension context",
        "<strong>Actual</strong>",
    ] {
        assert!(html.contains(text), "missing {text}: {html}");
    }
    assert_eq!(
        html.matches("conversation-event conversation-owner")
            .count(),
        1,
        "{html}"
    );
    assert_eq!(html.matches(r#"aria-label="You""#).count(), 1, "{html}");
}

#[test]
fn missing_metadata_stays_missing_and_markdown_keeps_scrollable_code_and_tables() {
    use portal_types::agents::Role;
    let html = render_timeline(history(vec![history_turn(
        "missing",
        Role::Assistant,
        "```rust\nlet answer = 42;\n```\n\n| Column |\n| --- |\n| Value |",
    )]));
    assert!(html.contains(">Selected assistant</strong>"), "{html}");
    assert!(html.contains(">pi</span>"), "{html}");
    assert!(html.contains(r#"aria-label="Time unavailable""#), "{html}");
    assert!(html.contains("—"), "{html}");
    assert!(!html.contains("datetime="), "{html}");
    assert!(!html.contains("Unknown model"), "{html}");
    assert!(!html.contains("recorded-model"), "{html}");
    assert!(
        html.contains("<pre ") && html.contains("<code class=\"language-rust\""),
        "{html}"
    );
    assert!(html.contains("<table "), "{html}");
    assert!(html.contains("markdown-body"), "{html}");
}

#[test]
fn timestamp_semantics_preserve_the_platform_boundary_and_do_not_claim_response_completion() {
    use portal_types::agents::{Role, Source, TimeMeaning, Timestamp};
    let cases = [
        (
            "request",
            Role::Assistant,
            TimeMeaning::Message,
            "Request started (not response completion)",
        ),
        (
            "message",
            Role::User,
            TimeMeaning::Message,
            "Harness message time",
        ),
        (
            "recorded",
            Role::Context,
            TimeMeaning::Recorded,
            "Transcript entry recorded",
        ),
        (
            "created",
            Role::Context,
            TimeMeaning::Created,
            "Dogsled delivery created",
        ),
    ];
    for (id, role, meaning, expected) in cases {
        let mut turn = history_turn(id, role, "Recorded text.");
        turn.timestamp = Some(Timestamp {
            ms: 1_799_999_999_123,
            meaning,
        });
        let html = render_timeline(history(vec![turn]));
        assert!(html.contains("<time "), "{html}");
        assert!(
            html.contains(r#"data-timestamp-ms="1799999999123""#),
            "{html}"
        );
        assert!(html.contains(expected), "{html}");
        // Native SSR must not invent a calendar or a test timezone for a browser-only UI.
        assert!(!html.contains("datetime="), "{html}");
        assert!(
            html.contains("date and time formatting unavailable"),
            "{html}"
        );
    }
    let mut other = history_turn("other-harness", Role::Assistant, "Reply.");
    other.source = Source::Harness {
        harness: "claude".into(),
        model: None,
        provider: None,
    };
    other.timestamp = Some(Timestamp {
        ms: 1000,
        meaning: TimeMeaning::Message,
    });
    let html = render_timeline(history(vec![other]));
    assert!(html.contains("Harness message time"), "{html}");
    assert!(!html.contains("Request started"), "{html}");
}

fn tool_turn(
    id: &str,
    name: Option<&str>,
    status: portal_types::agents::ToolStatus,
) -> portal_types::agents::Turn {
    use portal_types::agents::{Role, ToolSummary};
    let mut turn = history_turn(id, Role::Tool, "Do not render raw tool text or reasoning.");
    turn.tool = Some(ToolSummary {
        call_id: format!("call-{id}"),
        name: name.map(str::to_owned),
        status,
    });
    turn
}

#[test]
fn adjacent_tools_use_collapsed_official_disclosures_and_recorded_outcomes() {
    use portal_types::agents::{Role, ToolStatus};
    let html = render_timeline(history(vec![
        tool_turn("read", Some("read"), ToolStatus::Completed),
        tool_turn("bash", Some("bash"), ToolStatus::Error),
        tool_turn("unknown", None, ToolStatus::Unknown),
        history_turn("reply", Role::Assistant, "Separate reply."),
        tool_turn("search", Some("search"), ToolStatus::Unknown),
    ]));
    assert_eq!(
        html.matches("conversation-event conversation-tools")
            .count(),
        2,
        "{html}"
    );
    assert_eq!(html.matches("aria-expanded=false").count(), 2, "{html}");
    for expected in [
        "3 tool records · read, bash, Unnamed tool",
        "1 tool records · search",
        "Result recorded",
        "Error recorded",
        "No recorded outcome",
        r#"data-part="tool-records""#,
        r#"data-record-id="unknown""#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
    assert!(!html.contains("Do not render raw tool text"), "{html}");
    assert!(!html.contains("Running"), "{html}");
    assert!(!html.contains("Success"), "{html}");
    assert!(!html.contains("<details"), "{html}");
    assert!(html.contains("data-open=false"), "{html}");
}

#[test]
fn truncated_history_never_claims_a_complete_or_empty_start() {
    let mut snapshot = history(Vec::new());
    snapshot.truncated = true;
    let html = render_timeline(snapshot);
    assert!(html.contains("Earlier history was not returned."), "{html}");
    assert!(!html.contains("No conversation yet"), "{html}");
    let html = render_timeline(history(Vec::new()));
    assert!(html.contains("No conversation yet"), "{html}");
}

fn pending_history_root() -> Element {
    rsx! { crate::agent_view::Conversation { snapshot: None, loading: true, assistant: "Assistant" } }
}

#[test]
fn loading_history_does_not_wait_for_agent_metadata_or_claim_an_empty_conversation() {
    let html = dioxus_ssr::render(&VirtualDom::prebuilt(pending_history_root));
    assert!(
        html.contains(r#"aria-label="Loading conversation""#),
        "{html}"
    );
    assert!(!html.contains("No conversation yet"), "{html}");
}

type HistoryHandle =
    std::rc::Rc<std::cell::Cell<Option<Signal<portal_types::agents::Conversation>>>>;

fn changing_history_root(props: (portal_types::agents::Conversation, HistoryHandle)) -> Element {
    let snapshot = use_signal(|| props.0);
    props.1.set(Some(snapshot));
    rsx! { crate::agent_view::Conversation { snapshot: Some(snapshot()), loading: false, assistant: "Assistant" } }
}

fn record_scopes(
    dom: &VirtualDom,
    node: &VNode,
    records: &mut std::collections::BTreeMap<String, ScopeId>,
) {
    for (index, dynamic) in node.dynamic_nodes.iter().enumerate() {
        match dynamic {
            DynamicNode::Component(component) => {
                if let Some(key) = &node.key
                    && (component.name.ends_with("::MessageRecord")
                        || component.name.ends_with("::ToolRecords"))
                    && let Some(scope) = component.mounted_scope_id(index, node, dom)
                {
                    records.insert(key.clone(), scope);
                }
                if let Some(scope) = component.mounted_scope(index, node, dom) {
                    record_scopes(dom, scope.root_node(), records);
                }
            }
            DynamicNode::Fragment(children) => {
                for child in children {
                    record_scopes(dom, child, records);
                }
            }
            _ => {}
        }
    }
}

#[test]
fn stable_native_and_offset_keys_preserve_mounted_records_across_history_changes()
-> Result<(), Box<dyn std::error::Error>> {
    use portal_types::agents::{Role, ToolStatus};
    let handle = HistoryHandle::default();
    let initial = history(vec![
        history_turn("native-reply", Role::Assistant, "Kept reply."),
        tool_turn(
            "pi:offset:7:tool:call-id",
            Some("read"),
            ToolStatus::Unknown,
        ),
        history_turn("pi:offset:8", Role::User, "Kept input."),
    ]);
    let mut dom = VirtualDom::new_with_props(changing_history_root, (initial, handle.clone()));
    dom.rebuild_in_place();
    let mut before = std::collections::BTreeMap::new();
    record_scopes(&dom, dom.base_scope().root_node(), &mut before);
    assert_eq!(before.len(), 3, "keys were not mounted: {before:?}");
    assert!(before.contains_key("native-reply"));
    assert!(before.contains_key("pi:offset:7:tool:call-id"));
    assert!(before.contains_key("pi:offset:8"));
    let mut signal = handle.get().ok_or("Missing history signal")?;
    dom.in_runtime(|| {
        let mut snapshot = signal.write();
        snapshot
            .turns
            .insert(0, history_turn("earlier", Role::User, "Earlier message."));
        if let Some(tool) = snapshot
            .turns
            .get_mut(2)
            .and_then(|turn| turn.tool.as_mut())
        {
            tool.status = ToolStatus::Completed;
        }
    });
    dom.render_immediate_to_vec();
    let mut after = std::collections::BTreeMap::new();
    record_scopes(&dom, dom.base_scope().root_node(), &mut after);
    for (key, scope) in before {
        assert_eq!(after.get(&key), Some(&scope), "remounted {key}");
    }
    assert!(dioxus_ssr::render(&dom).contains("Result recorded"));
    Ok(())
}

fn activity_root(props: (crate::activity::Live, String, bool, Option<String>)) -> Element {
    rsx! { crate::agent_view::ActivityRow { live: props.0, coarse: props.1, connected: props.2, waiting: props.3 } }
}

fn render_activity(
    live: crate::activity::Live,
    coarse: &str,
    connected: bool,
    waiting: Option<&str>,
) -> String {
    let mut dom = VirtualDom::new_with_props(
        activity_root,
        (live, coarse.into(), connected, waiting.map(str::to_owned)),
    );
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

#[test]
fn every_live_phase_uses_only_actual_detail_and_names_all_active_tools() {
    use crate::activity::Live;
    use portal_types::agents::{ActiveTool, LivePhase, LiveState};
    for (phase, expected) in [
        (LivePhase::Idle, "Idle"),
        (LivePhase::Working, "Working…"),
        (LivePhase::Thinking, "Thinking…"),
        (LivePhase::Responding, "Responding…"),
        (LivePhase::ToolRunning, "Running tools: read, bash"),
    ] {
        let html = render_activity(
            Live::Detail(LiveState {
                phase,
                active_tools: vec![
                    ActiveTool {
                        call_id: "read".into(),
                        name: "read".into(),
                        parent_call_id: None,
                    },
                    ActiveTool {
                        call_id: "bash".into(),
                        name: "bash".into(),
                        parent_call_id: Some("read".into()),
                    },
                ],
            }),
            "working",
            true,
            None,
        );
        assert!(html.contains(expected), "{html}");
        assert!(html.contains(">Live</p>"), "{html}");
        assert_eq!(html.matches(r#"role="status""#).count(), 1, "{html}");
        assert!(html.contains(r#"aria-live="polite""#), "{html}");
        assert!(html.contains(r#"aria-atomic="true""#), "{html}");
    }
}

#[test]
fn blocked_pending_requests_and_agent_disconnection_take_priority_over_live_detail() {
    use crate::activity::Live;
    use portal_types::agents::{LivePhase, LiveState};
    let live = Live::Detail(LiveState {
        phase: LivePhase::Thinking,
        active_tools: Vec::new(),
    });
    for (coarse, connected, waiting, expected) in [
        (
            "working",
            true,
            Some("Waiting for your answers"),
            "Waiting for your answers",
        ),
        ("blocked", true, None, "Blocked"),
        ("working", false, None, "Disconnected"),
        ("disconnected", true, None, "Disconnected"),
    ] {
        let html = render_activity(live.clone(), coarse, connected, waiting);
        assert!(html.contains(expected), "{html}");
        assert!(!html.contains("Thinking"), "{html}");
        assert!(!html.contains(">Live</p>"), "{html}");
    }
}

#[test]
fn every_gap_and_pending_falls_back_to_coarse_state_without_claiming_completion() {
    use crate::activity::{Gap, Live};
    use portal_types::agents::UnavailableReason;
    let mut values = vec![
        Live::Pending,
        Live::Unavailable(Gap::Disconnected),
        Live::Unavailable(Gap::SessionMismatch),
        Live::Unavailable(Gap::Invalid),
    ];
    values.extend(
        [
            UnavailableReason::UnsupportedHarness,
            UnavailableReason::SessionUnverified,
            UnavailableReason::NativeUnavailable,
            UnavailableReason::NoProducer,
            UnavailableReason::NoSnapshot,
            UnavailableReason::Expired,
            UnavailableReason::ProducerClosed,
            UnavailableReason::Resyncing,
            UnavailableReason::Overflow,
        ]
        .into_iter()
        .map(|reason| Live::Unavailable(Gap::Server(reason))),
    );
    for live in values {
        for (coarse, expected) in [
            ("working", "Working…"),
            ("blocked", "Blocked"),
            ("idle", "Idle"),
            ("disconnected", "Disconnected"),
            ("unknown", "unknown"),
        ] {
            let html = render_activity(live.clone(), coarse, true, None);
            assert!(html.contains(expected), "{live:?}: {html}");
            assert!(!html.contains("Thinking"), "{html}");
            assert!(!html.contains("Responding"), "{html}");
            assert!(!html.contains("Running tools"), "{html}");
            assert!(!html.contains(">Live</p>"), "{html}");
            assert!(!html.contains("Finished"), "{html}");
            if coarse == "working" {
                assert!(!html.contains(">Idle</p>"), "{html}");
            }
        }
    }
}

#[test]
fn telemetry_loss_and_session_mismatch_remove_detail_but_keep_the_history_and_working_state()
-> Result<(), Box<dyn std::error::Error>> {
    use crate::activity::{Gap, Live};
    use portal_types::agents::{LivePhase, LiveState, Role};
    type LiveHandle = std::rc::Rc<std::cell::Cell<Option<Signal<Live>>>>;
    fn live_root(handle: LiveHandle) -> Element {
        let live = use_signal(|| {
            Live::Detail(LiveState {
                phase: LivePhase::Responding,
                active_tools: Vec::new(),
            })
        });
        handle.set(Some(live));
        rsx! {
            crate::agent_view::Conversation { snapshot: Some(history(vec![history_turn("kept", Role::Assistant, "Durable history.")])), loading: false, assistant: "Assistant" }
            crate::agent_view::ActivityRow { live: live(), coarse: "working", connected: true, waiting: None }
        }
    }
    let handle = LiveHandle::default();
    let mut dom = VirtualDom::new_with_props(live_root, handle.clone());
    dom.rebuild_in_place();
    let mut initial = std::collections::BTreeMap::new();
    record_scopes(&dom, dom.base_scope().root_node(), &mut initial);
    assert!(dioxus_ssr::render(&dom).contains("Responding…"));
    let mut signal = handle.get().ok_or("Missing activity signal")?;
    for gap in [
        Gap::Disconnected,
        Gap::SessionMismatch,
        Gap::Server(portal_types::agents::UnavailableReason::Expired),
    ] {
        dom.in_runtime(|| signal.set(Live::Unavailable(gap)));
        dom.render_immediate_to_vec();
        let html = dioxus_ssr::render(&dom);
        assert!(html.contains("Durable history."), "{html}");
        assert!(html.contains("Working…"), "{html}");
        assert!(!html.contains("Responding…"), "{html}");
        assert!(!html.contains(">Idle</p>"), "{html}");
        let mut current = std::collections::BTreeMap::new();
        record_scopes(&dom, dom.base_scope().root_node(), &mut current);
        assert_eq!(initial, current);
    }
    Ok(())
}

#[test]
fn official_controls_forward_labels_values_and_attributes() {
    let dom = VirtualDom::prebuilt(root);
    let html = dioxus_ssr::render(&dom);

    for expected in [
        "<form>",
        "<label",
        r#"for="smoke-subject""#,
        ">Subject</label>",
        "<input",
        r#"id="smoke-subject""#,
        r#"name="subject""#,
        r#"type="text""#,
        r#"value="Foundation check""#,
        "required",
        r#"for="smoke-message""#,
        ">Message</label>",
        "<textarea",
        r#"id="smoke-message""#,
        r#"name="message""#,
        r#"value="Keep the existing portal layout""#,
        r#"aria-describedby="smoke-help""#,
        "<button",
        r#"type="submit""#,
        r#"name="action""#,
        r#"value="send""#,
        "disabled",
        r#"aria-label="Send message""#,
        ">Send</button>",
    ] {
        assert!(html.contains(expected), "missing {expected:?} in {html}");
    }
}
