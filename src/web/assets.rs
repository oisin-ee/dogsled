//! The portal's own assets: the Dioxus bundle the browser loads.
//!
//! `dx bundle --web --release` writes the bundle to `web/public`
//! (`moon run :web-build`). The server embeds that folder, so the page and
//! the files it names travel with the binary.
//!
//! The bundle is a single-page app. A path that names no file is a route of
//! the app, and the app answers such a path with the page. A path under
//! `/api` is not a route of the app, so an unknown API path stays a 404.

use axum::http::{HeaderValue, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The page of the app. Every route of the app falls back to it.
const INDEX: &str = "index.html";
/// The cache rule for a file whose name carries a content hash.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// The cache rule for a file whose name is fixed, so the browser asks again.
const REVALIDATE: &str = "no-cache";
/// The answer for a path the server does not have.
const NOT_FOUND: &str = "not found";
/// The marker `dx` puts in the name of a file that carries a content hash,
/// such as `web-dxhc0b87d56cd4e880.js`.
const HASH_MARKER: &str = "-dxh";
/// The type of each file the bundle holds, by extension.
const CONTENT_TYPES: [(&str, &str); 18] = [
    ("html", "text/html; charset=utf-8"),
    ("htm", "text/html; charset=utf-8"),
    ("js", "text/javascript; charset=utf-8"),
    ("mjs", "text/javascript; charset=utf-8"),
    ("css", "text/css; charset=utf-8"),
    ("wasm", "application/wasm"),
    ("json", "application/json"),
    ("map", "application/json"),
    ("webmanifest", "application/manifest+json"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("ico", "image/x-icon"),
    ("woff", "font/woff"),
    ("woff2", "font/woff2"),
];
/// The type of a file whose extension the table does not name.
const DEFAULT_TYPE: &str = "application/octet-stream";

/// The embedded bundle.
#[derive(RustEmbed)]
#[folder = "web/public/"]
struct Bundle;

/// Serve one asset, or the page for a route of the app.
pub async fn serve(uri: Uri) -> Response {
    respond(uri.path())
}

/// The answer for a request path, read from the embedded bundle.
fn respond(request_path: &str) -> Response {
    let Some(name) = normalise(request_path) else {
        return not_found();
    };
    let name = if name.is_empty() {
        INDEX
    } else {
        name.as_str()
    };
    if let Some(file) = Bundle::get(name) {
        return file_response(name, &file);
    }
    if is_api(request_path) {
        return not_found();
    }
    match Bundle::get(INDEX) {
        Some(file) => file_response(INDEX, &file),
        None => not_found(),
    }
}

/// The name of a file in the bundle for a request path. `dx` writes a `.`
/// segment into the page (`/./assets/...`), so the answer drops it. A path
/// that climbs out of the bundle names no file, so the answer is nothing.
fn normalise(request_path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in request_path.trim_start_matches('/').split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// Whether a path belongs to the API rather than to the app.
fn is_api(request_path: &str) -> bool {
    request_path == "/api" || request_path.starts_with("/api/")
}

/// One file, with its type and its cache rule.
fn file_response(path: &str, file: &rust_embed::EmbeddedFile) -> Response {
    let mut response = file.data.to_vec().into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(path)),
    );
    let cache = if is_hashed(path) {
        IMMUTABLE
    } else {
        REVALIDATE
    };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    response
}

/// The type of a file, from its extension.
fn content_type(path: &str) -> &'static str {
    let extension = extension(path);
    CONTENT_TYPES
        .iter()
        .find(|(name, _)| *name == extension)
        .map_or(DEFAULT_TYPE, |(_, mime)| *mime)
}

/// The extension of a path, without the dot. An empty string when the path
/// has none.
fn extension(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or_default();
    match name.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() => extension,
        _ => "",
    }
}

/// Whether a file name carries a content hash, so the browser may keep it for
/// a year. `dx` writes such a file under a name that holds the hash of its
/// contents, such as `web-dxhc0b87d56cd4e880.js`. A file without a hash in
/// its name keeps the same name when its contents change, so the browser
/// must ask for it again.
fn is_hashed(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or_default();
    let Some((_, hash)) = name.rsplit_once(HASH_MARKER) else {
        return false;
    };
    let stem = hash.split('.').next().unwrap_or_default();
    stem.len() >= 8 && stem.chars().all(|c| c.is_ascii_hexdigit())
}

/// The 404 answer.
fn not_found() -> Response {
    (StatusCode::NOT_FOUND, NOT_FOUND).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(path: &str) -> Response {
        respond(path)
    }

    fn header_of(response: &Response, name: header::HeaderName) -> String {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    async fn text(response: Response) -> String {
        let body = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap_or_default();
        String::from_utf8_lossy(&body).into_owned()
    }

    /// The first file of the bundle whose name ends with `suffix`.
    fn embedded(suffix: &str) -> Option<String> {
        Bundle::iter()
            .find(|name| name.ends_with(suffix))
            .map(std::borrow::Cow::into_owned)
    }

    /// The path of the file the page loads the app from.
    fn script_of(page: &str) -> String {
        let marker = "src=\"";
        let start = page.find(marker).expect("the page loads a script") + marker.len();
        let end = page[start..].find('"').expect("the path ends") + start;
        page[start..end].to_owned()
    }

    /// The path of the wasm file the app's script loads. The script names the
    /// wasm file twice: once as a file name next to the script, and once as
    /// the path the browser fetches. The path is the one that starts at the
    /// root of the server.
    fn wasm_of(script: &str) -> String {
        let marker = ".wasm";
        let mut offset = 0;
        while let Some(found) = script[offset..].find(marker) {
            let end = offset + found + marker.len();
            let before = &script[..end];
            if let Some(start) = before.rfind('"') {
                let candidate = &before[start + 1..];
                if candidate.starts_with('/') {
                    return candidate.to_owned();
                }
            }
            offset = end;
        }
        panic!("the script loads the wasm file");
    }

    #[tokio::test]
    async fn the_root_serves_the_page() {
        let response = get("/");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            header_of(&response, header::CONTENT_TYPE).starts_with("text/html"),
            "the page is HTML"
        );
        assert_eq!(header_of(&response, header::CACHE_CONTROL), REVALIDATE);
        // The page holds the mount point of the app, so the answer is the
        // bundle's page and not a placeholder.
        assert!(text(response).await.contains("id=\"main\""));
    }

    #[test]
    fn a_wasm_file_is_served_as_wasm() {
        let name = embedded(".wasm").expect("the bundle holds a wasm file");
        let response = get(&format!("/{name}"));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE),
            "application/wasm"
        );
    }

    #[test]
    fn a_hashed_file_is_cached_for_good() {
        let name = embedded(".css").expect("the bundle holds a stylesheet");
        let response = get(&format!("/{name}"));
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            header_of(&response, header::CONTENT_TYPE).starts_with("text/css"),
            "the stylesheet is CSS"
        );
        assert_eq!(header_of(&response, header::CACHE_CONTROL), IMMUTABLE);
    }

    #[tokio::test]
    async fn an_unknown_route_falls_back_to_the_page() {
        let page = text(get("/")).await;
        let response = get("/some/route");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(text(response).await, page);
    }

    #[test]
    fn an_unknown_api_path_is_not_found() {
        assert_eq!(get("/api/nope").status(), StatusCode::NOT_FOUND);
        assert_eq!(get("/api").status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn the_page_loads_the_app_and_its_wasm() {
        let page = text(get("/")).await;
        let script_path = script_of(&page);
        let script = text(get(&script_path)).await;
        let wasm_path = wasm_of(&script);
        // The wasm file of the bundle is served as wasm, so the browser can
        // run the app the page starts.
        let response = get(&wasm_path);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE),
            "application/wasm"
        );
        assert_eq!(header_of(&response, header::CACHE_CONTROL), IMMUTABLE);
    }

    #[test]
    fn a_path_with_a_dot_segment_names_the_same_file() {
        let name = embedded(".css").expect("the bundle holds a stylesheet");
        let plain = get(&format!("/{name}"));
        let dotted = get(&format!("/./{name}"));
        assert_eq!(plain.status(), StatusCode::OK);
        assert_eq!(dotted.status(), StatusCode::OK);
        assert_eq!(
            header_of(&plain, header::CONTENT_TYPE),
            header_of(&dotted, header::CONTENT_TYPE)
        );
    }

    #[test]
    fn a_path_that_climbs_out_of_the_bundle_is_not_found() {
        assert_eq!(get("/../index.html").status(), StatusCode::NOT_FOUND);
        assert_eq!(get("/api/../index.html").status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn the_types_the_bundle_needs_are_known() {
        for (name, mime) in [
            ("a.html", "text/html"),
            ("a.js", "text/javascript"),
            ("a.wasm", "application/wasm"),
            ("a.css", "text/css"),
            ("a.svg", "image/svg+xml"),
            ("a.png", "image/png"),
            ("a.ico", "image/x-icon"),
        ] {
            assert!(content_type(name).starts_with(mime), "{name}");
        }
        assert_eq!(content_type("a.unknown"), DEFAULT_TYPE);
        assert_eq!(content_type("a"), DEFAULT_TYPE);
    }

    #[test]
    fn a_hashed_name_is_a_hashed_name() {
        assert!(is_hashed("assets/web-dxhc0b87d56cd4e880.js"));
        assert!(is_hashed("assets/web_bg-dxh8624d72399d2af3e.wasm"));
        assert!(!is_hashed("index.html"));
        assert!(!is_hashed("assets/favicon.ico"));
        // A file without a hash in its name keeps its name when its contents
        // change, so the browser must ask again.
        assert!(!is_hashed("wasm/web_bg.wasm"));
    }
}
