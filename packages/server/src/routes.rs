//! The router assembly — `app(state)` for serving, `router(state)` for
//! oneshot tests (the ayane split).
//!
//! Two API lanes share the tree: the WebSocket JSON-RPC surface at
//! `/api/rpc` (the webui's primary API) and a deliberately TINY REST
//! whitelist — `/api/health` (probes + container healthcheck), the
//! password login (the bootstrap: it mints the JWT every later call,
//! WS included, rides), the multipart upload (binary bodies do not
//! belong on a JSON control lane), and `/media/{key}` (large binary
//! serving). Every other former `/api/v1` route lives on as an RPC
//! method (`rpc.rs`); unknown `/api` paths answer 404 JSON (the
//! fallback's API-territory rule).

use axum::routing::get;
use axum::Router;
use tower_http::compression::predicate::SizeAbove;
use tower_http::compression::CompressionLayer;
use tower_http::limit::RequestBodyLimitLayer;

use plana_error_landing::ErrorLanding;

use crate::api;
use crate::config::Config;
use crate::media;
use crate::rpc;
use crate::state::AppState;
use crate::static_files;

/// The complete app: the router plus the body limit and the dynamic
/// gzip layer. The predicate deliberately drops tower-http's default
/// "skip images" rule — the gzip-test branch's intent was compressing
/// the dynamically served media responses themselves for clients that
/// advertise `Accept-Encoding: gzip` (bodies above 1 KiB only; smaller
/// ones lose more to gzip framing than they gain).
pub fn app(state: AppState) -> Router {
    router(state)
        .layer(RequestBodyLimitLayer::new(crate::config::BODY_LIMIT))
        .layer(CompressionLayer::new().compress_when(SizeAbove::new(1024)))
}

/// The route tree alone.
///
/// The landing covers everything EXCEPT `/media/{key}`: a media key
/// answers the same 404 whatever asked for it (the R-J invariant — an
/// anonymous direct link gets 200 or 404 and nothing else, no oracle),
/// so a browser navigation to a missing key must keep the JSON body an
/// `<img>`-bearing page and a link-preview crawler see. A landing there
/// would make one status wear two shapes depending on headers.
///
/// axum applies `.layer()` only to the routes registered BEFORE it, so
/// the media lane is built as its own layer-free router and merged after
/// the layered tree.
pub fn router(state: AppState) -> Router {
    let rpc_state = state.clone();
    // Cheap to clone: the landing holds one `Arc<str>` and is resolved
    // once, when the state is assembled.
    let error_landing = state.error_landing.clone();
    let media_state = state.clone();
    Router::new()
        .route("/api/health", get(api::settings::health))
        .route("/api/mcp/sse", get(crate::mcp::sse))
        .route("/api/mcp/message", axum::routing::post(crate::mcp::message))
        .nest("/api/v1", api::router())
        .route("/", get(static_files::serve_root))
        .route("/assets/{*path}", get(static_files::serve_asset))
        .fallback(static_files::fallback)
        .with_state(state)
        // The RPC lane owns its state (the builder captured it); merge
        // the stateless router it produces after the stateful tree.
        .merge(rpc::router(rpc_state))
        // Innermost, over the whole tree ABOVE — every handler and the
        // fallback included, so `app()`'s body limit and gzip sit
        // OUTSIDE it. The layer rewrites only a browser navigation that
        // ended in a 4xx/5xx bare-text body; fetch/XHR keeps its JSON.
        .layer(ErrorLanding::clone(&error_landing).layer())
        // The media lane, deliberately OUTSIDE the landing (see above).
        .merge(
            Router::new()
                .route("/media/{key}", get(media::download))
                .with_state(media_state),
        )
}

/// The error landing's shell: the very `index.html` bytes `/` serves
/// (`static_files::index_bytes_blocking`, both the embedded and the dev
/// mode), read once while the state is assembled.
///
/// A build with no dist has no frontend to hand over, so the layer is
/// `disabled()` and every response passes through untouched — the same
/// posture as this tree's other headless paths, and never a landing
/// painted from a shell the service cannot serve.
///
/// Non-UTF-8 shell bytes are treated as no shell: the layer would have to
/// splice markup into a document it cannot read, and a direct pass-through
/// is the honest answer there.
pub(crate) fn error_landing(config: &Config) -> ErrorLanding {
    landing_from_shell(static_files::index_bytes_blocking(config))
}

/// The shell decision itself: decodable bytes build a landing, anything
/// else is inert. Split out so the degradation rules are unit-testable
/// without a dist tree on disk.
fn landing_from_shell(shell: Option<Vec<u8>>) -> ErrorLanding {
    match shell_decoded(shell) {
        Shell::None | Shell::Undecodable => ErrorLanding::disabled(),
        Shell::Document(shell) => ErrorLanding::new(shell),
    }
}

/// What the build could produce for the landing's shell — the three cases
/// that decide between a real landing and an inert one.
///
/// The crate hands back an opaque `ErrorLanding`, so the verdict is kept
/// here where it was made: a test can assert the degradation rule itself
/// instead of inferring it from behaviour that needs a dist tree.
#[derive(Debug, PartialEq, Eq)]
enum Shell {
    /// No `index.html` in the build (a headless `--no-default-features`
    /// binary, or a dev-mode process without a dist on disk).
    None,
    /// Bytes that are not UTF-8: not a document this layer may splice
    /// markup into.
    Undecodable,
    /// The shell to serve landings out of.
    Document(String),
}

/// Classify the shell bytes. See [`Shell`].
fn shell_decoded(shell: Option<Vec<u8>>) -> Shell {
    match shell {
        None => Shell::None,
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(document) => Shell::Document(document),
            Err(_) => Shell::Undecodable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shell decision, factored out of [`error_landing`] so the
    /// degradation rules are testable without a dist tree on disk.
    ///
    /// A build that cannot produce a shell must pass every response
    /// through (`disabled()`) rather than paint a landing from nothing —
    /// and shell bytes that are not UTF-8 are "cannot produce a shell":
    /// the layer splices markup into the document, so a document it
    /// cannot read is not one it may rewrite. `from_utf8_lossy` here
    /// would silently substitute U+FFFD and hand hikari a corrupted page;
    /// `a_non_utf8_shell_disables_the_landing` is the pin against that.
    #[test]
    fn a_missing_shell_disables_the_landing() {
        assert_eq!(shell_decoded(None), Shell::None);
    }

    #[test]
    fn a_decodable_shell_builds_a_landing() {
        assert_eq!(
            shell_decoded(Some(b"<!doctype html><html></html>".to_vec())),
            Shell::Document("<!doctype html><html></html>".to_owned())
        );
    }

    #[test]
    fn a_non_utf8_shell_disables_the_landing() {
        // 0xFF is not a valid UTF-8 start byte.
        assert_eq!(
            shell_decoded(Some(vec![0x3c, 0xff, 0xfe, 0x3e])),
            Shell::Undecodable
        );
    }
}
