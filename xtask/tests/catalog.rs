//! The catalog of the pnpm workspace: a dependency that two or more
//! projects declare refers to the catalog of `pnpm-workspace.yaml`, so its
//! version cannot drift between the projects.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::support::workspace_root;

/// The declarations that keep their own range, as (project, dependency),
/// each with its reason.
const OWN_RANGES: &[(&str, &str)] = &[
    // The documentation site is on TypeScript 7 and Vitest 5, and the
    // other projects are on TypeScript 5.9 and Vitest 4.
    ("docs-site", "typescript"),
    ("docs-site", "vitest"),
    // The Product App holds React at 19.2, and the documentation site
    // takes 19.3.
    ("ui", "react"),
    ("ui", "react-dom"),
    ("ui", "@types/react"),
    ("ui", "@types/react-dom"),
    ("docs-site", "react"),
    ("docs-site", "react-dom"),
    ("docs-site", "@types/react"),
    ("docs-site", "@types/react-dom"),
];

#[derive(Deserialize)]
struct Workspace {
    packages: Vec<String>,
}

#[derive(Deserialize)]
struct Manifest {
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default, rename = "devDependencies")]
    dev_dependencies: BTreeMap<String, String>,
}

/// Each dependency that two or more projects of the workspace at `root`
/// declare, where a project gives its own range instead of `catalog:` and
/// `own_ranges` does not name it. Each item is `<project>: <dependency>
/// <range>`.
fn drift(root: &Path, own_ranges: &[(&str, &str)]) -> Vec<String> {
    let workspace = std::fs::read_to_string(root.join("pnpm-workspace.yaml"))
        .expect("read pnpm-workspace.yaml");
    let workspace: Workspace =
        serde_saphyr::from_str(&workspace).expect("pnpm-workspace.yaml lists the packages");

    let mut declarations: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for project in &workspace.packages {
        let manifest = std::fs::read_to_string(root.join(project).join("package.json"))
            .unwrap_or_else(|e| panic!("read {project}/package.json: {e}"));
        let manifest: Manifest = serde_json::from_str(&manifest)
            .unwrap_or_else(|e| panic!("parse {project}/package.json: {e}"));
        for (name, range) in manifest
            .dependencies
            .into_iter()
            .chain(manifest.dev_dependencies)
        {
            declarations
                .entry(name)
                .or_default()
                .push((project.clone(), range));
        }
    }

    declarations
        .into_iter()
        .filter(|(_, projects)| projects.len() > 1)
        .flat_map(|(name, projects)| {
            projects.into_iter().filter_map(move |(project, range)| {
                let own = own_ranges.contains(&(project.as_str(), name.as_str()));
                (range != "catalog:" && !own).then(|| format!("{project}: {name} {range}"))
            })
        })
        .collect()
}

fn write(root: &Path, path: &str, contents: &str) {
    let path = root.join(path);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn two_projects_with_their_own_range_of_one_dependency_drift() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "pnpm-workspace.yaml",
        "packages:\n  - ui\n  - mobile\n",
    );
    write(
        tmp.path(),
        "ui/package.json",
        r#"{"devDependencies": {"jsdom": "^30.0.1", "three": "^0.186.0"}}"#,
    );
    write(
        tmp.path(),
        "mobile/package.json",
        r#"{"devDependencies": {"jsdom": "^30.1.1"}}"#,
    );

    assert_eq!(
        drift(tmp.path(), &[]),
        ["ui: jsdom ^30.0.1", "mobile: jsdom ^30.1.1"],
        "a dependency of one project only keeps its range"
    );
}

#[test]
fn each_dependency_that_projects_of_the_workspace_share_refers_to_the_catalog() {
    let drift = drift(&workspace_root(), OWN_RANGES);
    assert!(
        drift.is_empty(),
        "these declarations must be \"catalog:\", or name the exception in OWN_RANGES \
         with its reason:\n{}",
        drift.join("\n")
    );
}
