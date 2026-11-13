//! The relay's saved state: passkeys, sign-in sessions, and Macs.
//!
//! One JSON file, rewritten whole on each change: there's one user and a
//! handful of records, so a database would be overkill. Sessions and Mac
//! tokens are kept as SHA-256 hashes, so the file alone can't sign anyone in.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use ludolph_json_file as json_file;
use ring::digest::{SHA256, digest};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use webauthn_rs::prelude::{AuthenticationResult, CredentialID, Passkey};

/// How long a sign-in lasts.
pub const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

#[derive(Clone, Default, Serialize, Deserialize)]
struct Data {
    /// The WebAuthn user handle; one user, made on first start.
    user: Option<Uuid>,
    #[serde(default)]
    passkeys: Vec<Passkey>,
    #[serde(default)]
    sessions: Vec<SessionRecord>,
    #[serde(default)]
    machines: Vec<MachineRecord>,
}

/// A sign-in session, by its token's hash.
#[derive(Clone, Serialize, Deserialize)]
struct SessionRecord {
    hash: String,
    /// Unix seconds.
    expires: u64,
}

/// A Mac, with its connection token's hash.
#[derive(Clone, Serialize, Deserialize)]
struct MachineRecord {
    id: String,
    name: String,
    token_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Machine {
    pub id: String,
    pub name: String,
}

impl From<&MachineRecord> for Machine {
    fn from(record: &MachineRecord) -> Self {
        Self {
            id: record.id.clone(),
            name: record.name.clone(),
        }
    }
}

/// The relay's state, in memory and in `<data dir>/state.json`.
pub struct Store {
    path: PathBuf,
    /// The WebAuthn user handle; fixed once the store opens.
    user: Uuid,
    data: Mutex<Data>,
    rng: SystemRandom,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("state.json");
        let mut data: Data = json_file::load(&path)?.unwrap_or_default();
        let fresh = data.user.is_none();
        let user = *data.user.get_or_insert_with(Uuid::new_v4);
        let store = Self {
            path,
            user,
            data: Mutex::new(data),
            rng: SystemRandom::new(),
        };
        if fresh {
            store.save(&store.lock())?;
        }
        Ok(store)
    }

    pub fn user(&self) -> Uuid {
        self.user
    }

    pub fn passkeys(&self) -> Vec<Passkey> {
        self.lock().passkeys.clone()
    }

    pub fn has_passkey(&self) -> bool {
        !self.lock().passkeys.is_empty()
    }

    pub fn credential_ids(&self) -> Vec<CredentialID> {
        self.lock()
            .passkeys
            .iter()
            .map(|p| p.cred_id().clone())
            .collect()
    }

    /// Save `passkey`, replacing an older one with its credential id.
    pub fn add_passkey(&self, passkey: Passkey) -> Result<()> {
        self.update(|data| {
            data.passkeys.retain(|p| p.cred_id() != passkey.cred_id());
            data.passkeys.push(passkey);
        })
    }

    /// Record a sign-in's new signature counter and backup state.
    pub fn update_passkeys(&self, result: &AuthenticationResult) -> Result<()> {
        self.update(|data| {
            for passkey in &mut data.passkeys {
                passkey.update_credential(result);
            }
        })
    }

    /// Start a session and return its cookie value.
    pub fn new_session(&self) -> Result<String> {
        let token = self.token()?;
        let now = now();
        self.update(|data| {
            data.sessions.retain(|s| s.expires > now);
            data.sessions.push(SessionRecord {
                hash: hash(&token),
                expires: now + SESSION_TTL.as_secs(),
            });
        })?;
        Ok(token)
    }

    /// Whether `token` is a live session's cookie value.
    pub fn session_valid(&self, token: &str) -> bool {
        let hash = hash(token);
        let now = now();
        self.lock()
            .sessions
            .iter()
            .any(|s| s.hash == hash && s.expires > now)
    }

    pub fn end_session(&self, token: &str) -> Result<()> {
        let hash = hash(token);
        self.update(|data| data.sessions.retain(|s| s.hash != hash))
    }

    pub fn machines(&self) -> Vec<Machine> {
        self.lock().machines.iter().map(Machine::from).collect()
    }

    pub fn machine_count(&self) -> usize {
        self.lock().machines.len()
    }

    /// Add a Mac named `name`; return it and its token (shown once).
    pub fn add_machine(&self, name: &str) -> Result<(Machine, String)> {
        let token = self.token()?;
        let machine = Machine {
            id: Uuid::new_v4().to_string(),
            name: name.to_owned(),
        };
        self.update(|data| {
            data.machines.push(MachineRecord {
                id: machine.id.clone(),
                name: machine.name.clone(),
                token_hash: hash(&token),
            });
        })?;
        Ok((machine, token))
    }

    /// Forget the Mac `id`; its token stops working.
    pub fn remove_machine(&self, id: &str) -> Result<()> {
        self.update(|data| data.machines.retain(|m| m.id != id))
    }

    pub fn has_machine(&self, id: &str) -> bool {
        self.lock().machines.iter().any(|m| m.id == id)
    }

    pub fn machine_for_token(&self, token: &str) -> Option<Machine> {
        let hash = hash(token);
        self.lock()
            .machines
            .iter()
            .find(|m| m.token_hash == hash)
            .map(Machine::from)
    }

    /// A fresh 256-bit secret, base64url.
    fn token(&self) -> Result<String> {
        let mut bytes = [0u8; 32];
        self.rng
            .fill(&mut bytes)
            .map_err(|_| anyhow!("generating a token"))?;
        Ok(B64.encode(bytes))
    }

    fn lock(&self) -> MutexGuard<'_, Data> {
        self.data.lock().expect("the store lock is poisoned")
    }

    /// Apply `change` to a copy of the state, save it, and only then make it
    /// current, so a failed save leaves the state as the file has it.
    fn update<R>(&self, change: impl FnOnce(&mut Data) -> R) -> Result<R> {
        let mut data = self.lock();
        let mut next = data.clone();
        let result = change(&mut next);
        self.save(&next)?;
        *data = next;
        Ok(result)
    }

    fn save(&self, data: &Data) -> Result<()> {
        blocking(|| json_file::save(&self.path, data))
    }
}

/// A token's SHA-256, base64url: what the file keeps instead of the token.
fn hash(token: &str) -> String {
    B64.encode(digest(&SHA256, token.as_bytes()))
}

/// Run blocking file work without stalling the runtime's other tasks: on a
/// multi-threaded runtime, this worker hands its tasks to another first.
pub fn blocking<R>(work: impl FnOnce() -> R) -> R {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current().map(|h| h.runtime_flavor()) {
        Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(work),
        _ => work(),
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_and_machine_tokens_survive_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let session = store.new_session().unwrap();
        let (mac, token) = store.add_machine("Work Mac").unwrap();
        let user = store.user();

        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.user(), user);
        assert!(store.session_valid(&session));
        assert!(!store.session_valid("guess"));
        assert_eq!(store.machine_for_token(&token), Some(mac.clone()));
        assert_eq!(store.machine_for_token("guess"), None);

        // Only hashes are saved.
        let saved = std::fs::read_to_string(dir.path().join("state.json")).unwrap();
        assert!(!saved.contains(&session) && !saved.contains(&token));

        store.end_session(&session).unwrap();
        assert!(!store.session_valid(&session));
        store.remove_machine(&mac.id).unwrap();
        assert_eq!(store.machine_for_token(&token), None);
    }

    #[test]
    fn a_change_that_isnt_saved_doesnt_apply() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let (mac, _) = store.add_machine("Work Mac").unwrap();
        let read_only = std::fs::Permissions::from_mode(0o500);
        std::fs::set_permissions(dir.path(), read_only).unwrap();
        assert!(store.remove_machine(&mac.id).is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        // Still added, as the file says.
        assert!(store.has_machine(&mac.id));
    }
}
