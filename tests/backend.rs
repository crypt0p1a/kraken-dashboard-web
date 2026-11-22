//! The routes of the backend, over HTTP on a port of its own

use kraken_dashboard_web::signature::{self, sign};
use kraken_dashboard_web::{Config, Shared, Version, gunzip, gzip};
use reqwest::StatusCode;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicI64, Ordering};

/// The key the bot and the backend of the tests share
const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

/// A push signed as the bot signs it, at a time of its own: two of the same body are two pushes
fn put_state(url: &str, body: Vec<u8>) -> reqwest::RequestBuilder {
    static LATER: AtomicI64 = AtomicI64::new(0);
    let at = kraken_dashboard_web::now_millis() + LATER.fetch_add(1, Ordering::SeqCst);
    reqwest::Client::new()
        .put(format!("{url}/state"))
        .header("content-type", "application/json")
        .header(signature::AT, at.to_string())
        .header(signature::SIGNATURE, sign(KEY, at, &body))
        .body(body)
}

/// The backend on a free port (0: the system picks it), its version starting from `start`
async fn backend_from(start: u64) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = kraken_dashboard_web::router_with(Shared::new(start, KEY.to_vec()));
    tokio::spawn(async move { axum::serve(listener, router).await });
    format!("http://{address}")
}

async fn backend() -> String {
    backend_from(100).await
}

/// A push as the bot sends it, its screens holding whatever the bot puts in them
fn push(at: i64, views: &[(&str, Option<i64>, i64)]) -> Value {
    json!({
        "at": at,
        "views": views
            .iter()
            .map(|(range, account, since)| json!({
                "range": range,
                "account": account,
                "data": {"since": since, "managers": [], "names": [[1, "main"]], "close": 174.79},
            }))
            .collect::<Vec<_>>(),
    })
}

async fn version(client: &reqwest::Client, url: &str) -> Version {
    client
        .get(format!("{url}/version"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn keeps_the_last_push_of_the_bot() {
    let url = backend().await;
    let client = reqwest::Client::new();

    // nothing yet: whoever reads is told why
    assert_eq!(
        version(&client, &url).await,
        Version {
            version: 100,
            pushed_at: None
        }
    );
    let response = client
        .get(format!("{url}/data?range=day"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        response
            .text()
            .await
            .unwrap()
            .contains("KRAKEN_BOT_PUSH_URL")
    );

    let response = put_state(
        &url,
        serde_json::to_vec(&push(
            5,
            &[("day", None, 10), ("day", Some(2), 20), ("week", None, 30)],
        ))
        .unwrap(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        version(&client, &url).await,
        Version {
            version: 101,
            pushed_at: Some(5)
        }
    );

    // each screen as the bot wrote it, the numbers included
    for (query, since) in [
        ("range=day", 10),
        ("range=day&account=2", 20),
        ("range=week", 30),
    ] {
        let data: Value = client
            .get(format!("{url}/data?{query}"))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(data["since"], since, "{query}");
        assert_eq!(data["close"], 174.79);
    }
    let response = client
        .get(format!("{url}/data?range=day&account=3"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(response.text().await.unwrap().contains("account 3"));

    // a push replaces everything, whatever the clock of the bot says
    put_state(
        &url,
        serde_json::to_vec(&push(1, &[("hour", None, 40)])).unwrap(),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(version(&client, &url).await.version, 102);
    let response = client
        .get(format!("{url}/data?range=day"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// The bot pushes compressed; a reader reads compressed, or not when it doesn't accept it
#[tokio::test]
async fn takes_and_serves_gzip() {
    let url = backend().await;
    let json = serde_json::to_vec(&push(1, &[("day", None, 10)])).unwrap();
    for (encoding, body, status) in [
        (Some("gzip"), gzip(&json), StatusCode::NO_CONTENT),
        (Some("x-gzip"), gzip(&json), StatusCode::NO_CONTENT),
        (Some("identity"), json.clone(), StatusCode::NO_CONTENT),
        (None, json.clone(), StatusCode::NO_CONTENT),
        (Some("br"), gzip(&json), StatusCode::UNSUPPORTED_MEDIA_TYPE),
        // not gzip, though it says so
        (Some("gzip"), json.clone(), StatusCode::BAD_REQUEST),
    ] {
        let mut request = put_state(&url, body);
        if let Some(encoding) = encoding {
            request = request.header("content-encoding", encoding);
        }
        assert_eq!(
            request.send().await.unwrap().status(),
            status,
            "{encoding:?}"
        );
    }

    let raw = reqwest::Client::builder().no_gzip().build().unwrap();
    let response = raw
        .get(format!("{url}/data?range=day"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(response.headers()["content-encoding"], "gzip");
    assert_eq!(response.headers()["vary"], "accept-encoding");
    let data: Value =
        serde_json::from_slice(&gunzip(&response.bytes().await.unwrap(), 1 << 20).unwrap())
            .unwrap();
    assert_eq!(data["since"], 10);
    let response = raw
        .get(format!("{url}/data?range=day"))
        .send()
        .await
        .unwrap();
    assert!(response.headers().get("content-encoding").is_none());
    let data: Value = response.json().await.unwrap();
    assert_eq!(data["since"], 10);
}

#[tokio::test]
async fn refuses_what_isnt_a_push() {
    let url = backend().await;
    let client = reqwest::Client::new();
    for body in ["{\"views\": 3}", "{\"at\": 1}", "not json"] {
        let response = put_state(&url, body.as_bytes().to_vec())
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body}"
        );
    }
    assert_eq!(version(&client, &url).await.pushed_at, None);
}

/// A push larger than the default limit of axum (2 MB) is taken; another body isn't
#[tokio::test]
async fn takes_a_large_push() {
    let url = backend().await;
    let large: Vec<(&str, Option<i64>, i64)> =
        (0..40_000).map(|it| ("day", Some(it), it)).collect();
    let json = serde_json::to_vec(&push(1, &large)).unwrap();
    assert!(json.len() > 2 * 1024 * 1024);
    let response = put_state(&url, json).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

/// Started again: a reader which read before reads again
#[tokio::test]
async fn never_has_the_same_version_from_a_run_to_the_next() {
    let client = reqwest::Client::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(kraken_dashboard_web::serve(
        listener,
        Config {
            push_key: KEY.to_vec(),
            auth: kraken_dashboard_web::auth::Auth::new(
                kraken_dashboard_web::auth::hash_password("a long enough password").unwrap(),
                true,
            ),
            page: None,
        },
        std::future::pending(),
    ));
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    // behind a session: refused, then read once logged in
    let url = format!("http://{address}");
    let response = client.get(format!("{url}/version")).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let logged = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .post(format!("{url}/login"))
        .form(&[("password", "a long enough password")])
        .send()
        .await
        .unwrap();
    let session = logged.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let started = client
        .get(format!("{url}/version"))
        .header("cookie", session)
        .send()
        .await
        .unwrap()
        .json::<Version>()
        .await
        .unwrap()
        .version;
    assert!(started.abs_diff(now) < 60_000, "{started} {now}");
}

/// The page, from its folder, compressed for whoever accepts it; the routes before it
#[tokio::test]
async fn serves_the_page() {
    let folder =
        std::env::temp_dir().join(format!("kraken-dashboard-web-page-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("index.html"), "<html>the desk".repeat(100)).unwrap();
    std::fs::write(folder.join("front.wasm"), b"\0asm").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let router = kraken_dashboard_web::app(Shared::new(100, KEY.to_vec()), Some(&folder));
    tokio::spawn(async move { axum::serve(listener, router).await });

    let raw = reqwest::Client::builder().no_gzip().build().unwrap();
    let response = raw
        .get(format!("{url}/"))
        .header("accept-encoding", "gzip")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-encoding"], "gzip");
    // the page asked again at each visit: a new image, a new page
    assert_eq!(response.headers()["cache-control"], "no-cache");
    let text = reqwest::get(format!("{url}/"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(text.starts_with("<html>the desk"));
    assert_eq!(
        reqwest::get(format!("{url}/front.wasm"))
            .await
            .unwrap()
            .headers()["content-type"],
        "application/wasm"
    );
    // the routes first
    assert_eq!(version(&reqwest::Client::new(), &url).await.version, 100);
    assert_eq!(
        reqwest::get(format!("{url}/nothing"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    std::fs::remove_dir_all(folder).unwrap();
}

/// Only the bot pushes: a push not signed with the key, or heard on the way and sent again, is
/// refused before it's read
#[tokio::test]
async fn takes_the_pushes_of_the_bot_only() {
    let url = backend().await;
    let client = reqwest::Client::new();
    let body = serde_json::to_vec(&push(1, &[("day", None, 10)])).unwrap();
    let at = kraken_dashboard_web::now_millis();
    for (signature, why) in [
        (None, "not signed"),
        (
            Some(sign(b"a key which isn't the one of the bot", at, &body)),
            "wrong signature",
        ),
    ] {
        let mut request = client
            .put(format!("{url}/state"))
            .header("content-type", "application/json")
            .header(signature::AT, at.to_string())
            .body(body.clone());
        if let Some(signature) = signature {
            request = request.header(signature::SIGNATURE, signature);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.text().await.unwrap().contains(why));
    }
    assert_eq!(version(&client, &url).await.pushed_at, None);

    let signed = put_state(&url, body);
    let again = signed.try_clone().unwrap();
    assert_eq!(
        signed.send().await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(again.send().await.unwrap().status(), StatusCode::CONFLICT);
    assert_eq!(version(&client, &url).await.version, 101);
}

/// The page and what it reads, with a session only: the password opens one, the pushes of the bot
/// need none, their signature is theirs
#[tokio::test]
async fn reads_with_a_session_only() {
    use kraken_dashboard_web::auth::{Auth, hash_password};

    let folder =
        std::env::temp_dir().join(format!("kraken-dashboard-web-login-{}", std::process::id()));
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("index.html"), "<html>the desk").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shared = Shared::new(100, KEY.to_vec()).with_auth(Auth::new(
        hash_password("a long enough password").unwrap(),
        true,
    ));
    let router = kraken_dashboard_web::app(shared, Some(&folder));
    tokio::spawn(async move { axum::serve(listener, router).await });
    // the redirections read, not followed
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();

    // no session: the page sends to the login, the routes refuse
    let page = client.get(format!("{url}/")).send().await.unwrap();
    assert_eq!(page.status(), StatusCode::SEE_OTHER);
    assert_eq!(page.headers()["location"], "/login");
    for route in ["/version", "/data?range=day"] {
        let response = client.get(format!("{url}{route}")).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "{route}");
    }
    let login = client.get(format!("{url}/login")).send().await.unwrap();
    assert_eq!(login.status(), StatusCode::OK);
    let text = login.text().await.unwrap();
    assert!(text.contains("type=\"password\""), "{text}");

    // the bot pushes all the same, signed
    let push = put_state(
        &url,
        serde_json::to_vec(&push(1, &[("day", None, 10)])).unwrap(),
    );
    assert_eq!(push.send().await.unwrap().status(), StatusCode::NO_CONTENT);

    // a wrong password: the page again, told
    let wrong = client
        .post(format!("{url}/login"))
        .form(&[("password", "not the password")])
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert!(wrong.headers().get("set-cookie").is_none());
    assert!(wrong.text().await.unwrap().contains("Not the password"));

    // the password: a session, its cookie for the browser only
    let right = client
        .post(format!("{url}/login"))
        .form(&[("password", "a long enough password")])
        .send()
        .await
        .unwrap();
    assert_eq!(right.status(), StatusCode::SEE_OTHER);
    assert_eq!(right.headers()["location"], "/");
    let cookie = right.headers()["set-cookie"].to_str().unwrap().to_string();
    for it in ["kraken_session=", "HttpOnly", "SameSite=Lax", "Secure"] {
        assert!(cookie.contains(it), "{it}: {cookie}");
    }
    let session = cookie.split(';').next().unwrap().to_string();
    let with = |route: &str| {
        client
            .get(format!("{url}{route}"))
            .header("cookie", session.clone())
            .send()
    };
    assert_eq!(
        with("/").await.unwrap().text().await.unwrap(),
        "<html>the desk"
    );
    assert_eq!(with("/version").await.unwrap().status(), StatusCode::OK);
    let data = with("/data?range=day").await.unwrap();
    assert_eq!(data.status(), StatusCode::OK);
    // logged in: the login sends to the page
    assert_eq!(
        with("/login").await.unwrap().status(),
        StatusCode::SEE_OTHER
    );

    // logged out: the session over, the cookie forgotten
    let out = with("/logout").await.unwrap();
    assert_eq!(out.status(), StatusCode::SEE_OTHER);
    assert!(
        out.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .contains("Max-Age=0")
    );
    assert_eq!(
        with("/version").await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    std::fs::remove_dir_all(folder).unwrap();
}

/// Too many tries in a minute: refused, the right one included, without being checked. Sent all at
/// once, they count as many
#[tokio::test]
async fn refuses_logins_after_too_many_wrong_passwords() {
    use kraken_dashboard_web::auth::{Auth, TRIES, hash_password};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let shared = Shared::new(100, KEY.to_vec()).with_auth(Auth::new(
        hash_password("a long enough password").unwrap(),
        true,
    ));
    let router = kraken_dashboard_web::router_with(shared);
    tokio::spawn(async move { axum::serve(listener, router).await });
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let log_in = |password: &'static str| {
        client
            .post(format!("{url}/login"))
            .form(&[("password", password)])
            .send()
    };
    for _ in 0..TRIES {
        assert_eq!(
            log_in("wrong").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }
    let refused = log_in("a long enough password").await.unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.text().await.unwrap().contains("wait a minute"));
}
