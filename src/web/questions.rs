//! The portal's questions: what the owner asks the agents. The module is a
//! placeholder until the questions land.

use axum::Router;

use crate::web::AppState;

/// The routes of the questions API.
pub fn routes() -> Router<AppState> {
    Router::new()
}
