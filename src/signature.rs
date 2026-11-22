//! The pushes of the bot, signed: HMAC-SHA256 of a key the bot and the backend share
//! (KRAKEN_BOT_PUSH_KEY, KRAKEN_DASHBOARD_WEB_PUSH_KEY), never sent.
//!
//! ```text
//! X-Kraken-Push-At: 1795316768688                 when the bot signed, UTC milliseconds
//! X-Kraken-Push-Signature: sha256=<hex>           HMAC-SHA256(key, "<at>\n" + the body as sent)
//! ```
//!
//! The body is signed as it goes, compressed or not: checked before it's read. A push signed more
//! than [`SKEW`] away from the clock of the backend is refused, and a signature already taken: a
//! push heard on the way can't be sent again.

use axum::http::{HeaderMap, StatusCode};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::VecDeque;
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

/// Checks the signatures with the key, the ones taken kept for [`SKEW`] on each side
pub struct Verifier {
    key: Vec<u8>,
    /// The signatures taken, oldest first: (when signed, signature)
    taken: Mutex<VecDeque<(i64, Vec<u8>)>>,
}

impl Verifier {
    pub fn new(key: Vec<u8>) -> Self {
        Self {
            key,
            taken: Mutex::new(VecDeque::new()),
        }
    }

    /// The push signed by the bot, at a time close to `now`, never taken before
    pub fn verify(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now: i64,
    ) -> Result<(), (StatusCode, String)> {
        let refused = |why: &str| (StatusCode::UNAUTHORIZED, why.to_string());
        let header = |name: &str| headers.get(name).and_then(|it| it.to_str().ok());
        let at: i64 = header(AT)
            .and_then(|it| it.trim().parse().ok())
            .ok_or_else(|| refused("not signed: X-Kraken-Push-At missing"))?;
        let signature = header(SIGNATURE)
            .and_then(|it| it.trim().strip_prefix("sha256="))
            .and_then(|it| hex::decode(it).ok())
            .ok_or_else(|| refused("not signed: X-Kraken-Push-Signature missing"))?;
        let mut mac = mac(&self.key);
        mac.update(format!("{at}\n").as_bytes());
        mac.update(body);
        // in constant time: the time taken tells nothing of the signature
        mac.verify_slice(&signature)
            .map_err(|_| refused("a wrong signature: not the key of the backend"))?;
        if (now - at).abs() > SKEW {
            return Err(refused(
                "signed too far from the clock of the backend: are both clocks right?",
            ));
        }
        let mut taken = self
            .taken
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        while taken.front().is_some_and(|(it, _)| *it < now - 2 * SKEW) {
            taken.pop_front();
        }
        if taken.iter().any(|(_, it)| *it == signature) {
            return Err((
                StatusCode::CONFLICT,
                "this push was taken already".to_string(),
            ));
        }
        taken.push_back((at, signature));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &[u8] = b"0123456789abcdef0123456789abcdef";

    fn headers(at: &str, signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AT, at.parse().unwrap());
        headers.insert(SIGNATURE, signature.parse().unwrap());
        headers
    }

    #[test]
    fn takes_a_push_signed_with_the_key_once() {
        let verifier = Verifier::new(KEY.to_vec());
        let body = b"{\"at\": 1}";
        let signed = headers("1000000", &sign(KEY, 1_000_000, body));
        assert_eq!(verifier.verify(&signed, body, 1_000_000), Ok(()));
        // heard on the way and sent again
        assert_eq!(
            verifier.verify(&signed, body, 1_000_001).unwrap_err().0,
            StatusCode::CONFLICT
        );
        // the next push
        let next = headers("1000500", &sign(KEY, 1_000_500, body));
        assert_eq!(verifier.verify(&next, body, 1_000_600), Ok(()));
    }

    #[test]
    fn refuses_what_isnt_signed_with_the_key() {
        let verifier = Verifier::new(KEY.to_vec());
        let body = b"{\"at\": 1}";
        let now = 1_000_000;
        for (headers, body) in [
            // another key
            (
                headers(
                    "1000000",
                    &sign(b"another key, long enough for it!", now, body),
                ),
                &body[..],
            ),
            // another body
            (
                headers("1000000", &sign(KEY, now, body)),
                &b"{\"at\": 2}"[..],
            ),
            // another time
            (headers("1000001", &sign(KEY, now, body)), &body[..]),
            // no signature
            (HeaderMap::new(), &body[..]),
            (headers("1000000", "md5=00"), &body[..]),
        ] {
            assert_eq!(
                verifier.verify(&headers, body, now).unwrap_err().0,
                StatusCode::UNAUTHORIZED
            );
        }
    }

    #[test]
    fn refuses_a_push_signed_too_long_ago_or_ahead() {
        let verifier = Verifier::new(KEY.to_vec());
        let body = b"{}";
        let now = 10_000_000;
        for at in [now - SKEW - 1, now + SKEW + 1] {
            let signed = headers(&at.to_string(), &sign(KEY, at, body));
            let (status, why) = verifier.verify(&signed, body, now).unwrap_err();
            assert_eq!(status, StatusCode::UNAUTHORIZED);
            assert!(why.contains("clock"), "{why}");
        }
        let at = now - SKEW;
        assert_eq!(
            verifier.verify(&headers(&at.to_string(), &sign(KEY, at, body)), body, now),
            Ok(())
        );
    }

    #[test]
    fn forgets_the_signatures_too_old_to_be_taken_again() {
        let verifier = Verifier::new(KEY.to_vec());
        for at in (0..100).map(|it| it * 60_000) {
            let signed = headers(&at.to_string(), &sign(KEY, at, b"{}"));
            verifier.verify(&signed, b"{}", at).unwrap();
        }
        // the last ten minutes and a bit
        assert!(verifier.taken.lock().unwrap().len() <= 12);
    }

    #[test]
    fn wants_a_long_key() {
        assert!(check_key("short").is_err());
        assert_eq!(
            check_key(" 0123456789abcdef0123456789abcdef ").unwrap(),
            KEY.to_vec()
        );
    }
}
