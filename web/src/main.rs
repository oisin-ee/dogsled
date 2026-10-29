//! The dogsled web portal: a Dioxus page that shows the board, live from `/api/stream`.

mod agent_view;
mod api;
mod app;
mod board;
#[cfg(test)]
mod component_tests;
pub mod components;
mod conversations;
mod question_form;
mod review_view;

fn main() {
    dioxus::launch(app::App);
}
