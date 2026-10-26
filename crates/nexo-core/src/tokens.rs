//! Tokens of each profile and other secrets (the Gemini key), in the OS
//! keychain; a file readable only by the owner where there is none
//! (`NEXO_TOKEN_STORE=file` forces it, e.g. in CI or containers).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::{Paths, write_private};
use crate::{Error, Result};

const SERVICE: &str = "nexo-cli";

/// The session the panel's person approved for this machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_token: String,
    pub refresh_expires_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl Tokens {
    /// The access token still has `margin` seconds to live.
    pub fn fresh(&self, margin: i64) -> bool {
        self.access_expires_at - chrono::Duration::seconds(margin) > Utc::now()
    }
}

#[derive(Debug, Clone)]
pub enum TokenStore {
    Keychain { fallback: std::path::PathBuf },
    File(std::path::PathBuf),
}

impl TokenStore {
    pub fn new(paths: &Paths) -> Self {
        let dir = paths.credentials_dir();
        if std::env::var("NEXO_TOKEN_STORE").is_ok_and(|v| v == "file") {
            Self::File(dir)
        } else {
            Self::Keychain { fallback: dir }
        }
    }

    pub fn load(&self, profile: &str) -> Result<Option<Tokens>> {
        match self.get(&token_account(profile))? {
            Some(json) => Ok(serde_json::from_str(&json).ok()),
            None => Ok(None),
        }
    }

    pub fn save(&self, profile: &str, tokens: &Tokens) -> Result<()> {
        self.set(&token_account(profile), &serde_json::to_string(tokens)?)
    }

    pub fn delete(&self, profile: &str) -> Result<()> {
        self.remove(&token_account(profile))
    }

    /// A named secret such as `gemini`.
    pub fn secret(&self, name: &str) -> Result<Option<String>> {
        self.get(&format!("secret:{name}"))
    }

    pub fn save_secret(&self, name: &str, value: &str) -> Result<()> {
        self.set(&format!("secret:{name}"), value)
    }

    pub fn delete_secret(&self, name: &str) -> Result<()> {
        self.remove(&format!("secret:{name}"))
    }

    /// Where secrets end up, for `nexo status`.
    pub fn describe(&self) -> &'static str {
        match self {
            Self::Keychain { .. } => "system keychain",
            Self::File(_) => "file (owner only)",
        }
    }

    fn get(&self, account: &str) -> Result<Option<String>> {
        match self {
            Self::Keychain { fallback } => {
                match keyring::Entry::new(SERVICE, account).and_then(|entry| entry.get_password()) {
                    Ok(value) => Ok(Some(value)),
                    Err(keyring::Error::NoEntry) => file_get(fallback, account),
                    Err(_) => file_get(fallback, account),
                }
            }
            Self::File(dir) => file_get(dir, account),
        }
    }

    fn set(&self, account: &str, value: &str) -> Result<()> {
        match self {
            Self::Keychain { fallback } => {
                match keyring::Entry::new(SERVICE, account).and_then(|e| e.set_password(value)) {
                    Ok(()) => {
                        // An older copy in the fallback file must not shadow it later.
                        let _ = file_remove(fallback, account);
                        Ok(())
                    }
                    Err(_) => file_set(fallback, account, value),
                }
            }
            Self::File(dir) => file_set(dir, account, value),
        }
    }

    fn remove(&self, account: &str) -> Result<()> {
        if let Self::Keychain { .. } = self {
            match keyring::Entry::new(SERVICE, account).and_then(|e| e.delete_credential()) {
                Ok(()) | Err(keyring::Error::NoEntry) => {}
                Err(e) => return Err(Error::Store(e.to_string())),
            }
        }
        let dir = match self {
            Self::Keychain { fallback } => fallback,
            Self::File(dir) => dir,
        };
        file_remove(dir, account)
    }
}

fn token_account(profile: &str) -> String {
    format!("profile:{profile}")
}

fn file_name(dir: &std::path::Path, account: &str) -> std::path::PathBuf {
    let safe: String = account
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join(format!("{safe}.json"))
}

fn file_get(dir: &std::path::Path, account: &str) -> Result<Option<String>> {
    match std::fs::read_to_string(file_name(dir, account)) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn file_set(dir: &std::path::Path, account: &str, value: &str) -> Result<()> {
    write_private(&file_name(dir, account), value.as_bytes())
}

fn file_remove(dir: &std::path::Path, account: &str) -> Result<()> {
    match std::fs::remove_file(file_name(dir, account)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(minutes: i64) -> Tokens {
        Tokens {
            access_token: "a".into(),
            access_expires_at: Utc::now() + chrono::Duration::minutes(minutes),
            refresh_token: "r".into(),
            refresh_expires_at: Utc::now() + chrono::Duration::days(30),
            email: Some("ktos@example.pl".into()),
        }
    }

    #[test]
    fn file_store_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let store = TokenStore::File(dir.path().join("credentials"));
        assert!(store.load("comfyroom").unwrap().is_none());
        store.save("comfyroom", &tokens(15)).unwrap();
        let loaded = store.load("comfyroom").unwrap().unwrap();
        assert_eq!(loaded.email.as_deref(), Some("ktos@example.pl"));
        store.delete("comfyroom").unwrap();
        assert!(store.load("comfyroom").unwrap().is_none());
        store.delete("comfyroom").unwrap();
    }

    #[test]
    fn freshness_has_a_margin() {
        assert!(tokens(15).fresh(30));
        assert!(!tokens(0).fresh(30));
    }
}
