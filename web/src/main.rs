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
mod new_lead;
mod notice;
mod question_form;
mod review_view;
mod workers;

fn main() {
    dioxus::launch(app::App);
}
