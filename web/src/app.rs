//! The page: the board on the left, the selected agent in the centre, and the
//! slots for the question form and the review beside it.

use dioxus::prelude::*;
use portal_types::{BoardView, GroupView};

use crate::agent_view::{AgentView, Draft};
use crate::api;
use crate::board::RepoGroup;
use crate::question_form::QuestionForm;
use crate::review_view::ReviewView;

const MAIN_CSS: Asset = asset!("/assets/main.css");
const REVIEW_CSS: Asset = asset!("/assets/review_view.css");

#[derive(Clone, Default)]
struct DeepLink {
    review: Option<String>,
    question: Option<String>,
    pane: Option<String>,
}

impl DeepLink {
    fn read() -> Self {
        let Some(query) = web_sys::window().and_then(|window| window.location().search().ok())
        else {
            return Self::default();
        };
        let Ok(params) = web_sys::UrlSearchParams::new_with_str(&query) else {
            return Self::default();
        };
        let get = |name| params.get(name).filter(|value| !value.is_empty());
        Self {
            review: get("review"),
            question: get("question"),
            pane: get("pane"),
        }
    }
}

#[component]
pub fn App() -> Element {
    let mut board = use_signal(|| None::<BoardView>);
    let mut error = use_signal(|| None::<String>);
    let link = use_hook(DeepLink::read);
    let selected = use_context_provider(|| Signal::new(link.pane.clone()));
    use_context_provider(|| Draft(Signal::new(String::new())));
    let standalone = link.review.is_some() || link.question.is_some();

    use_future(move || async move {
        match api::fetch_board().await {
            Ok(view) => board.set(Some(view)),
            Err(message) => error.set(Some(message)),
        }
        let result = api::watch(|view| {
            board.set(Some(view));
            error.set(None);
        })
        .await;
        if let Err(message) = result {
            error.set(Some(message));
        }
    });

    let view = board();
    let needs = view.as_ref().map_or(0, |view| {
        view.repos
            .iter()
            .flat_map(|repo| &repo.rows)
            .filter(|row| row.group == GroupView::Attention)
            .count()
    });

    rsx! {
        document::Stylesheet { href: MAIN_CSS }
        document::Stylesheet { href: REVIEW_CSS }
        div { class: if standalone { "app standalone" } else { "app" },
            if !standalone {
            section { class: "board",
                div { class: "head",
                    h1 { "Dogsled" }
                    span { class: if needs > 0 { "needs some" } else { "needs" }, "Needs you: {needs}" }
                }
                if let Some(message) = error() {
                    p { class: "error", "{message}" }
                }
                match view {
                    Some(view) if view.repos.is_empty() => rsx! { p { class: "empty", "Nothing on the board." } },
                    Some(view) => rsx! {
                        for group in view.repos {
                            RepoGroup { key: "{group.repository:?}", group }
                        }
                    },
                    None => rsx! { p { class: "empty", "Loading…" } },
                }
            }
            section { class: "centre",
                match selected() {
                    Some(pane) => rsx! { AgentView { pane } },
                    None => rsx! { p { class: "empty", "Select an agent" } },
                }
            }
            }
            aside { class: "right",
                if let Some(id) = link.review {
                    ReviewView { review_id: id }
                } else if let Some(id) = link.question {
                    QuestionForm { requested: id }
                } else {
                    QuestionForm {}
                    ReviewView {}
                }
            }
        }
    }
}
