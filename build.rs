use std::process::Command;

fn main() {
    // Re-run only if .git/HEAD or .git/refs/heads changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/refs/heads");

    // Get current git hash.
    let output = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .expect("Failed to execute git command");
    let git_hash = String::from_utf8(output.stdout)
        .expect("Invalid UTF-8")
        .trim()
        .to_string();

    // Set git hash as an environment variable.
    println!("cargo:rustc-env=GIT_HASH={git_hash}");
}
