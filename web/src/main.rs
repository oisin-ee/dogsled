//! The dogsled web portal: a Dioxus page that shows the board, live from `/api/stream`.

pub mod activity;
mod agent_view;
mod api;
mod app;
mod board;
#[cfg(test)]
mod component_tests;
pub mod components;
mod conversations;
mod files;
mod highlight;
mod metrics;
mod model_picker;
mod new_lead;
mod notice;
mod question_form;
mod review_view;
mod workers;

fn main() {
    // Dioxus's default logger records each INFO span, such as every new signal, as a
    // browser performance mark. The portal logs nothing itself, so keep warnings only.
    // This fails only when a subscriber is already installed, which keeps its own level.
    let _ = dioxus::logger::init(dioxus::logger::tracing::Level::WARN);
    dioxus::launch(app::App);
}

/// Give `node`'s key effect. Dioxus ignores the key of a component's root node and honours a
/// key only among fragment children, so a component that keys its own content returns it as
/// a child: a different key then mounts fresh state instead of keeping the old hooks.
pub(crate) fn keyed(node: dioxus::prelude::Element) -> dioxus::prelude::Element {
    use dioxus::prelude::*;
    rsx! { {node} }
}
