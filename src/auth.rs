//! Who reads the dashboard: a password, hashed with argon2id (KRAKEN_DASHBOARD_WEB_PASSWORD_HASH,
//! made by `kraken-dashboard-web hash-password`), then a session in a cookie.
//!
//! - the session: 32 bytes at random, in a cookie `HttpOnly`, `SameSite=Strict`, `Secure` (unless
//!   told otherwise, plain HTTP), for [`SESSION`]. Only its SHA-256 is kept, in memory: the backend
//!   started again, everyone logs in again
//! - the tries: [`TRIES`] wrong passwords in a minute, whoever made them, and the next are refused
//!   without being checked until the minute is over. argon2 itself takes its time
//! - the pushes of the bot have their own signature: no session for them

use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use axum::http::{HeaderMap, header};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// The name of the cookie of the session
pub const COOKIE: &str = "kraken_session";

/// How long a session lasts, UTC milliseconds: a month
pub const SESSION: i64 = 30 * 24 * 3_600_000;

/// Wrong passwords in a minute, at most
pub const TRIES: usize = 5;

/// The hash of a password, as KRAKEN_DASHBOARD_WEB_PASSWORD_HASH holds it: argon2id, a salt at
/// random
pub fn hash_password(password: &str) -> Result<String, String> {
    if password.chars().count() < 12 {
        return Err("12 characters at least".into());
    }
    let mut random = [0u8; 16];
    getrandom::getrandom(&mut random).map_err(|error| error.to_string())?;
    let salt = SaltString::encode_b64(&random).map_err(|error| error.to_string())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|it| it.to_string())
        .map_err(|error| error.to_string())
}

/// A hash as `hash_password` makes it, checked at the start rather than at each login
pub fn check_hash(hash: &str) -> Result<String, String> {
    let hash = hash.trim().trim_matches('\'').to_string();
    PasswordHash::new(&hash).map_err(|_| {
        "KRAKEN_DASHBOARD_WEB_PASSWORD_HASH: not a hash, made by kraken-dashboard-web hash-password"
            .to_string()
    })?;
    Ok(hash)
}

/// Why a login was refused
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    WrongPassword,
    TooManyTries,
}

pub struct Auth {
    hash: String,
    /// Plain HTTP only: the cookie without `Secure`, which a browser doesn't send back otherwise
    secure: bool,
    /// The SHA-256 of each session, and when it ends
    sessions: Mutex<HashMap<[u8; 32], i64>>,
    /// When the wrong passwords of the last minute were given
    wrong: Mutex<VecDeque<i64>>,
}

fn digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

impl Auth {
    pub fn new(hash: String, secure: bool) -> Self {
        Self {
            hash,
            secure,
            sessions: Mutex::new(HashMap::new()),
            wrong: Mutex::new(VecDeque::new()),
        }
    }

    /// A new session when `password` is the one: its token, for the cookie
    pub fn log_in(&self, password: &str, now: i64) -> Result<String, Refused> {
        {
            let mut wrong = self.wrong.lock().unwrap_or_else(|it| it.into_inner());
            while wrong.front().is_some_and(|it| *it <= now - 60_000) {
                wrong.pop_front();
            }
            if wrong.len() >= TRIES {
                return Err(Refused::TooManyTries);
            }
        }
        let right = PasswordHash::new(&self.hash).is_ok_and(|hash| {
            Argon2::default()
                .verify_password(password.as_bytes(), &hash)
                .is_ok()
        });
        if !right {
            self.wrong
                .lock()
                .unwrap_or_else(|it| it.into_inner())
                .push_back(now);
            return Err(Refused::WrongPassword);
        }
        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes).expect("the randomness of the system");
        let token = hex::encode(bytes);
        let mut sessions = self.sessions.lock().unwrap_or_else(|it| it.into_inner());
        sessions.retain(|_, end| *end > now);
        sessions.insert(digest(&token), now + SESSION);
        Ok(token)
    }

    /// The token of the session in the cookies of a request
    fn token(headers: &HeaderMap) -> Option<String> {
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|it| it.to_str().ok())
            .flat_map(|it| it.split(';'))
            .filter_map(|it| it.trim().split_once('='))
            .find(|(name, _)| *name == COOKIE)
            .map(|(_, value)| value.to_string())
    }

    /// A session going on at `now`
    pub fn has_session(&self, headers: &HeaderMap, now: i64) -> bool {
        let Some(token) = Self::token(headers) else {
            return false;
        };
        self.sessions
            .lock()
            .unwrap_or_else(|it| it.into_inner())
            .get(&digest(&token))
            .is_some_and(|end| *end > now)
    }

    /// The session of the request, over
    pub fn log_out(&self, headers: &HeaderMap) {
        if let Some(token) = Self::token(headers) {
            self.sessions
                .lock()
                .unwrap_or_else(|it| it.into_inner())
                .remove(&digest(&token));
        }
    }

    /// The cookie of a session, `Set-Cookie`
    pub fn cookie(&self, token: &str) -> String {
        format!(
            "{COOKIE}={token}; Path=/; Max-Age={}; HttpOnly; SameSite=Strict{}",
            SESSION / 1000,
            if self.secure { "; Secure" } else { "" }
        )
    }

    /// The cookie of no session, `Set-Cookie`: the browser forgets it
    pub fn no_cookie(&self) -> String {
        format!(
            "{COOKIE}=; Path=/; Max-Age=0; HttpOnly; SameSite=Strict{}",
            if self.secure { "; Secure" } else { "" }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, cookie.parse().unwrap());
        headers
    }

    #[test]
    fn opens_a_session_with_the_password_only() {
        let auth = Auth::new(hash_password("a long enough password").unwrap(), true);
        assert_eq!(
            auth.log_in("not the password", 0),
            Err(Refused::WrongPassword)
        );
        let token = auth.log_in("a long enough password", 0).unwrap();
        assert_eq!(token.len(), 64);
        assert!(auth.has_session(&with(&format!("other=1; {COOKIE}={token}")), 1));
        assert!(!auth.has_session(&with(&format!("{COOKIE}=0123")), 1));
        assert!(!auth.has_session(&HeaderMap::new(), 1));
        // a month, then over
        assert!(!auth.has_session(&with(&format!("{COOKIE}={token}")), SESSION + 1));
        // logged out
        let again = auth.log_in("a long enough password", 0).unwrap();
        auth.log_out(&with(&format!("{COOKIE}={again}")));
        assert!(!auth.has_session(&with(&format!("{COOKIE}={again}")), 1));
    }

    #[test]
    fn refuses_tries_after_too_many_wrong_ones() {
        let auth = Auth::new(hash_password("a long enough password").unwrap(), true);
        for at in 0..TRIES as i64 {
            assert_eq!(auth.log_in("wrong", at), Err(Refused::WrongPassword));
        }
        // the right one too, not even checked, until the minute is over
        assert_eq!(
            auth.log_in("a long enough password", 1_000),
            Err(Refused::TooManyTries)
        );
        assert!(auth.log_in("a long enough password", 61_000).is_ok());
    }

    #[test]
    fn sets_a_cookie_only_the_page_reads() {
        let auth = Auth::new(hash_password("a long enough password").unwrap(), true);
        let cookie = auth.cookie("abc");
        for it in [
            "kraken_session=abc",
            "HttpOnly",
            "SameSite=Strict",
            "Secure",
            "Path=/",
        ] {
            assert!(cookie.contains(it), "{it}: {cookie}");
        }
        let plain = Auth::new(hash_password("a long enough password").unwrap(), false);
        assert!(!plain.cookie("abc").contains("Secure"));
        assert!(auth.no_cookie().contains("Max-Age=0"));
    }

    #[test]
    fn hashes_a_long_password_only() {
        assert!(hash_password("short").is_err());
        let hash = hash_password("a long enough password").unwrap();
        assert!(hash.starts_with("$argon2id$"), "{hash}");
        // as .env holds it, quoted
        assert_eq!(check_hash(&format!("'{hash}'")).unwrap(), hash);
        assert!(check_hash("not a hash").is_err());
    }
}
