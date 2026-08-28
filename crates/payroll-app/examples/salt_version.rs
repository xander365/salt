//! Prints the `SaltVersion` baked into this build.
//!
//! Two jobs. Operationally it answers "which code is this?" for a deployed
//! artifact without a repository to consult. In CI it is what lets the
//! verification suite prove the stamped SHA actually follows `HEAD`, rather
//! than assert it in a comment.

fn main() {
    println!("{}", payroll_app::SALT_VERSION);
}
