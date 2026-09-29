//! The security policy: `SECURITY.md` at the repository root, which
//! GitHub shows in the security tab, and the link to it in `README.md`.
//! A person who finds a vulnerability reads these files to find the
//! private route, so a change that deletes or unlinks one fails here.

use std::path::PathBuf;

/// The repository root, read at run time so a binary built in another
/// worktree reads this one.
fn root() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level under the workspace root")
        .to_path_buf()
}

fn read(name: &str) -> String {
    let path = root().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{name} is missing at the repository root ({}): {error}",
            path.display()
        )
    })
}

/// The text with each run of whitespace as one space, because Markdown
/// can wrap a phrase across two lines.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[test]
fn the_repository_root_holds_a_security_policy() {
    let path = root().join("SECURITY.md");
    assert!(
        path.is_file(),
        "SECURITY.md is missing at the repository root: {}",
        path.display()
    );
}

#[test]
fn the_security_policy_names_github_private_vulnerability_reporting() {
    let policy = one_line(&read("SECURITY.md")).to_lowercase();
    assert!(
        policy.contains("github private vulnerability reporting"),
        "SECURITY.md does not name GitHub private vulnerability reporting"
    );
}

#[test]
fn the_security_section_of_the_readme_links_to_the_security_policy() {
    let readme = read("README.md");
    let section = readme
        .split("\n## ")
        .find(|section| section.lines().next() == Some("Security"))
        .expect("README.md has no \"## Security\" section");
    assert!(
        section.contains("](SECURITY.md)"),
        "the Security section of README.md does not link to SECURITY.md:\n{section}"
    );
}
