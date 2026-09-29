//! What Pagis encrypts at rest, and what a model provider receives: the
//! "What Pagis encrypts" section of `docs/DATA-AND-PRIVACY.md`, and the
//! links to it from the backup sections of `docs/DEPLOYING-A-SERVER.md`
//! and `desktop/README.md`. An operator reads these sections before they keep
//! a Backup or select a disk, so a change that deletes the section, drops
//! a store from it or unlinks it fails here.

use std::path::PathBuf;

const PRIVACY: &str = "docs/DATA-AND-PRIVACY.md";
const SERVER: &str = "docs/DEPLOYING-A-SERVER.md";
const CLIENT_APP: &str = "desktop/README.md";

const ENCRYPTION: &str = "What Pagis encrypts";
const MODEL_PROVIDER: &str = "What the model provider receives";

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
            "{name} is missing in the repository ({}): {error}",
            path.display()
        )
    })
}

/// The anchor that GitHub gives a heading: lower case, spaces as
/// hyphens, and no punctuation.
fn anchor(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter(|letter| letter.is_alphanumeric() || matches!(letter, ' ' | '-' | '_'))
        .map(|letter| if letter == ' ' { '-' } else { letter })
        .collect()
}

/// The level of a heading line (2 for `## `), or `None` for another
/// line.
fn heading_level(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|letter| *letter == '#').count();
    (level > 0 && line[level..].starts_with(' ')).then_some(level)
}

/// The lines under the heading `name` of `level`, up to the next heading
/// of the same level or a higher one. A `#` line in a fenced code block
/// is not a heading.
fn section(document: &str, level: usize, name: &str) -> Option<String> {
    let title = format!("{} {name}", "#".repeat(level));
    let mut lines = Vec::new();
    let mut inside = false;
    let mut fenced = false;
    for line in document.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        let heading = if fenced { None } else { heading_level(line) };
        if inside {
            if heading.is_some_and(|found| found <= level) {
                break;
            }
            lines.push(line);
        } else if heading == Some(level) && line.trim_end() == title {
            inside = true;
        }
    }
    inside.then(|| lines.join("\n"))
}

fn required_section(file: &str, level: usize, name: &str) -> String {
    section(&read(file), level, name)
        .unwrap_or_else(|| panic!("{file} has no \"{} {name}\" section", "#".repeat(level)))
}

/// The cells of each row of every Markdown table in `text`, without the
/// header rows and the delimiter rows.
fn table_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    let mut header = true;
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('|') {
            header = true;
            continue;
        }
        let cells: Vec<String> = line
            .trim_matches('|')
            .split('|')
            .map(|cell| cell.trim().to_string())
            .collect();
        let delimiter = cells
            .iter()
            .all(|cell| !cell.is_empty() && cell.chars().all(|letter| matches!(letter, '-' | ':')));
        if header {
            header = false;
        } else if !delimiter {
            rows.push(cells);
        }
    }
    rows
}

/// Every `code` span in `text`, without the backticks.
fn code_spans(text: &str) -> Vec<String> {
    text.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The rows of the table in the "What Pagis encrypts" section. Each row
/// has a store, what it holds, and whether Pagis encrypts it.
fn encryption_rows() -> Vec<Vec<String>> {
    let rows = table_rows(&required_section(PRIVACY, 2, ENCRYPTION));
    assert!(
        !rows.is_empty(),
        "the \"{ENCRYPTION}\" section of {PRIVACY} has no table of stores"
    );
    rows
}

/// Fail unless a row names `store` in its first cell, and its third cell
/// starts with the answer: "Yes", "No" or "Partly".
fn assert_row_for(rows: &[Vec<String>], store: &str, found: impl Fn(&str) -> bool) {
    let row = rows
        .iter()
        .find(|row| row.first().is_some_and(|cell| found(cell)))
        .unwrap_or_else(|| {
            panic!("the \"{ENCRYPTION}\" table of {PRIVACY} has no row for {store}:\n{rows:#?}")
        });
    let answered = row.get(2).is_some_and(|cell| {
        ["Yes", "No", "Partly"]
            .iter()
            .any(|answer| cell.starts_with(answer))
    });
    assert!(
        answered,
        "the \"{ENCRYPTION}\" row of {store} does not say whether Pagis encrypts it: {row:?}"
    );
}

#[test]
fn the_privacy_document_has_a_what_pagis_encrypts_section() {
    required_section(PRIVACY, 2, ENCRYPTION);
}

#[test]
fn the_encryption_table_names_each_path_of_the_state_directory() {
    let data = required_section(PRIVACY, 2, "Data and configuration");
    let paths: Vec<String> = table_rows(&data)
        .iter()
        .filter_map(|row| row.first())
        .flat_map(|cell| code_spans(cell))
        .collect();
    assert!(
        paths.contains(&"secrets.enc".to_string()) && paths.contains(&"pagis.db".to_string()),
        "the \"Data and configuration\" table of {PRIVACY} lists no State Directory paths: {paths:?}"
    );
    let rows = encryption_rows();
    for path in &paths {
        assert_row_for(&rows, &format!("`{path}`"), |cell| {
            code_spans(cell).contains(path)
        });
    }
}

#[test]
fn the_encryption_table_names_the_database_and_the_computer_volumes() {
    let rows = encryption_rows();
    assert_row_for(&rows, "the Postgres database", |cell| {
        cell.contains("Postgres")
    });
    assert_row_for(&rows, "the Computer volumes", |cell| {
        cell.contains("Computer volume")
    });
}

#[test]
fn the_privacy_document_says_what_the_model_provider_receives() {
    let encryption = required_section(PRIVACY, 2, ENCRYPTION);
    assert!(
        section(&encryption, 3, MODEL_PROVIDER).is_some(),
        "the \"{ENCRYPTION}\" section of {PRIVACY} has no \"### {MODEL_PROVIDER}\" part"
    );
}

#[test]
fn the_backup_section_of_the_server_document_links_to_what_pagis_encrypts() {
    let backup = required_section(SERVER, 2, "Backup and restore");
    let link = format!("](../{PRIVACY}#{})", anchor(ENCRYPTION));
    assert!(
        backup.contains(&link),
        "the Backup section of {SERVER} does not link to {link}:\n{backup}"
    );
}

#[test]
fn the_backup_section_of_the_client_app_document_links_to_what_pagis_encrypts() {
    let backup = required_section(CLIENT_APP, 2, "Back up and restore");
    let link = format!("](../{PRIVACY}#{})", anchor(ENCRYPTION));
    assert!(
        backup.contains(&link),
        "the backup section of {CLIENT_APP} does not link to {link}:\n{backup}"
    );
}

#[test]
fn the_server_document_links_the_provider_keys_to_what_the_model_provider_receives() {
    let administrator = required_section(SERVER, 2, "The first administrator");
    let link = format!("](../{PRIVACY}#{})", anchor(MODEL_PROVIDER));
    assert!(
        administrator.contains(&link),
        "the first administrator section of {SERVER} does not link to {link}:\n{administrator}"
    );
}

#[test]
fn the_multi_user_part_of_the_privacy_document_links_to_what_the_model_provider_receives() {
    let several = required_section(PRIVACY, 2, "Several People on a local installation");
    let link = format!("](#{})", anchor(MODEL_PROVIDER));
    assert!(
        several.contains(&link),
        "the multi-user part of {PRIVACY} does not link to {link}:\n{several}"
    );
}

#[test]
fn a_heading_anchor_is_the_github_anchor() {
    assert_eq!(anchor(ENCRYPTION), "what-pagis-encrypts");
    assert_eq!(anchor("Back up and restore"), "back-up-and-restore");
    assert_eq!(
        anchor("A Computer's disk, bounded"),
        "a-computers-disk-bounded"
    );
}

#[test]
fn a_hash_line_in_a_code_block_does_not_end_a_section() {
    let document = "## One\n\ntext\n\n```bash\n# a comment\n```\n\nmore\n\n## Two\n\nother\n";
    let one = section(document, 2, "One").expect("the section");
    assert!(one.contains("more"), "{one}");
    assert!(!one.contains("other"), "{one}");
}
