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
//!
//! A request that carries the portal's token as a `Bearer` credential passes
//! the `Origin` rule. A browser cannot add that header to a cross-site
//! request, so the token shows the request comes from a local client. The
//! `Host` rule still applies: the token does not let a request name another
//! host.

use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// The portal's own token.
///
/// The token is a secret, so its `Debug` shows that a token is there and
/// nothing more. A log line that prints the state cannot leak it.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// Wrap a token value.
    pub fn new(token: String) -> Self {
        Self(token)
    }

    /// The token's value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Token {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Token([redacted])")
    }
}

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

/// The token of an `Authorization: Bearer <token>` header, if the request has
/// a non-empty one.
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// Whether the request carries the portal's token as a `Bearer` credential.
///
/// Both sides are hashed first, so the comparison takes the same time for any
/// input. An empty portal token never matches.
#[must_use]
pub fn bearer_allowed(headers: &HeaderMap, token: &Token) -> bool {
    if token.as_str().is_empty() {
        return false;
    }
    let Some(given) = bearer(headers) else {
        return false;
    };
    let given = Sha256::digest(given.as_bytes());
    let expected = Sha256::digest(token.as_str().as_bytes());
    given
        .iter()
        .zip(expected.iter())
        .fold(0_u8, |difference, (a, b)| difference | (a ^ b))
        == 0
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
/// another origin while changing state. A request with the portal's token
/// passes the `Origin` rule.
pub fn check(method: &Method, headers: &HeaderMap, port: u16, token: &Token) -> Option<Response> {
    if !host_allowed(host_header(headers), port) {
        return Some(StatusCode::FORBIDDEN.into_response());
    }
    if !origin_allowed(method, origin_header(headers), port) && !bearer_allowed(headers, token) {
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

    /// Headers with one `Authorization` value.
    fn bearer_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            axum::http::header::AUTHORIZATION,
            value.parse().expect("a header value"),
        );
        headers
    }

    /// The portal's token, in the form the guard takes.
    fn token(value: &str) -> Token {
        Token::new(value.to_owned())
    }

    #[test]
    fn the_token_passes_the_origin_rule() {
        let headers = bearer_headers("Bearer secret");
        assert!(bearer_allowed(&headers, &token("secret")));
    }

    #[test]
    fn a_wrong_or_malformed_token_gets_no_bypass() {
        for value in [
            "Bearer other",
            "Bearer ",
            "Bearer",
            "Basic secret",
            "secret",
        ] {
            let headers = bearer_headers(value);
            assert!(!bearer_allowed(&headers, &token("secret")), "{value}");
        }
    }

    #[test]
    fn a_missing_token_gets_no_bypass() {
        assert!(!bearer_allowed(&HeaderMap::new(), &token("secret")));
    }

    #[test]
    fn an_empty_portal_token_never_matches() {
        assert!(!bearer_allowed(
            &bearer_headers("Bearer secret"),
            &token("")
        ));
        assert!(!bearer_allowed(&HeaderMap::new(), &token("")));
    }

    #[test]
    fn the_token_is_not_shown_in_its_debug_form() {
        assert_eq!(format!("{:?}", token("secret")), "Token([redacted])");
        assert!(!format!("{:?}", token("secret")).contains("secret"));
    }

    #[test]
    fn a_write_with_the_token_and_no_origin_is_allowed() {
        let mut headers = bearer_headers("Bearer secret");
        headers.insert(
            axum::http::header::HOST,
            "127.0.0.1:7465".parse().expect("a host"),
        );
        assert!(check(&Method::POST, &headers, 7465, &token("secret")).is_none());
    }

    #[test]
    fn a_write_with_a_wrong_token_is_refused() {
        let mut headers = bearer_headers("Bearer wrong");
        headers.insert(
            axum::http::header::HOST,
            "127.0.0.1:7465".parse().expect("a host"),
        );
        assert!(check(&Method::POST, &headers, 7465, &token("secret")).is_some());
    }

    #[test]
    fn the_token_does_not_pass_the_host_rule() {
        let mut headers = bearer_headers("Bearer secret");
        headers.insert(
            axum::http::header::HOST,
            "evil.example".parse().expect("a host"),
        );
        assert!(check(&Method::POST, &headers, 7465, &token("secret")).is_some());
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
