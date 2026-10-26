//! The guards that keep the web API local.
//!
//! The server binds to `127.0.0.1`, but a browser on the same machine can
//! still reach it from a page on another origin. Two headers decide whether a
//! request may come in:
//!
//! - `Host` must name the loopback address and the port the server listens on.
//!   A name that resolves elsewhere is a DNS rebinding attempt.
//! - `Origin` must name the same origin for a request that changes state. A
//!   missing `Origin` on such a request is a cross-site form post.

use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};

/// The origins that may change state on the portal: the loopback address and
/// its name, on the port the server listens on.
pub fn origins(port: u16) -> Vec<String> {
    vec![
        format!("http://127.0.0.1:{port}"),
        format!("http://localhost:{port}"),
    ]
}

/// Whether the `Host` header names the loopback address and the served port.
#[must_use]
pub fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host.trim();
    let Some((name, served)) = host.rsplit_once(':') else {
        return false;
    };
    if served != port.to_string() {
        return false;
    }
    matches!(name, "127.0.0.1" | "localhost")
}

/// Whether a request may change state.
///
/// A `GET`, `HEAD` or `OPTIONS` request changes nothing. Every other method
/// needs an `Origin` that names the portal itself.
#[must_use]
pub fn origin_allowed(method: &Method, origin: Option<&str>, port: u16) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    let Some(origin) = origin else {
        return false;
    };
    origins(port).iter().any(|allowed| allowed == origin)
}

/// The `Host` header of a request, if it has one.
fn host_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(axum::http::header::HOST)?.to_str().ok()
}

/// The `Origin` header of a request, if it has one.
fn origin_header(headers: &HeaderMap) -> Option<&str> {
    headers.get(axum::http::header::ORIGIN)?.to_str().ok()
}

/// Reject a request whose `Host` names another host, or whose `Origin` names
/// another origin while changing state.
pub fn check(method: &Method, headers: &HeaderMap, port: u16) -> Option<Response> {
    if !host_allowed(host_header(headers), port) {
        return Some(StatusCode::FORBIDDEN.into_response());
    }
    if !origin_allowed(method, origin_header(headers), port) {
        return Some(StatusCode::FORBIDDEN.into_response());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_loopback_host_and_its_name_are_allowed() {
        assert!(host_allowed(Some("127.0.0.1:7465"), 7465));
        assert!(host_allowed(Some("localhost:7465"), 7465));
    }

    #[test]
    fn another_host_or_port_is_not_allowed() {
        assert!(!host_allowed(Some("evil.example"), 7465));
        assert!(!host_allowed(Some("evil.example:7465"), 7465));
        assert!(!host_allowed(Some("127.0.0.1:8080"), 7465));
        assert!(!host_allowed(Some("127.0.0.1"), 7465));
        assert!(!host_allowed(Some("[::1]:7465"), 7465));
        assert!(!host_allowed(None, 7465));
    }

    #[test]
    fn a_read_needs_no_origin() {
        assert!(origin_allowed(&Method::GET, None, 7465));
        assert!(origin_allowed(&Method::HEAD, None, 7465));
        assert!(origin_allowed(&Method::OPTIONS, None, 7465));
    }

    #[test]
    fn a_write_without_the_portal_origin_is_refused() {
        assert!(!origin_allowed(&Method::POST, None, 7465));
        assert!(!origin_allowed(&Method::PUT, None, 7465));
        assert!(!origin_allowed(&Method::DELETE, None, 7465));
    }

    #[test]
    fn a_write_needs_the_portal_origin() {
        assert!(origin_allowed(
            &Method::POST,
            Some("http://127.0.0.1:7465"),
            7465
        ));
        assert!(origin_allowed(
            &Method::POST,
            Some("http://localhost:7465"),
            7465
        ));
    }

    #[test]
    fn another_origin_is_refused() {
        assert!(!origin_allowed(
            &Method::POST,
            Some("http://evil.example"),
            7465
        ));
        assert!(!origin_allowed(
            &Method::POST,
            Some("http://127.0.0.1:8080"),
            7465
        ));
        assert!(!origin_allowed(&Method::POST, Some("null"), 7465));
    }
}
