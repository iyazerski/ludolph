//! Sealing what a phone and this Mac say, so the relay passes it on without
//! reading or changing it (`docs/design.md`, "Trust").
//!
//! Linking a phone makes a random secret, which reaches the phone only in the
//! QR code's URL fragment. It gives two values, each an HMAC-SHA256 of a label
//! keyed with the secret: the phone's id, which the relay routes by, and the
//! sealing key, which only the phone and this Mac know. A sealed message is
//! AES-256-GCM: `version || nonce || ciphertext || tag`, in base64url without
//! padding, and its text starts with the time it was sealed. The associated
//! data says what the message is ([`Purpose`]), so the relay can't pass one
//! off as another: a reply opens only as the answer to its call, and a frame
//! only in its own stream, in order. The phone app does the same in `seal.js`;
//! the test vectors here and there match.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::hmac;
use ring::rand::SystemRandom;

/// The secret's length in bytes.
const SECRET_LEN: usize = 32;
/// The first byte of a sealed message; bumped if the format changes.
const VERSION: u8 = 1;
/// The version and the nonce, before the ciphertext.
const HEADER: usize = 1 + NONCE_LEN;
/// The sealing time's length: milliseconds since the Unix epoch, big-endian.
const STAMP: usize = 8;
/// How far a call's sealing time may be from this Mac's clock. It covers a
/// slow network and clocks that drift apart.
const MAX_AGE_MS: u64 = 5 * 60 * 1000;

/// One phone's id and sealing key.
pub struct Keys {
    /// The phone's id, which the relay sees.
    pub phone: String,
    key: LessSafeKey,
}

/// What a sealed message is, which its associated data names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// A tool call from the phone.
    Call,
    /// The phone asks to follow a chat.
    Watch,
    /// The answer to the call sealed with this nonce.
    Reply([u8; NONCE_LEN]),
    /// Frame `n`, counting from 0, of the watch sealed with this nonce.
    Frame([u8; NONCE_LEN], u64),
    /// A notification.
    Notice,
}

impl Purpose {
    fn aad(&self) -> Vec<u8> {
        match self {
            Self::Call => b"ludolph/call".to_vec(),
            Self::Watch => b"ludolph/watch".to_vec(),
            Self::Reply(nonce) => [b"ludolph/reply".as_slice(), nonce].concat(),
            Self::Frame(nonce, n) => {
                [b"ludolph/frame".as_slice(), nonce, &n.to_be_bytes()].concat()
            }
            Self::Notice => b"ludolph/notice".to_vec(),
        }
    }
}

/// An opened message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub text: String,
    /// Its nonce, which a reply or a frame is bound to.
    pub nonce: [u8; NONCE_LEN],
    /// When it was sealed, in ms since the Unix epoch.
    pub sealed_at: u64,
}

/// The calls opened lately, so that none runs twice.
#[derive(Default)]
pub struct Replays(Vec<(u64, [u8; NONCE_LEN])>);

/// Make a new random secret, in base64url.
///
/// # Errors
///
/// Fails if the system has no randomness to give.
pub fn new_secret() -> Result<String> {
    Ok(B64.encode(random::<SECRET_LEN>()?))
}

/// `N` random bytes from the system.
fn random<const N: usize>() -> Result<[u8; N]> {
    // ring's error says nothing more than that it failed.
    let bytes = ring::rand::generate(&SystemRandom::new()).map_err(|_| anyhow!("no randomness"))?;
    Ok(bytes.expose())
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Keys {
    /// Derive the phone's id and sealing key from `secret`.
    ///
    /// # Errors
    ///
    /// Fails if `secret` isn't 32 bytes in base64url.
    pub fn derive(secret: &str) -> Result<Self> {
        let secret = B64
            .decode(secret.trim())
            .ok()
            .filter(|s| s.len() == SECRET_LEN)
            .context("a phone's secret should be 32 bytes in base64url")?;
        let secret = hmac::Key::new(hmac::HMAC_SHA256, &secret);
        let phone = B64.encode(hmac::sign(&secret, b"ludolph/phone"));
        let key = UnboundKey::new(&AES_256_GCM, hmac::sign(&secret, b"ludolph/key").as_ref())
            .map_err(|_| anyhow!("bad key length"))?;
        Ok(Self {
            phone,
            key: LessSafeKey::new(key),
        })
    }

    /// Seal `text` for `purpose` with a fresh random nonce and the current
    /// time. Returns the sealed message and its nonce.
    ///
    /// # Errors
    ///
    /// Fails if the system has no randomness to give.
    pub fn seal(&self, purpose: Purpose, text: &str) -> Result<(String, [u8; NONCE_LEN])> {
        let nonce = random()?;
        Ok((self.seal_with(nonce, now_ms(), purpose, text)?, nonce))
    }

    fn seal_with(
        &self,
        nonce: [u8; NONCE_LEN],
        sealed_at: u64,
        purpose: Purpose,
        text: &str,
    ) -> Result<String> {
        let tag_len = self.key.algorithm().tag_len();
        let mut sealed = Vec::with_capacity(HEADER + STAMP + text.len() + tag_len);
        sealed.push(VERSION);
        sealed.extend_from_slice(&nonce);
        sealed.extend_from_slice(&sealed_at.to_be_bytes());
        sealed.extend_from_slice(text.as_bytes());
        let tag = self
            .key
            .seal_in_place_separate_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(purpose.aad()),
                &mut sealed[HEADER..],
            )
            .map_err(|_| anyhow!("sealing failed"))?;
        sealed.extend_from_slice(tag.as_ref());
        Ok(B64.encode(sealed))
    }

    /// Open a message sealed for `purpose`.
    ///
    /// # Errors
    ///
    /// Fails if it was sealed with another key or for another purpose, or
    /// was changed.
    pub fn open(&self, purpose: Purpose, sealed: &str) -> Result<Opened> {
        let mut sealed = B64.decode(sealed).context("not base64url")?;
        let Some(&version) = sealed.first() else {
            bail!("an empty message");
        };
        ensure!(
            version == VERSION,
            "message format {version} is unknown; update Ludolph"
        );
        ensure!(sealed.len() >= HEADER, "a message too short");
        let nonce: [u8; NONCE_LEN] = sealed[1..HEADER].try_into().context("bad nonce")?;
        // Decrypts in place and moves the text to the front of the buffer.
        let len = self
            .key
            .open_within(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(purpose.aad()),
                &mut sealed,
                HEADER..,
            )
            .map_err(|_| anyhow!("the message doesn't open: link this phone again"))?
            .len();
        sealed.truncate(len);
        let (stamp, _) = sealed
            .split_first_chunk::<STAMP>()
            .context("a message too short")?;
        let sealed_at = u64::from_be_bytes(*stamp);
        sealed.drain(..STAMP);
        let text = String::from_utf8(sealed).context("the message isn't UTF-8")?;
        Ok(Opened {
            text,
            nonce,
            sealed_at,
        })
    }
}

impl Replays {
    /// Accept `opened` at `now` (ms since the Unix epoch) once, if it was
    /// sealed recently enough.
    ///
    /// # Errors
    ///
    /// Fails if it was sealed too far from `now`, or came before.
    pub fn check(&mut self, opened: &Opened, now: u64) -> Result<()> {
        let off = now.abs_diff(opened.sealed_at);
        ensure!(
            off <= MAX_AGE_MS,
            "the call was sealed {}s away from this Mac's time: it's a replay, or a clock is wrong",
            off / 1000
        );
        // Older entries can't match: their calls fail the time check above.
        self.0.retain(|(at, _)| now.abs_diff(*at) <= MAX_AGE_MS);
        ensure!(
            !self.0.iter().any(|(_, seen)| *seen == opened.nonce),
            "the call came before: it's a replay"
        );
        self.0.push((opened.sealed_at, opened.nonce));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shared with the phone app's seal.js: change both together.
    const SECRET: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8";
    const PHONE: &str = "qf1syN8L0ri6_W_o_Q6pYdOXB4FnBEZyH7IKLMUOCq0";
    const SEALED_AT: u64 = 1_700_000_000_000;
    const SEALED: &str = "AQcHBwcHBwcHBwcHB-rmwF5EwJQr_SVTFIOQC9hXniqNm4CHwM_pJifdcQ9PK6fv";

    #[test]
    fn the_test_vectors_hold() {
        let keys = Keys::derive(SECRET).unwrap();
        let reply = Purpose::Reply([9; NONCE_LEN]);
        let sealed = keys
            .seal_with([7; NONCE_LEN], SEALED_AT, reply, "héllo 👋")
            .unwrap();
        assert_eq!(keys.phone, PHONE);
        assert_eq!(sealed, SEALED);
        let opened = keys.open(reply, SEALED).unwrap();
        assert_eq!(
            opened,
            Opened {
                text: "héllo 👋".into(),
                nonce: [7; NONCE_LEN],
                sealed_at: SEALED_AT
            }
        );
    }

    #[test]
    fn only_the_same_key_and_purpose_open_a_message() {
        let keys = Keys::derive(&new_secret().unwrap()).unwrap();
        let (sealed, nonce) = keys
            .seal(Purpose::Frame([1; NONCE_LEN], 3), "text")
            .unwrap();
        assert_ne!(nonce, keys.seal(Purpose::Call, "text").unwrap().1);
        let opened = keys
            .open(Purpose::Frame([1; NONCE_LEN], 3), &sealed)
            .unwrap();
        assert_eq!((opened.text.as_str(), opened.nonce), ("text", nonce));
        for other in [
            Purpose::Frame([1; NONCE_LEN], 4),
            Purpose::Frame([2; NONCE_LEN], 3),
            Purpose::Call,
        ] {
            assert!(keys.open(other, &sealed).is_err(), "{other:?}");
        }
        let stranger = Keys::derive(&new_secret().unwrap()).unwrap();
        assert_ne!(stranger.phone, keys.phone);
        let frame = Purpose::Frame([1; NONCE_LEN], 3);
        assert!(stranger.open(frame, &sealed).is_err());
        let mut tampered = sealed.into_bytes();
        let last = tampered.len() - 2;
        tampered[last] = if tampered[last] == b'A' { b'B' } else { b'A' };
        assert!(
            keys.open(frame, std::str::from_utf8(&tampered).unwrap())
                .is_err()
        );
    }

    #[test]
    fn a_call_runs_once_and_only_while_fresh() {
        let keys = Keys::derive(SECRET).unwrap();
        let sealed = keys
            .seal_with([7; NONCE_LEN], SEALED_AT, Purpose::Call, "{}")
            .unwrap();
        let opened = keys.open(Purpose::Call, &sealed).unwrap();
        let mut replays = Replays::default();
        let late = SEALED_AT + MAX_AGE_MS + 1;
        assert!(replays.check(&opened, late).is_err());
        assert!(replays.check(&opened, SEALED_AT - MAX_AGE_MS - 1).is_err());
        assert!(replays.check(&opened, SEALED_AT + 1000).is_ok());
        assert!(replays.check(&opened, SEALED_AT + 2000).is_err());
    }

    #[test]
    fn bad_secrets_are_refused() {
        assert!(Keys::derive("short").is_err());
        assert!(Keys::derive(&"A".repeat(43)).is_ok());
    }
}
