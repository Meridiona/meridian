// meridian — normalises screenpipe activity into structured app sessions
//
// PKCE (RFC 7636) helpers for the OAuth 2.0 Authorization Code flow used by the
// browser-based PM provider auth (Jira, Linear). Public desktop clients can't
// ship a client secret, so we prove possession of the authorization code with a
// per-flow code_verifier / code_challenge pair instead.

use base64::Engine as _;
use sha2::{Digest, Sha256};

/// A freshly generated PKCE pair plus an anti-CSRF `state` token. Each browser
/// flow creates exactly one — the verifier is kept in-process and presented at
/// token exchange; the challenge is what travels through the browser.
pub struct Pkce {
    /// The high-entropy secret kept in memory and sent only at token exchange.
    pub verifier: String,
    /// `BASE64URL(SHA256(verifier))` — sent on the `/authorize` request.
    pub challenge: String,
    /// Opaque anti-CSRF value echoed back on the redirect; we reject a mismatch.
    pub state: String,
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Generate a PKCE pair (S256) and a random `state`. 32 random bytes → a 43-char
/// base64url verifier, comfortably inside RFC 7636's 43–128 char range.
pub fn generate() -> Pkce {
    let verifier_bytes: [u8; 32] = rand::random();
    let verifier = b64url(&verifier_bytes);

    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let challenge = b64url(&hasher.finalize());

    let state_bytes: [u8; 16] = rand::random();
    let state = b64url(&state_bytes);

    Pkce {
        verifier,
        challenge,
        state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_43_chars_and_url_safe() {
        let p = generate();
        assert_eq!(p.verifier.len(), 43, "32 bytes → 43 base64url chars");
        assert!(
            p.verifier
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "verifier must be URL-safe with no padding: {}",
            p.verifier
        );
    }

    #[test]
    fn challenge_is_sha256_of_verifier() {
        let p = generate();
        // Recompute the challenge independently and compare.
        let mut h = Sha256::new();
        h.update(p.verifier.as_bytes());
        let expected = b64url(&h.finalize());
        assert_eq!(p.challenge, expected);
        assert_eq!(p.challenge.len(), 43);
    }

    #[test]
    fn each_call_is_unique() {
        let a = generate();
        let b = generate();
        assert_ne!(a.verifier, b.verifier);
        assert_ne!(a.state, b.state);
    }
}
