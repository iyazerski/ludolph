//! Web Push: tell the phone when a chat finishes, needs an answer, or fails,
//! even while the app is closed.
//!
//! The browser subscribes with the relay's VAPID key and hands back an
//! endpoint on its push service (FCM for Chrome). Notifications are encrypted
//! for the subscription (RFC 8291, `aes128gcm`, by `web-push-native`) and
//! signed with the VAPID key (RFC 8292, with `ring`). The key and the
//! subscriptions live in the data dir.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ludolph_relay_proto::Sealed;
use ring::rand::SystemRandom;
use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;
use web_push_native::Auth;
use web_push_native::p256::PublicKey;

use ludolph_json_file as json_file;

use crate::store::blocking;

/// Push services keep an undelivered notification this long.
const TTL_SECS: u64 = 24 * 3600;
/// The VAPID `sub` claim: a contact for the push service.
const CONTACT: &str = "https://github.com/iyazerski/ludolph";
/// How long a VAPID token stays valid; push services accept up to 24 h.
const VAPID_LIFETIME_SECS: u64 = 12 * 3600;
/// How long one request to a push service may take.
const PUSH_TIMEOUT: Duration = Duration::from_secs(15);
const AUTH_SECRET_LEN: usize = 16;

/// A browser's push subscription, as `PushSubscription.toJSON()` gives it,
/// with the ids of the phone links it holds keys for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    pub keys: SubscriptionKeys,
    /// A Mac's notice reaches this browser sealed for one of these.
    #[serde(default)]
    pub phones: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionKeys {
    /// The browser's P-256 public key, uncompressed, base64url.
    pub p256dh: String,
    /// The 16-byte authentication secret, base64url.
    pub auth: String,
}

/// What a notification carries; the service worker shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Payload {
    /// Sent from the app's settings to try notifications.
    Test { title: String, body: String },
    /// A Mac's notice, sealed for the phone: only it reads what happened.
    Notice {
        machine: String,
        /// The Mac's name, when there are several, so the phone says which.
        mac: Option<String>,
        sealed: Sealed,
    },
}

/// The VAPID key, the browsers' subscriptions, and delivery to them.
pub struct Push {
    vapid: EcdsaKeyPair,
    rng: SystemRandom,
    http: reqwest::Client,
    store: PathBuf,
    subscriptions: Mutex<Vec<Subscription>>,
}

impl Push {
    /// Load (or create) the VAPID key and the saved subscriptions in `dir`.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let rng = SystemRandom::new();
        let key_file = dir.join("vapid.pk8");
        let pkcs8 = match std::fs::read(&key_file) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let doc = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
                    .map_err(|_| anyhow!("generating the VAPID key"))?;
                json_file::save_bytes(&key_file, doc.as_ref())?;
                doc.as_ref().to_vec()
            }
            Err(e) => return Err(e).with_context(|| format!("reading {}", key_file.display())),
        };
        let vapid = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng)
            .map_err(|e| anyhow!("bad VAPID key in {}: {e}", key_file.display()))?;
        let store = dir.join("subscriptions.json");
        let subscriptions = json_file::load(&store)?.unwrap_or_default();
        // reqwest leaves the crypto provider to the app, and the scratch
        // image has no system roots: use ring and Mozilla's bundled roots.
        // An error only means a provider is already installed.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .map(|cert| reqwest::Certificate::from_der(cert))
            .collect::<reqwest::Result<Vec<_>>>()?;
        let http = reqwest::Client::builder()
            .timeout(PUSH_TIMEOUT)
            .tls_certs_only(roots)
            .build()?;
        Ok(Self {
            vapid,
            rng,
            http,
            store,
            subscriptions: Mutex::new(subscriptions),
        })
    }

    /// The VAPID public key the browser subscribes with, base64url.
    pub fn public_key(&self) -> String {
        B64.encode(self.vapid.public_key().as_ref())
    }

    /// Save `subscription`, replacing an older one for the same endpoint.
    pub async fn subscribe(&self, subscription: Subscription) -> Result<()> {
        // Reject keys that can't be used now, not at the first notification.
        decode_keys(&subscription.keys)?;
        self.update(|subs| {
            subs.retain(|s| s.endpoint != subscription.endpoint);
            subs.push(subscription);
        })
        .await
    }

    pub async fn unsubscribe(&self, endpoint: &str) -> Result<()> {
        self.update(|subs| subs.retain(|s| s.endpoint != endpoint))
            .await
    }

    /// Apply `change` to a copy of the subscriptions, save it, and only then
    /// make it current, so a failed save leaves them as the file has them.
    async fn update(&self, change: impl FnOnce(&mut Vec<Subscription>)) -> Result<()> {
        let mut subs = self.subscriptions.lock().await;
        let mut next = subs.clone();
        change(&mut next);
        self.save(&next)?;
        *subs = next;
        Ok(())
    }

    pub async fn send_to(&self, endpoint: &str, payload: &Payload) -> Result<()> {
        let sub = self
            .subscriptions
            .lock()
            .await
            .iter()
            .find(|s| s.endpoint == endpoint)
            .cloned()
            .context("this browser isn't subscribed")?;
        self.deliver(&sub, payload).await
    }

    /// Send each subscription its `payload`, if it has one, dropping ones the
    /// push service retired.
    pub async fn broadcast(&self, payload: impl Fn(&Subscription) -> Option<Payload>) {
        let subs = self.subscriptions.lock().await.clone();
        for sub in subs {
            let Some(payload) = payload(&sub) else {
                continue;
            };
            if let Err(e) = self.deliver(&sub, &payload).await {
                eprintln!("push to {}: {e:#}", short_endpoint(&sub.endpoint));
            }
        }
    }

    async fn deliver(&self, sub: &Subscription, payload: &Payload) -> Result<()> {
        let (ua_public, auth) = decode_keys(&sub.keys)?;
        let body = web_push_native::encrypt(serde_json::to_vec(payload)?, &ua_public, &auth)?;
        let url = reqwest::Url::parse(&sub.endpoint).context("bad push endpoint")?;
        let auth = self.vapid_header(&url.origin().ascii_serialization())?;
        let response = self
            .http
            .post(url)
            .header("Authorization", auth)
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .header("TTL", TTL_SECS.to_string())
            .header("Urgency", "high")
            .body(body)
            .send()
            .await?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        // 404/410: the browser unsubscribed or the subscription expired.
        if status == reqwest::StatusCode::NOT_FOUND || status == reqwest::StatusCode::GONE {
            self.unsubscribe(&sub.endpoint).await?;
            bail!("subscription expired ({status}); removed it");
        }
        let text = response.text().await.unwrap_or_default();
        bail!("push service answered {status}: {}", text.trim())
    }

    /// The `Authorization` header: a VAPID JWT for `audience`, plus the public key.
    fn vapid_header(&self, audience: &str) -> Result<String> {
        let exp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() + VAPID_LIFETIME_SECS;
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = B64.encode(serde_json::to_vec(
            &json!({ "aud": audience, "exp": exp, "sub": CONTACT }),
        )?);
        let signing_input = format!("{header}.{claims}");
        let signature = self
            .vapid
            .sign(&self.rng, signing_input.as_bytes())
            .map_err(|_| anyhow!("signing the VAPID token"))?;
        Ok(format!(
            "vapid t={signing_input}.{}, k={}",
            B64.encode(signature.as_ref()),
            self.public_key()
        ))
    }

    fn save(&self, subs: &[Subscription]) -> Result<()> {
        blocking(|| json_file::save(&self.store, &subs))
    }
}

fn decode_keys(keys: &SubscriptionKeys) -> Result<(PublicKey, Auth)> {
    // Browsers send unpadded base64url; tolerate padding anyway.
    let decode = |s: &str| B64.decode(s.trim_end_matches('='));
    let public = PublicKey::from_sec1_bytes(&decode(&keys.p256dh).context("bad p256dh key")?)
        .context("bad p256dh key")?;
    let auth = decode(&keys.auth).context("bad auth secret")?;
    if auth.len() != AUTH_SECRET_LEN {
        bail!("the auth secret isn't {AUTH_SECRET_LEN} bytes");
    }
    Ok((public, Auth::clone_from_slice(&auth)))
}

/// The push service's host, for log lines (the full endpoint is a secret).
fn short_endpoint(endpoint: &str) -> String {
    reqwest::Url::parse(endpoint)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_else(|| "push service".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[tokio::test]
    async fn subscribing_checks_the_browser_keys() {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::open(dir.path()).unwrap();
        // RFC 8291 appendix A's browser keys.
        let p256dh = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
        let sub = |p256dh: &str, auth: &str| Subscription {
            endpoint: "https://push.example/abc".into(),
            keys: SubscriptionKeys {
                p256dh: p256dh.into(),
                auth: auth.into(),
            },
            phones: Vec::new(),
        };
        push.subscribe(sub(p256dh, "BTBZMqHH6r4Tts7J_aSIgg=="))
            .await
            .unwrap();
        for (p256dh, auth) in [
            (&p256dh[..40], "BTBZMqHH6r4Tts7J_aSIgg"),
            (p256dh, "BTBZMqHH6r4T"),
        ] {
            assert!(push.subscribe(sub(p256dh, auth)).await.is_err());
        }
    }

    #[test]
    fn vapid_tokens_verify_with_the_public_key() {
        let dir = tempfile::tempdir().unwrap();
        let push = Push::open(dir.path()).unwrap();
        let header = push.vapid_header("https://fcm.googleapis.com").unwrap();
        let token = header
            .strip_prefix("vapid t=")
            .and_then(|r| r.split(", k=").next())
            .unwrap();
        let (input, signature) = token.rsplit_once('.').unwrap();
        let public = B64.decode(push.public_key()).unwrap();
        ring::signature::UnparsedPublicKey::new(&ring::signature::ECDSA_P256_SHA256_FIXED, public)
            .verify(input.as_bytes(), &B64.decode(signature).unwrap())
            .unwrap();
        let claims: Value =
            serde_json::from_slice(&B64.decode(input.split('.').nth(1).unwrap()).unwrap()).unwrap();
        assert_eq!(claims["aud"], "https://fcm.googleapis.com");
        // The key survives a restart, so existing subscriptions keep working.
        assert_eq!(
            Push::open(dir.path()).unwrap().public_key(),
            push.public_key()
        );
    }
}
