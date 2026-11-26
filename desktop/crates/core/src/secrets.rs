//! The board token outside settings.json: Windows Credential Manager ("Desk Hub", "board-token").
//! The token stays in memory in `Settings.board.token` (the session and HTTP calls use it);
//! only the copy written to disk is blanked once the credential store holds it. Elsewhere
//! (Linux builds, tests) there is no store and the file keeps the token as before.

use anyhow::Result;

use crate::settings::Settings;

pub trait SecretStore: Send + Sync {
    fn get(&self) -> Result<Option<String>>;
    fn set(&self, value: &str) -> Result<()>;
    /// Removes the secret; a missing one is not an error.
    fn delete(&self) -> Result<()>;
}

/// No credential store: the settings file keeps the token.
pub struct NoStore;

impl SecretStore for NoStore {
    fn get(&self) -> Result<Option<String>> {
        Ok(None)
    }
    fn set(&self, _: &str) -> Result<()> {
        anyhow::bail!("no credential store on this platform")
    }
    fn delete(&self) -> Result<()> {
        Ok(())
    }
}

/// Credential names under the "Desk Hub" service.
pub const BOARD_TOKEN: &str = "board-token";
/// The plex.tv account token from "Sign in with Plex".
pub const PLEX_TOKEN: &str = "plex-token";
/// The token of a deskhub-relay (Claude Code on a server, Settings > AI).
pub const REMOTE_TOKEN: &str = "remote-claude-token";
/// A GitHub fine-grained personal access token (contribution chart, Settings > Calendar & GitHub).
pub const GITHUB_TOKEN: &str = "github-token";

#[cfg(windows)]
pub struct CredentialManager {
    pub user: &'static str,
}

#[cfg(windows)]
impl CredentialManager {
    fn entry(&self) -> Result<keyring::Entry> {
        Ok(keyring::Entry::new("Desk Hub", self.user)?)
    }
}

#[cfg(windows)]
impl SecretStore for CredentialManager {
    fn get(&self) -> Result<Option<String>> {
        match self.entry()?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn set(&self, value: &str) -> Result<()> {
        Ok(self.entry()?.set_password(value)?)
    }
    fn delete(&self) -> Result<()> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// The platform's store for the board token: Credential Manager on Windows, none elsewhere.
pub fn system_store() -> Box<dyn SecretStore> {
    system_store_for(BOARD_TOKEN)
}

/// The platform's store for one named secret.
pub fn system_store_for(user: &'static str) -> Box<dyn SecretStore> {
    #[cfg(windows)]
    {
        Box::new(CredentialManager { user })
    }
    #[cfg(not(windows))]
    {
        let _ = user;
        Box::new(NoStore)
    }
}

/// Stores a secret and reads it back: only a confirmed write counts.
pub fn store_verified(store: &dyn SecretStore, value: &str) -> Result<()> {
    store.set(value)?;
    match store.get()? {
        Some(v) if v == value => Ok(()),
        _ => anyhow::bail!("the credential store did not keep the value"),
    }
}

/// After loading settings.json: a file without a token takes it from the store.
pub fn load_token(store: &dyn SecretStore, s: &mut Settings) {
    if s.board.token.is_empty() {
        match store.get() {
            Ok(Some(t)) => s.board.token = t,
            Ok(None) => {}
            Err(e) => log::warn!("credential store: {e:#}"),
        }
    }
}

/// What to write to settings.json: the token goes to the store and the file copy is blank,
/// but only once the store reads it back. If the store fails, the file keeps the token.
/// An empty token never reaches the store: a store that failed to answer at start would
/// otherwise lose the real token on the next save.
pub fn for_file(store: &dyn SecretStore, s: &Settings) -> Settings {
    let mut out = s.clone();
    let token = &s.board.token;
    if token.is_empty() {
        return out;
    }
    let stored = match store.get() {
        Ok(Some(t)) if &t == token => true,
        Ok(_) => store.set(token).is_ok() && matches!(store.get(), Ok(Some(t)) if &t == token),
        Err(_) => false,
    };
    if stored {
        out.board.token.clear();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Mem(Mutex<Option<String>>);
    impl SecretStore for Mem {
        fn get(&self) -> Result<Option<String>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn set(&self, v: &str) -> Result<()> {
            *self.0.lock().unwrap() = Some(v.to_string());
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
    }

    /// Writes are lost: set succeeds but nothing reads back.
    struct Leaky;
    impl SecretStore for Leaky {
        fn get(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn set(&self, _: &str) -> Result<()> {
            Ok(())
        }
        fn delete(&self) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn store_verified_needs_the_read_back() {
        let store = Mem::default();
        store_verified(&store, "plex").unwrap();
        assert_eq!(store.get().unwrap().as_deref(), Some("plex"));
        assert!(store_verified(&Leaky, "plex").is_err());
        assert!(store_verified(&NoStore, "plex").is_err());
        store.delete().unwrap();
        assert_eq!(store.get().unwrap(), None);
    }

    fn with_token(t: &str) -> Settings {
        let mut s = Settings::default();
        s.board.token = t.into();
        s
    }

    #[test]
    fn moves_the_file_token_into_the_store() {
        let store = Mem::default();
        let mut s = with_token("secret");
        load_token(&store, &mut s);
        assert_eq!(s.board.token, "secret");
        let file = for_file(&store, &s);
        assert_eq!(file.board.token, "");
        assert_eq!(store.get().unwrap().as_deref(), Some("secret"));
        // In memory the token stays.
        assert_eq!(s.board.token, "secret");

        // Next start: the file has no token, the store fills it in.
        let mut next = file;
        load_token(&store, &mut next);
        assert_eq!(next.board.token, "secret");
    }

    #[test]
    fn a_changed_token_replaces_the_stored_one() {
        let store = Mem::default();
        store.set("old").unwrap();
        let file = for_file(&store, &with_token("new"));
        assert_eq!(file.board.token, "");
        assert_eq!(store.get().unwrap().as_deref(), Some("new"));
    }

    #[test]
    fn without_a_working_store_the_file_keeps_the_token() {
        assert_eq!(
            for_file(&NoStore, &with_token("secret")).board.token,
            "secret"
        );
        assert_eq!(
            for_file(&Leaky, &with_token("secret")).board.token,
            "secret"
        );
        let mut s = with_token("");
        load_token(&NoStore, &mut s);
        assert_eq!(s.board.token, "");
    }

    #[test]
    fn an_empty_token_never_overwrites_the_store() {
        let store = Mem::default();
        store.set("secret").unwrap();
        assert_eq!(for_file(&store, &with_token("")).board.token, "");
        assert_eq!(store.get().unwrap().as_deref(), Some("secret"));
    }

    #[test]
    fn a_file_token_wins_over_the_store_on_load() {
        let store = Mem::default();
        store.set("stored").unwrap();
        let mut s = with_token("typed-into-the-file");
        load_token(&store, &mut s);
        assert_eq!(s.board.token, "typed-into-the-file");
    }
}
