//! The portal's own assets: the files the browser loads. The portal has none
//! yet, so every asset request answers 404.

use axum::http::{StatusCode, Uri};
use axum::response::{IntoResponse, Response};

/// The answer the stub gives for an asset it does not have.
const NO_ASSETS: &str = "no assets yet";

/// Serve one asset.
///
/// The portal ships no assets yet, so the answer is always 404. The stub takes
/// the request URI, because the real handler reads the file it names.
pub async fn serve(_uri: Uri) -> Response {
    (StatusCode::NOT_FOUND, NO_ASSETS).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_asset_the_portal_does_not_have_is_not_found() {
        let response = serve("/index.html".parse().expect("a uri")).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .expect("the body");
        assert_eq!(String::from_utf8_lossy(&body), NO_ASSETS);
    }
}
