//! Backup secrets (S3 keys, WebDAV passwords, repository passphrases).
//!
//! They go to the OS credential store (Keychain, Windows Credential
//! Manager, Secret Service) when the user picks "Remember on this
//! computer", and otherwise live only in memory for the session. They are
//! never written to the database, settings, logs or a backup. Without a
//! usable credential store, "remember" silently degrades to the session:
//! the scheduler then asks again after a restart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

/// Name the secrets are filed under in the OS credential store.
const SERVICE: &str = "Printdex";

/// A credential store.
pub trait SecretBackend: Send + Sync + std::fmt::Debug {
    fn get(&self, key: &str) -> Result<Option<String>, String>;
    fn set(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete(&self, key: &str) -> Result<(), String>;
}

/// The OS credential store, through `keyring`.
#[derive(Debug)]
struct OsKeyring;

impl SecretBackend for OsKeyring {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
        match entry.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        keyring::Entry::new(SERVICE, key)
            .and_then(|e| e.set_password(value))
            .map_err(|e| e.to_string())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        match keyring::Entry::new(SERVICE, key).and_then(|e| e.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// An in-memory stand-in for the OS store (tests).
#[derive(Debug, Default)]
pub struct MemoryBackend(Mutex<HashMap<String, String>>);

impl SecretBackend for MemoryBackend {
    fn get(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.lock().get(key).cloned())
    }
    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        self.lock().insert(key.to_owned(), value.to_owned());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), String> {
        self.lock().remove(key);
        Ok(())
    }
}

impl MemoryBackend {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Every value stored (tests check no secret leaks elsewhere).
    pub fn values(&self) -> Vec<String> {
        self.lock().values().cloned().collect()
    }
}

/// Session secrets plus the (lazily opened) OS credential store.
#[derive(Debug, Default)]
pub struct Store {
    /// `None` until first use; then the OS store, if it works.
    backend: OnceLock<Option<Arc<dyn SecretBackend>>>,
    session: Mutex<HashMap<String, String>>,
}

impl Store {
    /// A store whose OS backend is replaced (tests), or absent.
    pub fn with_backend(backend: Option<Arc<dyn SecretBackend>>) -> Store {
        let store = Store::default();
        let _ = store.backend.set(backend);
        store
    }

    fn backend(&self) -> Option<&Arc<dyn SecretBackend>> {
        self.backend
            .get_or_init(|| {
                if std::env::var_os("PRINTDEX_KEYRING").is_some_and(|v| v == "off") {
                    return None;
                }
                match keyring::Entry::store_status() {
                    Ok(()) => Some(Arc::new(OsKeyring) as Arc<dyn SecretBackend>),
                    Err(e) => {
                        tracing::info!(error = %e, "no OS credential store; secrets last one session");
                        None
                    }
                }
            })
            .as_ref()
    }

    fn session(&self) -> std::sync::MutexGuard<'_, HashMap<String, String>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The secret under `key`: from this session, else the OS store.
    pub fn get(&self, key: &str) -> Option<String> {
        if let Some(v) = self.session().get(key) {
            return Some(v.clone());
        }
        let v = self.backend()?.get(key).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "reading the credential store failed");
            None
        })?;
        self.session().insert(key.to_owned(), v.clone());
        Some(v)
    }

    /// Keeps `value` for the session and, with `remember`, in the OS
    /// store. Returns whether it was remembered.
    pub fn set(&self, key: &str, value: &str, remember: bool) -> bool {
        self.session().insert(key.to_owned(), value.to_owned());
        let backend = self.backend();
        if !remember {
            if let Some(b) = backend {
                let _ = b.delete(key);
            }
            return false;
        }
        match backend.map(|b| b.set(key, value)) {
            Some(Ok(())) => true,
            Some(Err(e)) => {
                tracing::warn!(error = %e, "saving to the credential store failed");
                false
            }
            None => false,
        }
    }

    /// Forgets `key` everywhere.
    pub fn delete(&self, key: &str) {
        self.session().remove(key);
        if let Some(b) = self.backend() {
            let _ = b.delete(key);
        }
    }

    /// Whether secrets can outlive the session here.
    pub fn persistent(&self) -> bool {
        self.backend().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembered_secrets_survive_a_new_session() {
        let os: Arc<dyn SecretBackend> = Arc::new(MemoryBackend::default());
        let a = Store::with_backend(Some(os.clone()));
        assert!(a.set("k", "v", true));
        let b = Store::with_backend(Some(os.clone()));
        assert_eq!(b.get("k").as_deref(), Some("v"));
        // Not remembered: gone after the session, and removed from the OS.
        assert!(!b.set("k", "w", false));
        assert_eq!(b.get("k").as_deref(), Some("w"));
        assert_eq!(Store::with_backend(Some(os)).get("k"), None);
    }

    #[test]
    fn without_os_store_secrets_last_the_session() {
        let s = Store::with_backend(None);
        assert!(!s.persistent());
        assert!(!s.set("k", "v", true));
        assert_eq!(s.get("k").as_deref(), Some("v"));
        s.delete("k");
        assert_eq!(s.get("k"), None);
    }
}
