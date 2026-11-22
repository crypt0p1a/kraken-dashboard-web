//! kraken-dashboard-web: the web backend of [kraken-trading-bot]. The bot pushes, at each tick, what
//! each screen of its dashboard shows; the backend keeps the last push in memory, to serve it to
//! the dashboard on the web. Read only: nothing goes back to the bot.
//!
//! | Route | From | What |
//! |-------|------|------|
//! | `PUT /state` | the bot | [`Push`]: every screen, JSON, compressed with gzip or not, [signed](signature) |
//! | `GET /version` | a reader | [`Version`]: changes at each push, and from a run to the next |
//! | `GET /data?range=day&account=2` | a reader | the data of a screen, as the bot pushed it; no `account`: all of them added up |
//! | `GET /` | a browser | the page: the dashboard as the terminal shows it, reading the routes above |
//!
//! The data of a screen is kept as the bot wrote it, never read here: what it holds is the
//! business of the bot and of the dashboard. Nothing is kept on disk: started again, the backend
//! has nothing until the next push of the bot.
//!
//! The pushes are signed with a key only the bot and the backend know ([`signature`]).
//!
//! [kraken-trading-bot]: https://github.com/crypt0p1a/kraken-trading-bot

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use flate2::Compression;
use flate2::read::MultiGzDecoder;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

pub mod signature;

pub const STATE: &str = "/state";
pub const VERSION: &str = "/version";
pub const DATA: &str = "/data";

/// The largest push as it comes, compressed or not
pub const MAX_PUSH: usize = 64 * 1024 * 1024;

/// The largest push once decompressed
pub const MAX_PUSH_DECOMPRESSED: usize = 256 * 1024 * 1024;

/// What a screen shows: a range of an account, or of all of them added up when None
#[derive(Debug, Deserialize)]
pub struct View {
    /// hour, day, week, month, all
    pub range: String,
    pub account: Option<i64>,
    /// As the bot wrote it
    pub data: Box<RawValue>,
}

/// Every screen at once, as the database of the bot was at a time
#[derive(Debug, Deserialize)]
pub struct Push {
    /// When the bot read it, by its clock, UTC milliseconds
    pub at: i64,
    pub views: Vec<View>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Version {
    /// Of the last push. From when the backend started: never the same number twice, a reader
    /// which read before a restart reads again after
    pub version: u64,
    /// When the bot read its last push, UTC milliseconds: None before any
    pub pushed_at: Option<i64>,
}

/// The query of `GET /data`
#[derive(Debug, Clone, Deserialize)]
pub struct DataQuery {
    pub range: String,
    pub account: Option<i64>,
}

struct Stored {
    version: u64,
    pushed_at: Option<i64>,
    /// The data of each screen as JSON compressed with gzip, ready to be served
    views: HashMap<(String, Option<i64>), Bytes>,
}

/// What the backend holds, shared by its routes
#[derive(Clone)]
pub struct Shared {
    stored: Arc<Mutex<Stored>>,
    /// The pushes one after the other, in the order they came: one the bot gave up on, come first,
    /// never replaces the next
    pushing: Arc<tokio::sync::Mutex<()>>,
    /// Checks the signatures of the pushes
    signatures: Arc<signature::Verifier>,
}

impl Shared {
    /// Nothing pushed yet, the version starting from `start`, the pushes signed with `push_key`
    pub fn new(start: u64, push_key: Vec<u8>) -> Self {
        Self {
            signatures: Arc::new(signature::Verifier::new(push_key)),
            stored: Arc::new(Mutex::new(Stored {
                version: start,
                pushed_at: None,
                views: HashMap::new(),
            })),
            pushing: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Stored> {
        // a route never panics holding it; if one did, what's stored is still whole
        self.stored
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// What the backend is started with
pub struct Config {
    /// The key the bot signs its pushes with
    pub push_key: Vec<u8>,
    /// Where the page is (front/dist): the routes only without it
    pub page: Option<std::path::PathBuf>,
}

/// UTC milliseconds
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |it| it.as_millis() as i64)
}

/// The routes and the page, the version starting from now
pub fn router(config: Config) -> Router {
    let router = router_with(Shared::new(now_millis() as u64, config.push_key));
    match &config.page {
        Some(page) => with_page(router, page),
        None => router,
    }
}

/// The page served from `assets` (front/dist), for what isn't a route: its index at `/`
pub fn with_page(router: Router, assets: &Path) -> Router {
    let page = tower_http::services::ServeDir::new(assets).append_index_html_on_directories(true);
    let compressed = tower::Layer::layer(&tower_http::compression::CompressionLayer::new(), page);
    router
        .fallback_service(compressed)
        .layer(axum::middleware::map_response(asked_again))
}

/// The page itself asked again at each visit, never kept by the browser: an image built again
/// serves new files, whose names, which change with what they hold, are in it. Those files are kept
async fn asked_again(mut response: Response) -> Response {
    let html = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|it| it.to_str().ok())
        .is_some_and(|it| it.starts_with("text/html"));
    if html {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static("no-cache"),
        );
    }
    response
}

pub fn router_with(shared: Shared) -> Router {
    Router::new()
        // the push only is large: the other bodies keep the default limit
        .route(STATE, put(push).layer(DefaultBodyLimit::max(MAX_PUSH)))
        .route(VERSION, get(version))
        .route(DATA, get(data))
        .with_state(shared)
}

/// Serves until `shutdown`
pub async fn serve(
    listener: tokio::net::TcpListener,
    config: Config,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(config))
        .with_graceful_shutdown(shutdown)
        .await
}

/// Compressed, as `Content-Encoding: gzip`
pub fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::with_capacity(bytes.len() / 4), Compression::default());
    // in memory: writing can't fail
    encoder.write_all(bytes).expect("compressed in memory");
    encoder.finish().expect("compressed in memory")
}

/// What [`gzip`] compressed, its members one after the other, `limit` bytes at most once
/// decompressed: a few bytes can't fill the memory
pub fn gunzip(bytes: &[u8], limit: usize) -> Result<Vec<u8>, String> {
    let mut decompressed = Vec::new();
    MultiGzDecoder::new(bytes)
        .take(limit as u64 + 1)
        .read_to_end(&mut decompressed)
        .map_err(|error| format!("not gzip: {error}"))?;
    if decompressed.len() > limit {
        return Err(format!("larger than {limit} bytes once decompressed"));
    }
    Ok(decompressed)
}

/// How the body of a push is encoded
enum Encoding {
    Plain,
    Gzip,
}

/// `Content-Encoding`: none, gzip (x-gzip) or identity, nothing else
fn encoding(headers: &HeaderMap) -> Result<Encoding, String> {
    let codings: Vec<String> = headers
        .get_all(header::CONTENT_ENCODING)
        .iter()
        .map(|it| it.to_str().unwrap_or("?"))
        .flat_map(|it| it.split(','))
        .map(|it| it.trim().to_ascii_lowercase())
        .filter(|it| !it.is_empty() && it != "identity")
        .collect();
    match codings.as_slice() {
        [] => Ok(Encoding::Plain),
        [coding] if coding == "gzip" || coding == "x-gzip" => Ok(Encoding::Gzip),
        other => Err(format!("Content-Encoding {other:?}: gzip or nothing only")),
    }
}

/// The bot: replaces everything shown, its signature checked first
async fn push(State(shared): State<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    if let Err(refused) = shared.signatures.verify(&headers, &body, now_millis()) {
        tracing::warn!(why = %refused.1, "a push refused");
        return refused.into_response();
    }
    let _turn = shared.pushing.lock().await;
    let encoding = match encoding(&headers) {
        Ok(encoding) => encoding,
        Err(error) => return (StatusCode::UNSUPPORTED_MEDIA_TYPE, error).into_response(),
    };
    // decompressed, read and compressed again screen by screen: off the threads serving the routes
    let read = tokio::task::spawn_blocking(move || -> Result<_, (StatusCode, String)> {
        let json = match encoding {
            Encoding::Gzip => gunzip(&body, MAX_PUSH_DECOMPRESSED)
                .map_err(|error| (StatusCode::BAD_REQUEST, error))?,
            Encoding::Plain => body.to_vec(),
        };
        let push: Push = serde_json::from_slice(&json)
            .map_err(|error| (StatusCode::UNPROCESSABLE_ENTITY, error.to_string()))?;
        let views = push
            .views
            .into_iter()
            .map(|view| {
                let data = Bytes::from(gzip(view.data.get().as_bytes()));
                ((view.range, view.account), data)
            })
            .collect::<HashMap<_, _>>();
        Ok((push.at, views))
    })
    .await;
    let (at, views) = match read {
        Ok(Ok(read)) => read,
        Ok(Err(refused)) => return refused.into_response(),
        Err(error) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response();
        }
    };
    let mut stored = shared.lock();
    stored.views = views;
    stored.version += 1;
    stored.pushed_at = Some(at);
    tracing::debug!(
        version = stored.version,
        views = stored.views.len(),
        "pushed"
    );
    StatusCode::NO_CONTENT.into_response()
}

async fn version(State(shared): State<Shared>) -> Json<Version> {
    let stored = shared.lock();
    Json(Version {
        version: stored.version,
        pushed_at: stored.pushed_at,
    })
}

/// The data of a screen, as the bot pushed it: compressed to whoever accepts gzip
async fn data(
    State(shared): State<Shared>,
    headers: HeaderMap,
    Query(query): Query<DataQuery>,
) -> Response {
    let found = {
        let stored = shared.lock();
        if stored.pushed_at.is_none() {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "nothing from the bot yet: is it running, with KRAKEN_BOT_PUSH_URL?",
            )
                .into_response();
        }
        stored
            .views
            .get(&(query.range.clone(), query.account))
            .cloned()
    };
    let Some(compressed) = found else {
        return (
            StatusCode::NOT_FOUND,
            match query.account {
                Some(account) => format!("no {} of account {account} from the bot", query.range),
                None => format!("no {} from the bot", query.range),
            },
        )
            .into_response();
    };
    // the same URL is gzip or not as it's asked: a cache keeps them apart
    let headers_of = [
        (header::CONTENT_TYPE, "application/json"),
        (header::VARY, "accept-encoding"),
    ];
    if accepts_gzip(&headers) {
        return (headers_of, [(header::CONTENT_ENCODING, "gzip")], compressed).into_response();
    }
    match gunzip(&compressed, MAX_PUSH_DECOMPRESSED) {
        Ok(plain) => (headers_of, plain).into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error).into_response(),
    }
}

/// `Accept-Encoding` names gzip, or `*` when it doesn't, with a weight other than 0
fn accepts_gzip(headers: &HeaderMap) -> bool {
    let codings: Vec<(String, bool)> = headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|it| it.to_str().ok())
        .flat_map(|it| it.split(','))
        .filter_map(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            let name = parts
                .next()
                .filter(|it| !it.is_empty())?
                .to_ascii_lowercase();
            // `q=0`, `Q=0`, `q = 0.000`
            let refused = parts.any(|it| {
                it.split_once('=').is_some_and(|(name, weight)| {
                    name.trim().eq_ignore_ascii_case("q")
                        && weight.trim().parse::<f32>().is_ok_and(|q| q == 0.0)
                })
            });
            Some((name, !refused))
        })
        .collect();
    let weight = |name: &str| {
        codings
            .iter()
            .find(|(it, _)| it == name)
            .map(|(_, accepted)| *accepted)
    };
    weight("gzip")
        .or_else(|| weight("x-gzip"))
        .or_else(|| weight("*"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompresses_what_it_compressed_up_to_a_limit() {
        let text = "{\"at\": 1, \"close\": 174.79}".repeat(1_000);
        let compressed = gzip(text.as_bytes());
        assert!(compressed.len() * 10 < text.len());
        assert_eq!(gunzip(&compressed, text.len()).unwrap(), text.as_bytes());
        assert!(
            gunzip(&compressed, text.len() - 1)
                .unwrap_err()
                .contains("larger than")
        );
        assert!(gunzip(b"not compressed", 100).is_err());
        let mut two = gzip(b"first, ");
        two.extend(gzip(b"second"));
        assert_eq!(gunzip(&two, 100).unwrap(), b"first, second");
    }

    #[test]
    fn tells_who_accepts_gzip() {
        for (accept, gzip) in [
            ("gzip", true),
            ("br, gzip;q=0.5", true),
            ("*", true),
            ("x-gzip", true),
            ("gzip;q=0, *", false),
            ("gzip;Q=0", false),
            ("gzip; q = 0.000", false),
            ("*;q=0", false),
            ("br", false),
            ("", false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, accept.parse().unwrap());
            assert_eq!(accepts_gzip(&headers), gzip, "{accept}");
        }
    }
}
