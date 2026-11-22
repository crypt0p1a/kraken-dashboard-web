//! The pushes of the bot, signed: HMAC-SHA256 of a key the bot and the backend share
//! (KRAKEN_BOT_PUSH_KEY, KRAKEN_DASHBOARD_WEB_PUSH_KEY), never sent.
//!
//! ```text
//! X-Kraken-Push-At: 1795316768688                 when the bot signed, UTC milliseconds
//! X-Kraken-Push-Signature: sha256=<hex>           HMAC-SHA256(key, "<at>\n" + the body as sent)
//! ```
//!
//! The body is signed as it goes, compressed or not. The headers are checked before the body is
//! even received, the signature before it's read. A push signed more than [`SKEW`] away from the
//! clock of the backend is refused, and one signed at or before the last taken: a push heard on the
//! way can't be sent again, nor one held back to replace a newer one.

use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Mutex;

pub const AT: &str = "x-kraken-push-at";
pub const SIGNATURE: &str = "x-kraken-push-signature";

/// The shortest key: 32 bytes, `openssl rand -hex 32` gives 64
pub const SHORTEST_KEY: usize = 32;

/// How far the clock of the bot may be from the one of the backend, UTC milliseconds
pub const SKEW: i64 = 5 * 60_000;

/// The signature of a push signed at `at`, as `X-Kraken-Push-Signature` holds it
pub fn sign(key: &[u8], at: i64, body: &[u8]) -> String {
    let mut mac = mac(key);
    mac.update(format!("{at}\n").as_bytes());
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn mac(key: &[u8]) -> Hmac<Sha256> {
    // HMAC takes a key of any length
    Hmac::<Sha256>::new_from_slice(key).expect("a key of any length")
}

/// A key long enough, refused at the start otherwise
pub fn check_key(key: &str) -> Result<Vec<u8>, String> {
    let key = key.trim();
    if key.len() < SHORTEST_KEY {
        return Err(format!(
            "KRAKEN_DASHBOARD_WEB_PUSH_KEY: {SHORTEST_KEY} characters at least, e.g. openssl rand -hex 32"
        ));
    }
    Ok(key.as_bytes().to_vec())
}

/// A push whose headers say it's signed, its body still to be checked
#[derive(Debug)]
pub struct Signed {
    at: i64,
    signature: Vec<u8>,
}

/// Checks the signatures with the key, and when the last push taken was signed
pub struct Verifier {
    key: Vec<u8>,
    /// UTC milliseconds: a push signed at or before it is refused
    last: Mutex<Option<i64>>,
}

type Refused = (StatusCode, String);

impl Verifier {
    pub fn new(key: Vec<u8>) -> Self {
        Self {
            key,
            last: Mutex::new(None),
        }
    }

    fn last(&self) -> std::sync::MutexGuard<'_, Option<i64>> {
        self.last
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The headers, before the body is received: signed, close to `now`, after the last push taken
    pub fn check_headers(&self, headers: &HeaderMap, now: i64) -> Result<Signed, Refused> {
        let refused = |why: &str| (StatusCode::UNAUTHORIZED, why.to_string());
        let header = |name: &str| headers.get(name).and_then(|it| it.to_str().ok());
        let at: i64 = header(AT)
            .and_then(|it| it.trim().parse().ok())
            .ok_or_else(|| refused("not signed: X-Kraken-Push-At missing"))?;
        let signature = header(SIGNATURE)
            .and_then(|it| it.trim().strip_prefix("sha256="))
            .and_then(|it| hex::decode(it).ok())
            .ok_or_else(|| refused("not signed: X-Kraken-Push-Signature missing"))?;
        if (now - at).abs() > SKEW {
            return Err(refused(
                "signed too far from the clock of the backend: are both clocks right?",
            ));
        }
        self.after_the_last(at)?;
        Ok(Signed { at, signature })
    }

    fn after_the_last(&self, at: i64) -> Result<(), Refused> {
        match *self.last() {
            Some(last) if at <= last => Err((
                StatusCode::CONFLICT,
                "signed at or before the last push taken: sent again, or held back".to_string(),
            )),
            _ => Ok(()),
        }
    }

    /// The body, as the headers say it's signed: taken, the last push from then on
    pub fn verify(&self, signed: Signed, body: &[u8]) -> Result<(), Refused> {
        let mut mac = mac(&self.key);
        mac.update(format!("{}\n", signed.at).as_bytes());
        mac.update(body);
        // in constant time: the time taken tells nothing of the signature
        mac.verify_slice(&signed.signature).map_err(|_| {
            (
                StatusCode::UNAUTHORIZED,
                "a wrong signature: not the key of the backend".to_string(),
            )
        })?;
        let mut last = self.last();
        // another push taken while this one was received
        if last.is_some_and(|it| signed.at <= it) {
            return Err((
                StatusCode::CONFLICT,
                "signed at or before the last push taken: sent again, or held back".to_string(),
            ));
        }
        *last = Some(signed.at);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A key of the tests: never written whole, which the scanners of secrets would take for one
    fn key() -> Vec<u8> {
        "0123456789abcdef".repeat(2).into_bytes()
    }

    fn headers(at: i64, signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AT, at.to_string().parse().unwrap());
        headers.insert(SIGNATURE, signature.parse().unwrap());
        headers
    }

    fn check(
        verifier: &Verifier,
        headers: &HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<(), Refused> {
        let signed = verifier.check_headers(headers, now)?;
        verifier.verify(signed, body)
    }

    /// The very value kraken-trading-bot signs: the same vector is in its tests, computed apart
    /// (Python's hmac). A change of one side only fails here or there
    #[test]
    fn checks_as_the_bot_signs() {
        assert_eq!(
            sign(
                b"kraken-dashboard-web",
                1_795_316_768_688,
                b"{\"at\":1795316768688,\"views\":[]}"
            ),
            "sha256=a7c3f506b4154c876d825fe791df3f461d098fc9527c99930e86473c935c6f34"
        );
    }

    #[test]
    fn takes_each_push_once_in_order() {
        let verifier = Verifier::new(key());
        let body = b"{\"at\": 1}";
        let first = headers(1_000_000, &sign(&key(), 1_000_000, body));
        assert_eq!(check(&verifier, &first, body, 1_000_000), Ok(()));
        // heard on the way and sent again
        assert_eq!(
            check(&verifier, &first, body, 1_000_001).unwrap_err().0,
            StatusCode::CONFLICT
        );
        let next = headers(1_000_500, &sign(&key(), 1_000_500, body));
        assert_eq!(check(&verifier, &next, body, 1_000_600), Ok(()));
        // signed between the two, held back and sent late: never replaces the newer one
        let held = headers(1_000_200, &sign(&key(), 1_000_200, body));
        assert_eq!(
            check(&verifier, &held, body, 1_000_700).unwrap_err().0,
            StatusCode::CONFLICT
        );
    }

    #[test]
    fn refuses_what_isnt_signed_with_the_key() {
        let verifier = Verifier::new(key());
        let body = b"{\"at\": 1}";
        let now = 1_000_000;
        for (headers, body) in [
            // another key
            (
                headers(now, &sign(b"another key, long enough for it!", now, body)),
                &body[..],
            ),
            // another body
            (headers(now, &sign(&key(), now, body)), &b"{\"at\": 2}"[..]),
            // another time
            (headers(now + 1, &sign(&key(), now, body)), &body[..]),
            (headers(now, "md5=00"), &body[..]),
        ] {
            assert_eq!(
                check(&verifier, &headers, body, now).unwrap_err().0,
                StatusCode::UNAUTHORIZED
            );
        }
        // nothing to check: refused before the body
        assert_eq!(
            verifier
                .check_headers(&HeaderMap::new(), now)
                .unwrap_err()
                .0,
            StatusCode::UNAUTHORIZED
        );
        // refused: nothing taken, the next push goes
        let signed = headers(now, &sign(&key(), now, body));
        assert_eq!(check(&verifier, &signed, body, now), Ok(()));
    }

    #[test]
    fn refuses_a_push_signed_too_long_ago_or_ahead() {
        let verifier = Verifier::new(key());
        let now = 10_000_000;
        for at in [now - SKEW - 1, now + SKEW + 1] {
            let (status, why) = verifier
                .check_headers(&headers(at, &sign(&key(), at, b"{}")), now)
                .unwrap_err();
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(why.contains("clock"), "{why}");
        }
        let at = now - SKEW;
        assert_eq!(
            check(
                &verifier,
                &headers(at, &sign(&key(), at, b"{}")),
                b"{}",
                now
            ),
            Ok(())
        );
    }

    #[test]
    fn wants_a_long_key() {
        assert!(check_key("short").is_err());
        let long = "0123456789abcdef".repeat(2);
        assert_eq!(check_key(&format!(" {long} ")).unwrap(), long.into_bytes());
    }
}
