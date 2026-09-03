//! The one piece of state every handler shares: the already-connected,
//! already-schema-checked database handle.

use std::sync::Arc;

use payroll_app::SaltDatabase;

/// Cheap to clone (an `Arc` underneath), which is what letting `Router`
/// hand a copy to every request requires.
#[derive(Clone)]
pub struct AppState {
    db: Arc<SaltDatabase>,
}

impl AppState {
    pub fn new(db: SaltDatabase) -> Self {
        Self { db: Arc::new(db) }
    }

    pub(crate) fn db(&self) -> &SaltDatabase {
        &self.db
    }
}
