//! Tests for the third-party notices: the licenses that the
//! configuration accepts, the notices of the Rust crates from the output
//! of cargo-about, the notices of the npm packages of the Product App from
//! the license list of Vite, the text of the notices file, the gate step
//! and the pin of cargo-about.

use std::path::Path;

use pagis_versions::{CARGO_ABOUT_SHA256, CARGO_ABOUT_VERSION};
use xtask::notices::{
    ABOUT_CONFIG, Notice, Part, SCREEND_NOTICES, accepted_licenses, cargo_about_cmd,
    client_notices, crate_notices, notices_step, pnpm_packages, render, screend_notices,
    vite_packages,
};
use xtask::{Action, tools};

use crate::support::{host_index, host_is_pinned, workspace_root};

fn accepted() -> Vec<String> {
    ["MIT", "Apache-2.0", "OFL-1.1"].map(String::from).to_vec()
}

/// A crate directory with a `Cargo.toml` and the named extra files, and
/// the path of its manifest.
fn crate_dir(root: &Path, name: &str, files: &[(&str, &str)]) -> String {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\n").unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
    dir.join("Cargo.toml").to_string_lossy().into_owned()
}

/// The notices of a license list of Vite.
fn package_notices(list: &str, accepted: &[String]) -> anyhow::Result<Vec<Notice>> {
    xtask::notices::package_notices(vite_packages(list)?, accepted)
}

fn about_crate(name: &str, version: &str, manifest: &str) -> serde_json::Value {
    serde_json::json!({ "name": name, "version": version, "manifest_path": manifest })
}

// --- the configuration ---

#[test]
fn the_accepted_licenses_come_from_the_configuration_in_their_order() {
    let config = "accepted = [\"MIT\", \"Apache-2.0\"]\ntargets = []\n";
    assert_eq!(accepted_licenses(config).unwrap(), ["MIT", "Apache-2.0"]);
}

#[test]
fn a_configuration_with_no_accepted_list_is_an_error() {
    let error = accepted_licenses("targets = []\n").unwrap_err();
    assert!(format!("{error:#}").contains("accepted"), "{error:#}");
}

/// cargo-about and cargo-deny read the same release targets, so the
/// notices name the crates that the advisory check checks.
#[test]
fn cargo_about_reads_the_release_targets_of_cargo_deny() {
    let read = |path: &str| -> toml::Value {
        toml::from_str(&std::fs::read_to_string(workspace_root().join(path)).unwrap()).unwrap()
    };
    let about = read(ABOUT_CONFIG);
    let deny = read("deny.toml");
    assert_eq!(about["targets"], deny["graph"]["targets"]);
    assert_eq!(about["ignore-build-dependencies"].as_bool(), Some(true));
    assert_eq!(about["ignore-dev-dependencies"].as_bool(), Some(true));
    assert_eq!(about["private"]["ignore"].as_bool(), Some(true));
}

/// The Product App build writes the license lists that the notices of
/// the Server Package read.
#[test]
fn the_product_app_build_writes_the_license_lists_that_the_notices_read() {
    for (config, list) in [
        ("ui/vite.config.ts", ".vite/license.json"),
        ("ui/vite.sw.config.ts", ".vite/license-sw.json"),
    ] {
        let body = std::fs::read_to_string(workspace_root().join(config)).unwrap();
        assert!(
            body.contains(&format!("license: {{ fileName: '{list}' }}")),
            "{config} does not write {list}"
        );
        assert!(
            xtask::notices::PRODUCT_APP_LICENSES.contains(&format!("ui/dist/{list}").as_str()),
            "the notices do not read ui/dist/{list}"
        );
    }
}

/// pagis-screend is the own code of Pagis, so cargo-about leaves it out
/// as it leaves out the members of the workspace.
#[test]
fn each_own_crate_is_private_and_under_the_mit_license() {
    let root = workspace_root();
    let workspace: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("Cargo.toml")).unwrap()).unwrap();
    assert_eq!(
        workspace["workspace"]["package"]["publish"].as_bool(),
        Some(false)
    );
    let screend: toml::Value =
        toml::from_str(&std::fs::read_to_string(root.join("computer/screend/Cargo.toml")).unwrap())
            .unwrap();
    assert_eq!(screend["package"]["publish"].as_bool(), Some(false));
    assert_eq!(screend["package"]["license"].as_str(), Some("MIT"));
}

// --- the Rust crates ---

#[test]
fn each_crate_license_names_the_crates_that_use_it() {
    let tmp = tempfile::tempdir().unwrap();
    let a = crate_dir(tmp.path(), "a", &[]);
    let b = crate_dir(tmp.path(), "b", &[]);
    let json = serde_json::json!({
        "licenses": [{
            "name": "MIT License",
            "id": "MIT",
            "text": "Copyright b\n\nPermission is granted.\n",
            "used_by": [
                { "crate": about_crate("b", "2.0.0", &b) },
                { "crate": about_crate("a", "1.0.0", &a) },
            ],
        }],
        "crates": [
            { "package": about_crate("a", "1.0.0", &a) },
            { "package": about_crate("b", "2.0.0", &b) },
        ],
    });

    let notices = crate_notices(&json.to_string()).unwrap();

    assert_eq!(
        notices,
        [Notice {
            license: "MIT".into(),
            users: vec!["a 1.0.0".into(), "b 2.0.0".into()],
            text: "Copyright b\n\nPermission is granted.".into(),
        }]
    );
}

/// Apache-2.0 requires that a redistribution passes on the NOTICE file of
/// the work, and cargo-about does not read NOTICE files.
#[test]
fn the_notice_file_of_a_crate_is_passed_on() {
    let tmp = tempfile::tempdir().unwrap();
    let moka = crate_dir(
        tmp.path(),
        "moka",
        &[("NOTICE", "Moka\nCopyright moka authors\n")],
    );
    let json = serde_json::json!({
        "licenses": [{
            "name": "Apache License 2.0",
            "id": "Apache-2.0",
            "text": "Apache text",
            "used_by": [{ "crate": about_crate("moka", "0.12.16", &moka) }],
        }],
        "crates": [{ "package": about_crate("moka", "0.12.16", &moka) }],
    });

    let notices = crate_notices(&json.to_string()).unwrap();

    assert!(
        notices.contains(&Notice {
            license: "NOTICE".into(),
            users: vec!["moka 0.12.16".into()],
            text: "Moka\nCopyright moka authors".into(),
        }),
        "{notices:?}"
    );
}

#[test]
fn a_crate_that_no_license_text_names_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let a = crate_dir(tmp.path(), "a", &[]);
    let lone = crate_dir(tmp.path(), "lone", &[]);
    let json = serde_json::json!({
        "licenses": [{
            "name": "MIT License",
            "id": "MIT",
            "text": "text",
            "used_by": [{ "crate": about_crate("a", "1.0.0", &a) }],
        }],
        "crates": [
            { "package": about_crate("a", "1.0.0", &a) },
            { "package": about_crate("lone", "0.1.0", &lone) },
        ],
    });

    let error = crate_notices(&json.to_string()).unwrap_err();

    assert!(format!("{error:#}").contains("lone 0.1.0"), "{error:#}");
}

#[test]
fn an_empty_license_text_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let a = crate_dir(tmp.path(), "a", &[]);
    let json = serde_json::json!({
        "licenses": [{
            "name": "MIT License",
            "id": "MIT",
            "text": "  \n",
            "used_by": [{ "crate": about_crate("a", "1.0.0", &a) }],
        }],
        "crates": [{ "package": about_crate("a", "1.0.0", &a) }],
    });

    let error = crate_notices(&json.to_string()).unwrap_err();

    assert!(format!("{error:#}").contains("a 1.0.0"), "{error:#}");
}

// --- the npm packages of the Product App ---

#[test]
fn packages_with_the_same_license_text_share_one_notice() {
    let list = serde_json::json!([
        { "name": "zustand", "version": "5.0.15", "identifier": "MIT", "text": "MIT text" },
        { "name": "react", "version": "19.2.8", "identifier": "MIT", "text": "MIT text" },
        { "name": "lucide-react", "version": "1.48.0", "identifier": "ISC", "text": "ISC text" },
    ]);
    let mut accepted = accepted();
    accepted.push("ISC".into());

    let notices = package_notices(&list.to_string(), &accepted).unwrap();

    assert_eq!(
        notices,
        [
            Notice {
                license: "ISC".into(),
                users: vec!["lucide-react 1.48.0".into()],
                text: "ISC text".into(),
            },
            Notice {
                license: "MIT".into(),
                users: vec!["react 19.2.8".into(), "zustand 5.0.15".into()],
                text: "MIT text".into(),
            },
        ]
    );
}

#[test]
fn a_package_license_that_the_configuration_does_not_accept_is_an_error() {
    let list = serde_json::json!([
        { "name": "copyleft", "version": "1.0.0", "identifier": "GPL-3.0-only", "text": "GPL" },
    ]);

    let error = package_notices(&list.to_string(), &accepted()).unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("copyleft 1.0.0"), "{message}");
    assert!(message.contains("GPL-3.0-only"), "{message}");
}

#[test]
fn an_expression_with_one_accepted_choice_is_accepted() {
    let list = serde_json::json!([
        { "name": "dual", "version": "1.0.0", "identifier": "(GPL-3.0-only OR MIT)", "text": "t" },
    ]);

    let notices = package_notices(&list.to_string(), &accepted()).unwrap();

    assert_eq!(notices[0].license, "(GPL-3.0-only OR MIT)");
}

#[test]
fn a_package_with_no_license_is_an_error() {
    let list = serde_json::json!([{ "name": "bare", "version": "1.0.0", "text": "t" }]);

    let error = package_notices(&list.to_string(), &accepted()).unwrap_err();

    assert!(format!("{error:#}").contains("bare 1.0.0"), "{error:#}");
}

/// A package can leave its license file out of what it publishes. The
/// notice then holds the standard text of the license, as cargo-about
/// does for a crate.
#[test]
fn a_package_with_no_license_file_gets_the_standard_license_text() {
    let list = serde_json::json!([
        { "name": "@react-three/fiber", "version": "9.7.0", "identifier": "MIT" },
    ]);

    let notices = package_notices(&list.to_string(), &accepted()).unwrap();

    assert_eq!(notices[0].users, ["@react-three/fiber 9.7.0"]);
    assert!(
        notices[0].text.starts_with("MIT License"),
        "{}",
        notices[0].text
    );
    assert!(
        notices[0].text.contains("Permission is hereby granted"),
        "{}",
        notices[0].text
    );
}

#[test]
fn an_empty_license_list_gives_no_notice() {
    assert_eq!(package_notices("[]", &accepted()).unwrap(), []);
}

/// pnpm lists the production packages of the Client App by license,
/// with the directory of each version, where the license file is.
#[test]
fn the_pnpm_license_list_gives_each_version_with_its_license_file() {
    let tmp = tempfile::tempdir().unwrap();
    let semver = tmp.path().join("semver");
    std::fs::create_dir_all(&semver).unwrap();
    std::fs::write(semver.join("LICENSE"), "ISC text\n").unwrap();
    let lazy = tmp.path().join("lazy-val");
    std::fs::create_dir_all(&lazy).unwrap();
    let list = serde_json::json!({
        "ISC": [{
            "name": "semver",
            "versions": ["7.7.2"],
            "paths": [semver],
            "license": "ISC",
        }],
        "MIT": [{
            "name": "lazy-val",
            "versions": ["1.0.5"],
            "paths": [lazy],
            "license": "MIT",
        }],
    });

    let notices = xtask::notices::package_notices(pnpm_packages(&list.to_string()).unwrap(), &{
        let mut accepted = accepted();
        accepted.push("ISC".into());
        accepted
    })
    .unwrap();

    assert_eq!(notices[0].users, ["semver 7.7.2"]);
    assert_eq!(notices[0].text, "ISC text");
    assert_eq!(notices[1].users, ["lazy-val 1.0.5"]);
    assert!(
        notices[1].text.contains("Permission is hereby granted"),
        "{notices:?}"
    );
}

#[test]
fn a_pnpm_entry_whose_versions_and_paths_differ_is_an_error() {
    let list = serde_json::json!({
        "MIT": [{ "name": "odd", "versions": ["1.0.0", "2.0.0"], "paths": ["/x"], "license": "MIT" }],
    });

    let error = pnpm_packages(&list.to_string()).unwrap_err();

    assert!(format!("{error:#}").contains("odd"), "{error:#}");
}

#[test]
fn the_notices_of_the_client_app_name_its_npm_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let updater = tmp.path().join("electron-updater");
    std::fs::create_dir_all(&updater).unwrap();
    std::fs::write(updater.join("LICENSE"), "MIT text").unwrap();
    let list = serde_json::json!({
        "MIT": [{
            "name": "electron-updater",
            "versions": ["6.8.10"],
            "paths": [updater],
            "license": "MIT",
        }],
    });

    let text = client_notices(&list.to_string(), &accepted()).unwrap();

    assert!(
        text.starts_with("Third-party notices of the Pagis Client App\n"),
        "{text}"
    );
    assert!(text.contains("electron-updater 6.8.10"), "{text}");
    assert!(text.contains("LICENSE.electron.txt"), "{text}");
}

// --- the notices file ---

#[test]
fn the_notices_file_holds_each_part_with_its_notices() {
    let text = render(
        "the Pagis Server Package",
        &[
            Part {
                title: "gog".into(),
                notices: vec![Notice {
                    license: "MIT".into(),
                    users: vec!["gog 0.42.0".into()],
                    text: "gog license".into(),
                }],
            },
            Part {
                title: "The Rust crates of pagis".into(),
                notices: vec![Notice {
                    license: "Apache-2.0".into(),
                    users: vec!["a 1.0.0".into(), "b 2.0.0".into()],
                    text: "line one\nline two".into(),
                }],
            },
        ],
    );

    assert!(
        text.starts_with("Third-party notices of the Pagis Server Package\n"),
        "{text}"
    );
    let gog = text.find("gog\n=====").expect("the gog part");
    let crates = text
        .find("The Rust crates of pagis\n=====")
        .expect("the crate part");
    assert!(gog < crates, "{text}");
    assert!(
        text.contains(
            "License: Apache-2.0\nUsed by:\n  a 1.0.0\n  b 2.0.0\n\nline one\nline two\n"
        ),
        "{text}"
    );
    assert!(text.ends_with("line two\n"), "{text}");
}

/// The file is the same for the same input, so the stored notices of
/// screend change only when their input changes.
#[test]
fn the_notices_file_lists_the_notices_in_a_fixed_order() {
    let notice = |license: &str, user: &str| Notice {
        license: license.into(),
        users: vec![user.into()],
        text: "t".into(),
    };
    let one = render(
        "x",
        &[Part {
            title: "p".into(),
            notices: vec![
                notice("MIT", "b 1"),
                notice("ISC", "c 1"),
                notice("MIT", "a 1"),
            ],
        }],
    );
    let two = render(
        "x",
        &[Part {
            title: "p".into(),
            notices: vec![
                notice("MIT", "a 1"),
                notice("MIT", "b 1"),
                notice("ISC", "c 1"),
            ],
        }],
    );

    assert_eq!(one, two);
    assert!(one.find("c 1").unwrap() < one.find("a 1").unwrap(), "{one}");
}

#[test]
fn the_notices_of_screend_name_its_crates() {
    let tmp = tempfile::tempdir().unwrap();
    let ring = crate_dir(tmp.path(), "ring", &[]);
    let json = serde_json::json!({
        "licenses": [{
            "name": "ISC License",
            "id": "ISC",
            "text": "ISC text",
            "used_by": [{ "crate": about_crate("ring", "0.17.14", &ring) }],
        }],
        "crates": [{ "package": about_crate("ring", "0.17.14", &ring) }],
    });

    let text = screend_notices(&json.to_string()).unwrap();

    assert!(
        text.starts_with(
            "Third-party notices of pagis-screend, the screen daemon of the Pagis Computer Image\n"
        ),
        "{text}"
    );
    assert!(text.contains("ring 0.17.14"), "{text}");
}

/// The Computer Image is built from `computer/` alone, where xtask is not,
/// so the notices of screend are stored beside it and the image copies
/// them.
#[test]
fn the_computer_image_holds_the_stored_notices_of_screend() {
    assert_eq!(SCREEND_NOTICES, "computer/screend/THIRD_PARTY_NOTICES");
    let root = workspace_root();
    assert!(root.join(SCREEND_NOTICES).is_file());
    let dockerfile = std::fs::read_to_string(root.join("computer/Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(
            "COPY screend/THIRD_PARTY_NOTICES /usr/share/doc/pagis-screend/THIRD_PARTY_NOTICES"
        ),
        "{dockerfile}"
    );
}

// --- cargo-about ---

#[test]
fn cargo_about_runs_offline_and_fails_on_a_license_it_cannot_satisfy() {
    let tmp = tempfile::tempdir().unwrap();
    let target = Path::new("/shared/pagis-target");
    let cmd = cargo_about_cmd(
        tmp.path(),
        target,
        "computer/screend/Cargo.toml",
        &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"],
        Path::new("/out/screend.json"),
    );
    let script = cmd.args.join(" ");

    assert_eq!(cmd.program, "sh");
    assert_eq!(cmd.cwd.as_deref(), Some(tmp.path()));
    assert!(
        script.contains("cargo fetch --locked --manifest-path \"$1\""),
        "{script}"
    );
    for option in [
        "generate",
        "--format json",
        "--frozen",
        "--fail",
        "--config about.toml",
        "--manifest-path \"$1\"",
        "--output-file \"$2\"",
        "--target x86_64-unknown-linux-gnu",
        "--target aarch64-unknown-linux-gnu",
    ] {
        assert!(script.contains(option), "{option} is missing: {script}");
    }
    assert!(
        script.contains("/shared/pagis-target/cargo-about"),
        "{script}"
    );
    assert!(
        script.ends_with("computer/screend/Cargo.toml /out/screend.json"),
        "{script}"
    );
    if host_is_pinned() {
        assert!(
            script.contains(CARGO_ABOUT_SHA256[host_index()].1),
            "{script}"
        );
    }
}

#[test]
fn the_cargo_about_pin_names_the_release_archive_of_the_host() {
    let Ok(pin) = tools::cargo_about() else {
        assert!(!host_is_pinned());
        return;
    };
    let (triple, sha256) = CARGO_ABOUT_SHA256[host_index()];
    assert_eq!(pin.tool, "cargo-about");
    assert_eq!(
        pin.url,
        format!(
            "https://github.com/EmbarkStudios/cargo-about/releases/download/{CARGO_ABOUT_VERSION}/cargo-about-{CARGO_ABOUT_VERSION}-{triple}.tar.gz"
        )
    );
    assert_eq!(pin.sha256, sha256);
    assert_eq!(
        pin.member,
        format!("cargo-about-{CARGO_ABOUT_VERSION}-{triple}/cargo-about")
    );
}

// --- the gate step ---

/// The gate builds the Product App for its license list, writes the
/// notices of the Server Package and of the Client App, and checks the
/// stored notices of screend. Each part fails on a license that the configuration does not
/// accept, and on a package that has no notice.
#[test]
fn the_gate_step_writes_the_server_notices_and_checks_the_screend_notices() {
    let tmp = tempfile::tempdir().unwrap();
    let step = notices_step(tmp.path());
    assert_eq!(step.name, "notices");
    let Action::Run(cmds) = &step.action else {
        panic!("the step must run, got {:?}", step.action);
    };
    let lines: Vec<String> = cmds
        .iter()
        .map(|cmd| format!("{} {}", cmd.program, cmd.args.join(" ")))
        .collect();
    let output = xtask::target_dir(tmp.path()).join("notices/THIRD_PARTY_NOTICES");
    let client = xtask::target_dir(tmp.path()).join("notices/client/THIRD_PARTY_NOTICES");
    assert_eq!(
        lines,
        [
            "pnpm --filter ./ui run build".to_string(),
            format!(
                "cargo run --package xtask -- notices server {}",
                output.display()
            ),
            format!(
                "cargo run --package xtask -- notices client {}",
                client.display()
            ),
            "cargo run --package xtask -- notices screend --check".to_string(),
        ]
    );
    for cmd in cmds {
        assert_eq!(cmd.cwd.as_deref(), Some(tmp.path()));
    }
}
