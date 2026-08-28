//! Stamps `SaltVersion` at build time: semver plus a short git SHA in one
//! string, e.g. `0.1.0+g1a2b3c4` (CONTEXT.md, `SaltVersion`). The result is
//! baked into the binary via the `SALT_VERSION` build-time environment
//! variable, read back by `lib.rs` through `env!`. Salt never reads git at
//! run time — a deployed binary has no repository.
//!
//! A **release** build refuses a dirty working tree: the recorded SHA would
//! otherwise name a commit the built binary does not actually match. A
//! debug build tolerates it, so ordinary development is not blocked.

use std::process::Command;

fn main() {
    let sha = git_output(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| {
        panic!("payroll-app must be built inside a git repository with at least one commit")
    });

    let dirty = !git_output(&["status", "--porcelain"])
        .unwrap_or_default()
        .is_empty();

    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile == "release" && dirty {
        panic!(
            "refusing a release build from a dirty working tree: the recorded \
             SaltVersion would name a commit the built binary does not match. \
             Commit or stash your changes, or build a debug profile instead."
        );
    }

    let version = env!("CARGO_PKG_VERSION");
    println!("cargo:rustc-env=SALT_VERSION={version}+g{sha}");

    // Rebuild only when the commit or the dirty/clean state can have
    // changed, not on every invocation. `--git-dir` finds the real `.git`
    // even if this crate ever moves relative to the workspace root.
    if let Some(git_dir) = git_output(&["rev-parse", "--git-dir"]) {
        println!("cargo:rerun-if-changed={git_dir}/HEAD");
        println!("cargo:rerun-if-changed={git_dir}/index");
    }
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|text| text.trim().to_string())
}
