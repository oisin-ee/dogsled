//! The portal's reviews: what the owner asks the agents to look at again. The
//! module is a placeholder until the reviews land.

use axum::Router;

use crate::web::AppState;

/// The routes of the reviews API.
pub fn routes() -> Router<AppState> {
    Router::new()
}
