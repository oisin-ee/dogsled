//! Guarded read-only metrics, with independently reported peer failures.
use axum::Router;
use axum::extract::{Query, State};
use axum::response::{IntoResponse as _, Response};
use axum::routing::get;
use portal_types::metrics::{HostCoverage, MetricsReport};

use super::AppState;
use crate::error::{Error, Result};
use crate::metrics::Options;

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/api/metrics", get(metrics))
}

async fn metrics(State(state): State<AppState>, Query(options): Query<Options>) -> Response {
    match tokio::task::spawn_blocking(move || fleet(&state, &options)).await {
        Ok(Ok(report)) => ([("cache-control", "no-store")], axum::Json(report)).into_response(),
        Ok(Err(error)) => super::observation_error(&error.to_string()),
        Err(error) => super::observation_error(&format!("metrics task: {error}")),
    }
}

fn fleet(state: &AppState, options: &Options) -> Result<MetricsReport> {
    let mut report = crate::metrics::local(&state.web, options)?;
    let launch = state
        .leads
        .as_ref()
        .ok_or_else(|| Error::Invalid("peer transport unavailable".into()))?;
    let policy = crate::config::load(&launch.config_path)?;
    let fleet = crate::hosts::ActionFleet::current(
        launch.shell.as_ref(),
        &state.web,
        &policy,
        launch.home.clone(),
    )?;
    let args = options.argv()?;
    let replies = std::thread::scope(|scope| {
        let handles: Vec<_> = fleet
            .hosts
            .iter()
            .filter(|host| !host.is_local())
            .map(|host| {
                let fleet = &fleet;
                let args = &args;
                (
                    host,
                    scope.spawn(move || fleet.remote::<MetricsReport>(host, args)),
                )
            })
            .collect();
        handles
            .into_iter()
            .map(|(host, handle)| {
                let result = handle
                    .join()
                    .unwrap_or_else(|_| Err(Error::Invalid("peer metrics task panicked".into())));
                (host.id.clone(), result)
            })
            .collect::<Vec<_>>()
    });
    for (host, result) in replies {
        let result =
            result.and_then(|peer| crate::metrics::report::merge(&mut report, peer, &host));
        if let Err(error) = result {
            report.hosts.push(HostCoverage {
                host,
                transcripts: 0,
                indexed_ms: None,
                error: Some(error.to_string()),
            });
        }
    }
    Ok(report)
}
