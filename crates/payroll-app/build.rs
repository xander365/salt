//! Stamps `SaltVersion` at build time: semver plus a short git SHA in one
//! string, e.g. `0.1.0+g1a2b3c4` (CONTEXT.md, `SaltVersion`). The result is
//! baked into the binary via the `SALT_VERSION` build-time environment
//! variable, read back by `lib.rs` through `env!`. Salt never reads git at
//! run time — a deployed binary has no repository.
//!
//! A **release** build refuses a dirty working tree: the recorded SHA would
//! otherwise name a commit the built binary does not actually match. A
//! debug build tolerates it, so ordinary development is not blocked.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let workspace_root = git_output(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| {
        panic!("payroll-app must be built inside a git repository: `git rev-parse` failed")
    });
    let sha = git_output(&["rev-parse", "--short=8", "HEAD"]).unwrap_or_else(|| {
        panic!("payroll-app must be built from a git repository with at least one commit")
    });

    watch_everything_that_can_change_the_answer(&workspace_root);

    let dirty = !git_output(&["status", "--porcelain"])
        .unwrap_or_default()
        .is_empty();

    let profile = std::env::var("PROFILE").unwrap_or_default();
    if profile == "release" && dirty {
        panic!(
            "refusing a release build from a dirty working tree: the recorded \
             SaltVersion would name a commit the built binary does not match. \
             Commit, stash or remove your changes — untracked files count too \
             — or build a debug profile instead."
        );
    }

    let version = env!("CARGO_PKG_VERSION");
    println!("cargo:rustc-env=SALT_VERSION={version}+g{sha}");
}

/// Declares every path whose change can make a previously stamped
/// `SaltVersion` wrong.
///
/// Cargo caches a *successful* build script run, so both answers this script
/// produces — the SHA, and whether the tree is dirty — go stale unless the
/// inputs behind them are watched. Two distinct staleness bugs are being shut
/// here, and neither is covered by watching `crates/` alone:
///
/// 1. A tracked file **outside** `crates/` is edited. The tree is now dirty,
///    but nothing under `crates/` moved, so a cached clean-tree run would let
///    a release build through the gate.
/// 2. The changes are **committed**. The working tree is clean again and no
///    source file differs from the last build, but `HEAD` now names a
///    different commit — so the stamped SHA would name the commit *before*
///    the one actually built. That is precisely the lie the gate exists to
///    prevent.
///
/// A failing run is never cached, so the opposite direction — a dirty tree
/// that becomes clean — needs no watch: the panic simply reruns.
fn watch_everything_that_can_change_the_answer(workspace_root: &str) {
    // Every workspace entry except build output and the git database. `target/`
    // is excluded because Cargo walks a watched directory recursively, and
    // watching this build's own output reruns the script on every invocation.
    // A brand-new *untracked* file created directly in the workspace root is
    // the one dirtying edit this cannot see; anything inside a watched
    // directory is seen, because creating it changes that directory's mtime.
    if let Ok(entries) = std::fs::read_dir(workspace_root) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if name == "target" || name == ".git" {
                continue;
            }
            println!("cargo:rerun-if-changed={}", entry.path().display());
        }
    }

    // The commit pointer itself, for case 2. `.git/index` is deliberately not
    // watched: `git status` above may rewrite it, which would rerun this
    // script on every build for no gain.
    let Some(git_dir) = git_output(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return;
    };
    let mut pointers = vec![git_dir.join("HEAD"), git_dir.join("packed-refs")];
    // On an attached HEAD the loose ref file is what a commit rewrites; on a
    // detached HEAD `.git/HEAD` above already carries the SHA.
    if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        pointers.push(git_dir.join(head_ref));
    }
    for pointer in pointers {
        // Only existing paths: Cargo treats a missing watched path as changed,
        // which would rebuild forever once `git gc` packs the loose refs away.
        if Path::new(&pointer).exists() {
            println!("cargo:rerun-if-changed={}", pointer.display());
        }
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
