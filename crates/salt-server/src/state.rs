//! The state every handler shares: the already-connected,
//! already-schema-checked database handle, and the one fact about
//! configuration a handler needs to answer a request rather than to start up
//! — whether the session cookie carries `Secure`.

use std::sync::Arc;

use payroll_app::SaltDatabase;

/// Cheap to clone (an `Arc` underneath), which is what letting `Router`
/// hand a copy to every request requires.
#[derive(Clone)]
pub struct AppState {
    db: Arc<SaltDatabase>,
    secure_cookies: bool,
}

impl AppState {
    /// `secure_cookies` is the inverse of `ServerConfig::insecure_cookies` —
    /// carried as its own plain `bool` rather than the whole `ServerConfig`,
    /// so a handler reaching for it cannot also reach for a database URL or
    /// a bind address it has no business touching.
    pub fn new(db: SaltDatabase, secure_cookies: bool) -> Self {
        Self {
            db: Arc::new(db),
            secure_cookies,
        }
    }

    pub(crate) fn db(&self) -> &SaltDatabase {
        &self.db
    }

    /// Whether the session cookie the login and logout routes set must carry
    /// `Secure` — on by default, and off only under the development flag
    /// `ServerConfig::insecure_cookies` names, which cannot itself be `true`
    /// in production (`config.rs`'s own refusal at startup).
    pub(crate) fn secure_cookies(&self) -> bool {
        self.secure_cookies
    }
}
