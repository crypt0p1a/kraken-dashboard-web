//! kraken-dashboard-web: keeps what kraken-trading-bot pushes at each tick
//!
//! ```text
//! kraken-dashboard-web [--listen ADDRESS]
//! ```
//!
//! ADDRESS: where it listens, KRAKEN_DASHBOARD_WEB_LISTEN else 127.0.0.1:8080. In the image,
//! 0.0.0.0:8080: reached through the port Docker publishes.
//!
//! The page is served from KRAKEN_DASHBOARD_WEB_PAGE, else ./front/dist (built by
//! `trunk build --release` in front/): without it, the routes only.

use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "kraken-dashboard-web [--listen ADDRESS]

ADDRESS: where it listens, KRAKEN_DASHBOARD_WEB_LISTEN else 127.0.0.1:8080";

const DEFAULT_LISTEN: &str = "127.0.0.1:8080";

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
    match kraken_dashboard_web::serve(listener, page.as_deref(), shutdown()).await {
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
