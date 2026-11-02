//! Composition smoke tests for the official controls and console surfaces.

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
    lead: "Move state · lead · dogsled", on_close: |_| {}, on_done: |_| {} } }
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
