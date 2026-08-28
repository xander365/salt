//! The stateful application layer for Salt, built on the pure `payroll`
//! crate (ADR-0009). `payroll-app` depends on `payroll`; the reverse
//! dependency does not exist.
//!
//! INV-001 (money is exact decimal, never `f32`/`f64`) reaches this crate
//! too: the ban is declared once in the workspace root's `Cargo.toml`
//! under `[workspace.lints]`, so a member cannot opt out of it by
//! forgetting an attribute.

mod error;

pub use error::PayrollAppError;

/// The Salt release that produced this build: semver plus a short git SHA in
/// one string, e.g. `0.1.0+g1a2b3c4` (CONTEXT.md, `SaltVersion`). Lets a
/// future maintainer find the exact code behind a historical figure.
///
/// Captured once by `build.rs` at build time and baked in here; never read
/// from git at run time, because a deployed binary has no repository. A
/// release build from a dirty working tree fails rather than record a SHA
/// the binary does not match.
pub const SALT_VERSION: &str = env!("SALT_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_version_is_semver_plus_a_short_git_sha() {
        let (semver, sha_part) = SALT_VERSION
            .split_once('+')
            .expect("SaltVersion must carry a build-metadata segment introduced by '+'");

        assert_eq!(
            semver.chars().filter(|&c| c == '.').count(),
            2,
            "expected a semver like 0.1.0, got {semver}"
        );
        assert!(
            sha_part.starts_with('g'),
            "expected the git SHA segment to start with 'g', got {sha_part}"
        );
        assert_eq!(
            sha_part.len(),
            9,
            "expected 'g' plus an 8-character short SHA, got {sha_part}"
        );
    }
}
