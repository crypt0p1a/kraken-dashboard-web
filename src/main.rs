//! kraken-dashboard-web: keeps what kraken-trading-bot pushes at each tick
//!
//! ```text
//! kraken-dashboard-web [--listen ADDRESS]
//! kraken-dashboard-web hash-password       the line of .env for a password typed, never shown
//! ```
//!
//! ADDRESS: where it listens, KRAKEN_DASHBOARD_WEB_LISTEN else 127.0.0.1:8080. In the image,
//! 0.0.0.0:8080: reached through the port Docker publishes.
//!
//! The page is served from KRAKEN_DASHBOARD_WEB_PAGE, else ./front/dist (built by
//! `trunk build --release` in front/): without it, the routes only.
//!
//! KRAKEN_DASHBOARD_WEB_PUSH_KEY, required: the key the bot signs its pushes with, its
//! KRAKEN_BOT_PUSH_KEY. 32 characters at least: `openssl rand -hex 32`.
//!
//! KRAKEN_DASHBOARD_WEB_PASSWORD_HASH, required: the password of the dashboard, hashed, as
//! `hash-password` gives it. KRAKEN_DASHBOARD_WEB_COOKIE_SECURE=false for a page served in plain
//! HTTP, whose browser wouldn't send back a cookie only for HTTPS.

use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "kraken-dashboard-web [--listen ADDRESS]
kraken-dashboard-web hash-password

ADDRESS: where it listens, KRAKEN_DASHBOARD_WEB_LISTEN else 127.0.0.1:8080
hash-password: the line of .env for a password typed, never shown";

/// The line of .env for a password typed twice (read from the input when it isn't a terminal),
/// quoted: docker compose reads a $ in it literally
fn hash_password() -> Result<String, String> {
    use std::io::IsTerminal;
    let password = if std::io::stdin().is_terminal() {
        let first = rpassword::prompt_password("password (12 characters at least): ")
            .map_err(|error| error.to_string())?;
        let again =
            rpassword::prompt_password("the same again: ").map_err(|error| error.to_string())?;
        if first != again {
            return Err("not the same twice".into());
        }
        first
    } else {
        let mut line = String::new();
        std::io::stdin()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        line.trim_end_matches(['\r', '\n']).to_string()
    };
    let hash = kraken_dashboard_web::auth::hash_password(&password)?;
    Ok(format!("KRAKEN_DASHBOARD_WEB_PASSWORD_HASH='{hash}'"))
}

const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

/// KRAKEN_DASHBOARD_WEB_COOKIE_SECURE: true unless told false, whatever its case; another word is
/// refused rather than read as true, a cookie the browser would never send back
fn secure_cookie(value: &str) -> Result<bool, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        other => Err(format!(
            "KRAKEN_DASHBOARD_WEB_COOKIE_SECURE: \"{other}\", true or false"
        )),
    }
}

fn listen(arguments: &[String], environment: Option<String>) -> Result<String, String> {
    match arguments {
        [] => Ok(environment
            .map(|it| it.trim().to_string())
            .filter(|it| !it.is_empty())
            .unwrap_or_else(|| DEFAULT_LISTEN.to_string())),
        [flag, address] if flag == "--listen" => Ok(address.clone()),
        other => Err(format!("unexpected arguments {other:?}")),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.iter().any(|it| it == "-h" || it == "--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if arguments == ["hash-password"] {
        return match hash_password() {
            Ok(line) => {
                println!("{line}");
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("kraken-dashboard-web: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let address = match listen(
        &arguments,
        std::env::var("KRAKEN_DASHBOARD_WEB_LISTEN").ok(),
    ) {
        Ok(address) => address,
        Err(error) => {
            eprintln!("kraken-dashboard-web: {error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match tokio::net::TcpListener::bind(&address).await {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!(%address, %error, "can't listen");
            return ExitCode::FAILURE;
        }
    };
    let push_key = match kraken_dashboard_web::signature::check_key(
        &std::env::var("KRAKEN_DASHBOARD_WEB_PUSH_KEY").unwrap_or_default(),
    ) {
        Ok(key) => key,
        Err(error) => {
            eprintln!("kraken-dashboard-web: {error}");
            return ExitCode::FAILURE;
        }
    };
    let password_hash = match kraken_dashboard_web::auth::check_hash(
        &std::env::var("KRAKEN_DASHBOARD_WEB_PASSWORD_HASH").unwrap_or_default(),
    ) {
        Ok(hash) => hash,
        Err(error) => {
            eprintln!("kraken-dashboard-web: {error}");
            return ExitCode::FAILURE;
        }
    };
    // HTTPS but told otherwise: the cookie is sent back over HTTPS only
    let secure = match secure_cookie(
        &std::env::var("KRAKEN_DASHBOARD_WEB_COOKIE_SECURE").unwrap_or_default(),
    ) {
        Ok(secure) => secure,
        Err(error) => {
            eprintln!("kraken-dashboard-web: {error}");
            return ExitCode::FAILURE;
        }
    };
    let page = std::env::var_os("KRAKEN_DASHBOARD_WEB_PAGE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("front/dist"));
    let page = if page.join("index.html").is_file() {
        tracing::info!(%address, page = %page.display(), "listening");
        Some(page)
    } else {
        tracing::warn!(%address, page = %page.display(), "listening, without the page: built by trunk build --release in front/");
        None
    };
    let config = kraken_dashboard_web::Config {
        push_key,
        auth: kraken_dashboard_web::auth::Auth::new(password_hash, secure),
        page,
    };
    match kraken_dashboard_web::serve(listener, config, shutdown()).await {
        Ok(()) => {
            tracing::info!("stopped");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "stopped");
            ExitCode::FAILURE
        }
    }
}

/// SIGTERM (docker stop) or Ctrl-C
async fn shutdown() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_whether_the_cookie_is_secure() {
        for (value, secure) in [
            ("", true),
            ("TRUE", true),
            ("False", false),
            (" no ", false),
        ] {
            assert_eq!(secure_cookie(value), Ok(secure), "{value}");
        }
        assert!(secure_cookie("off").is_err());
    }

    #[test]
    fn reads_where_to_listen() {
        let args = |values: &[&str]| values.iter().map(|it| it.to_string()).collect::<Vec<_>>();
        assert_eq!(listen(&args(&[]), None).unwrap(), DEFAULT_LISTEN);
        assert_eq!(
            listen(&args(&[]), Some(" 0.0.0.0:8080 ".into())).unwrap(),
            "0.0.0.0:8080"
        );
        assert_eq!(listen(&args(&[]), Some("".into())).unwrap(), DEFAULT_LISTEN);
        // the argument wins
        assert_eq!(
            listen(
                &args(&["--listen", "[::]:9000"]),
                Some("0.0.0.0:8080".into())
            )
            .unwrap(),
            "[::]:9000"
        );
        assert!(listen(&args(&["--listen"]), None).is_err());
        assert!(listen(&args(&["--port", "1"]), None).is_err());
    }
}
