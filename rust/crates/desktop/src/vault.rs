//! Encrypted credentials vault — wraps the platform-native secret store
//! (macOS Keychain, Windows Credential Manager, Linux Secret Service).
//!
//! Used for things that must never appear in plain text on disk:
//! - OAuth access / refresh tokens (Gmail, Slack, GitHub, …)
//! - PATs and API keys the user prefers not to keep in settings.json
//!
//! Each secret is namespaced under the `opc-desktop` service so it's
//! discoverable in Keychain Access and can be wiped by uninstalling.
//!
//! Why a thin wrapper around the `keyring` crate: the surface we actually
//! use is `set/get/delete/exists`, and standardising on Result<_, String>
//! keeps the rest of the desktop code free of yet-another error type.

const SERVICE: &str = "opc-desktop";

/// Store a secret. Overwrites any existing value at the same key.
pub fn store(key: &str, value: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
    entry.set_password(value).map_err(|e| e.to_string())
}

/// Retrieve a previously-stored secret. Returns `Ok(None)` if absent so
/// callers can distinguish "nothing set yet" from a real failure (broken
/// keyring, denied access).
pub fn load(key: &str) -> Result<Option<String>, String> {
    let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(s) => Ok(Some(s)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Remove a secret. Idempotent — missing entries are not an error.
pub fn delete(key: &str) -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE, key).map_err(|e| e.to_string())?;
    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On CI / headless environments the keyring will reject access. We
    /// only verify our wrappers don't panic and surface errors as
    /// `Err(String)` — not that the underlying platform store works.
    #[test]
    fn missing_key_returns_none_or_error() {
        // The key is intentionally unique-per-run so we don't collide with
        // anything a developer may have stored under the same SERVICE.
        let key = format!("test-{}", std::process::id());
        let result = load(&key);
        match result {
            Ok(None) => {} // happy path
            Ok(Some(_)) => panic!("unexpected stale value for fresh key"),
            // CI environments often have no keyring backend at all — that's
            // a legitimate error, not a bug in this wrapper.
            Err(_) => {}
        }
    }
}
