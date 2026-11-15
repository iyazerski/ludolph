use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::Serialize;
use url::Url;
use webauthn_rs::prelude::{
    CreationChallengeResponse, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, RequestChallengeResponse, Webauthn, WebauthnBuilder,
};

use crate::store::{SESSION_TTL, Store};

/// __Host- cookies require Secure, Path=/ and no Domain to block subdomain cookie injection.
const COOKIE: &str = "__Host-ludolph";
const FLOW_TTL: Duration = Duration::from_secs(300);
const MAX_FLOWS: usize = 64;

enum Flow {
    Register(PasskeyRegistration),
    Login(PasskeyAuthentication),
}

struct Pending {
    at: Instant,
    flow: Flow,
}

pub struct Auth {
    webauthn: Webauthn,
    flows: Mutex<HashMap<String, Pending>>,
}

#[derive(Serialize)]
pub struct Started<T> {
    pub flow: String,
    pub options: T,
}

impl Auth {
    pub fn new(public_url: &Url) -> Result<Self> {
        let rp_id = public_url
            .host_str()
            .context("the public URL has no host")?;
        let webauthn = WebauthnBuilder::new(rp_id, public_url)?
            .rp_name("Ludolph")
            .build()?;
        Ok(Self {
            webauthn,
            flows: Mutex::new(HashMap::new()),
        })
    }

    pub fn start_register(&self, store: &Store) -> Result<Started<CreationChallengeResponse>> {
        let existing = store.credential_ids();
        let (options, state) = self.webauthn.start_passkey_registration(
            store.user(),
            "ludolph",
            "Ludolph",
            Some(existing),
        )?;
        Ok(Started {
            flow: self.keep(Flow::Register(state)),
            options,
        })
    }

    pub fn finish_register(
        &self,
        store: &Store,
        flow: &str,
        credential: &RegisterPublicKeyCredential,
    ) -> Result<()> {
        let Some(Flow::Register(state)) = self.take(flow) else {
            bail!("this passkey request expired; try again");
        };
        let passkey = self
            .webauthn
            .finish_passkey_registration(credential, &state)
            .map_err(|e| anyhow!("the passkey wasn't accepted: {e}"))?;
        store.add_passkey(passkey)
    }

    pub fn start_login(&self, store: &Store) -> Result<Started<RequestChallengeResponse>> {
        let passkeys = store.passkeys();
        if passkeys.is_empty() {
            bail!("no passkey yet: open the setup link first");
        }
        let (options, state) = self.webauthn.start_passkey_authentication(&passkeys)?;
        Ok(Started {
            flow: self.keep(Flow::Login(state)),
            options,
        })
    }

    pub fn finish_login(
        &self,
        store: &Store,
        flow: &str,
        credential: &PublicKeyCredential,
    ) -> Result<()> {
        let Some(Flow::Login(state)) = self.take(flow) else {
            bail!("this sign-in expired; try again");
        };
        let result = self
            .webauthn
            .finish_passkey_authentication(credential, &state)
            .map_err(|e| anyhow!("the passkey wasn't accepted: {e}"))?;
        store.update_passkeys(&result)
    }

    fn keep(&self, flow: Flow) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let mut flows = self.lock();
        flows.retain(|_, pending| pending.at.elapsed() < FLOW_TTL);
        while flows.len() >= MAX_FLOWS {
            let Some(oldest) = flows
                .iter()
                .min_by_key(|(_, pending)| pending.at)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            flows.remove(&oldest);
        }
        let pending = Pending {
            at: Instant::now(),
            flow,
        };
        flows.insert(id.clone(), pending);
        id
    }

    fn take(&self, id: &str) -> Option<Flow> {
        let pending = self.lock().remove(id)?;
        (pending.at.elapsed() < FLOW_TTL).then_some(pending.flow)
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<String, Pending>> {
        self.flows
            .lock()
            .expect("the sign-in flows lock is poisoned")
    }
}

pub fn session_cookie(jar: &CookieJar) -> Option<&str> {
    jar.get(COOKIE).map(Cookie::value)
}

pub fn set_cookie(jar: CookieJar, token: String) -> CookieJar {
    let max_age = time::Duration::seconds(SESSION_TTL.as_secs().cast_signed());
    jar.add(session(token, max_age))
}

pub fn clear_cookie(jar: CookieJar) -> CookieJar {
    jar.remove(session(String::new(), time::Duration::ZERO))
}

fn session(value: String, max_age: time::Duration) -> Cookie<'static> {
    Cookie::build((COOKIE, value))
        .max_age(max_age)
        .path("/")
        .secure(true)
        .http_only(true)
        .same_site(SameSite::Strict)
        .build()
}
