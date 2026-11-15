//! Shared wire format with seal.js: base64url(version || nonce || ciphertext || tag).
//! Timestamped plaintext uses purpose-specific AAD to bind replies and ordered frames to requests.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::hmac;
use ring::rand::SystemRandom;
use serde::{Deserialize, Serialize};

const SECRET_LEN: usize = 32;
const VERSION: u8 = 1;
const HEADER: usize = 1 + NONCE_LEN;
const STAMP: usize = 8;
const MAX_AGE_MS: u64 = 5 * 60 * 1000;

pub struct Keys {
    pub phone: String,
    key: LessSafeKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Call,
    Watch,
    /// The answer to the call sealed with this nonce.
    Reply([u8; NONCE_LEN]),
    /// Frame `n`, counting from 0, of the watch sealed with this nonce.
    Frame([u8; NONCE_LEN], u64),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Opened {
    pub text: String,
    pub nonce: [u8; NONCE_LEN],
    pub sealed_at: u64,
}

#[derive(Default, Serialize, Deserialize)]
pub struct Replays(Vec<(u64, [u8; NONCE_LEN])>);

pub fn new_secret() -> Result<String> {
    Ok(B64.encode(random::<SECRET_LEN>()?))
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let bytes = ring::rand::generate(&SystemRandom::new()).map_err(|_| anyhow!("no randomness"))?;
    Ok(bytes.expose())
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

impl Keys {
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
    pub fn check(&mut self, opened: &Opened, now: u64) -> Result<()> {
        let off = now.abs_diff(opened.sealed_at);
        ensure!(
            off <= MAX_AGE_MS,
            "the call was sealed {}s away from this Mac's time: it's a replay, or a clock is wrong",
            off / 1000
        );
        // Expired calls fail the time check, so their nonce records can be discarded.
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
}
