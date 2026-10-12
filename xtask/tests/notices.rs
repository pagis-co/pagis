//! Tests for the third-party notices: the licenses that the
//! configuration accepts, the notices of the Rust crates from the output
//! of cargo-about, the notices of the npm packages of the Product App from
//! the license list of Vite, the notices of the npm packages of the
//! Coding Harnesses, the text of the notices file, the gate step and the
//! pin of cargo-about.

use std::path::{Path, PathBuf};

use pagis_versions::{CARGO_ABOUT_SHA256, CARGO_ABOUT_VERSION};
use xtask::notices::{
    ABOUT_CONFIG, HARNESS_NOTICES, IOS_ACKNOWLEDGEMENTS, LINUX_TARGETS, Notice, Part,
    RELAY_MANIFEST, SCREEND_NOTICES, SWIFT_PACKAGES, SwiftPin, accepted_licenses, acknowledgements,
    cargo_about_cmd, client_notices, crate_notices, harness_notices, license_file_name,
    mobile_parts, notices_step, pnpm_packages, relay_notices, render, screend_notices,
    swift_package, swift_pins, vite_packages,
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

/// An npm package directory: its `package.json` with `version`, and the
/// named files.
fn npm_package(root: &Path, dir: &str, version: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = root.join(dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        serde_json::json!({ "version": version }).to_string(),
    )
    .unwrap();
    for (file, body) in files {
        std::fs::write(dir.join(file), body).unwrap();
    }
    dir
}

/// pnpm lists the production packages of the Client App by license,
/// with the directory of each version, where the license file is.
#[test]
fn the_pnpm_license_list_gives_each_version_with_its_license_file() {
    let tmp = tempfile::tempdir().unwrap();
    let semver = npm_package(tmp.path(), "semver", "7.7.2", &[("LICENSE", "ISC text\n")]);
    let lazy = npm_package(tmp.path(), "lazy-val", "1.0.5", &[]);
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

/// With the hoisted layout, pnpm lists a directory for each copy of a
/// package, and not one for each version. The version of each copy comes
/// from its `package.json`, and two copies of one version are one package.
#[test]
fn the_pnpm_license_list_reads_the_version_of_each_hoisted_copy() {
    let tmp = tempfile::tempdir().unwrap();
    let top = npm_package(tmp.path(), "zod", "4.6.5", &[("LICENSE", "zod 4")]);
    let nested = npm_package(tmp.path(), "a/zod", "3.25.76", &[("LICENSE", "zod 3")]);
    let again = npm_package(tmp.path(), "b/zod", "3.25.76", &[("LICENSE", "zod 3")]);
    let list = serde_json::json!({
        "MIT": [{
            "name": "zod",
            "versions": ["3.25.76", "4.6.5"],
            "paths": [top, nested, again],
            "license": "MIT",
        }],
    });

    let packages = pnpm_packages(&list.to_string()).unwrap();

    let found: Vec<_> = packages
        .iter()
        .map(|package| (package.version.as_str(), package.text.as_deref()))
        .collect();
    assert_eq!(
        found,
        [("4.6.5", Some("zod 4")), ("3.25.76", Some("zod 3"))]
    );
}

/// A `LICENSES` directory, as the REUSE layout keeps, is not the license
/// file of the package.
#[test]
fn a_license_directory_is_not_a_license_file() {
    let tmp = tempfile::tempdir().unwrap();
    let mcp = npm_package(tmp.path(), "pi-mcp", "1.1.0", &[]);
    std::fs::create_dir_all(mcp.join("LICENSES")).unwrap();
    let list = serde_json::json!({
        "MIT": [{ "name": "pi-mcp", "versions": ["1.1.0"], "paths": [mcp], "license": "MIT" }],
    });

    let packages = pnpm_packages(&list.to_string()).unwrap();

    assert_eq!(packages[0].text, None);
}

#[test]
fn the_notices_of_the_client_app_name_its_npm_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let updater = npm_package(
        tmp.path(),
        "electron-updater",
        "6.8.10",
        &[("LICENSE", "MIT text")],
    );
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

/// A Coding Harness runs as a program of its own, and some harness
/// packages come under the terms of their vendor in a license file, with
/// no SPDX license. The notices pass that file on.
#[test]
fn the_notices_of_the_coding_harnesses_pass_on_the_terms_of_a_vendor() {
    let tmp = tempfile::tempdir().unwrap();
    let sdk = npm_package(
        tmp.path(),
        "claude-agent-sdk",
        "0.3.287",
        &[("LICENSE.md", "© Anthropic PBC. All rights reserved.\n")],
    );
    let pi = npm_package(tmp.path(), "pi-acp", "0.0.34", &[]);
    let list = serde_json::json!({
        "Unknown": [{
            "name": "@anthropic-ai/claude-agent-sdk",
            "versions": ["0.3.287"],
            "paths": [sdk],
            "license": "Unknown",
        }],
        "MIT": [{
            "name": "pi-acp",
            "versions": ["0.0.34"],
            "paths": [pi],
            "license": "MIT",
        }],
    });

    let text = harness_notices(&list.to_string(), &accepted()).unwrap();

    assert!(
        text.starts_with(
            "Third-party notices of the Coding Harnesses of the Pagis Computer Image\n"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "License: the terms of its vendor\nUsed by:\n  @anthropic-ai/claude-agent-sdk 0.3.287\n\n© Anthropic PBC. All rights reserved.\n"
        ),
        "{text}"
    );
    assert!(text.contains("  pi-acp 0.0.34"), "{text}");
    assert!(text.contains("Permission is hereby granted"), "{text}");
}

#[test]
fn a_harness_package_with_vendor_terms_and_no_license_file_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let sdk = npm_package(
        tmp.path(),
        "claude-agent-sdk",
        "0.3.287",
        &[("README.md", "readme")],
    );
    let list = serde_json::json!({
        "Unknown": [{
            "name": "@anthropic-ai/claude-agent-sdk",
            "versions": ["0.3.287"],
            "paths": [sdk],
            "license": "Unknown",
        }],
    });

    let error = harness_notices(&list.to_string(), &accepted()).unwrap_err();

    assert!(
        format!("{error:#}").contains("@anthropic-ai/claude-agent-sdk 0.3.287 has no license file"),
        "{error:#}"
    );
}

/// A new package with no SPDX license stops the gate until a person reads
/// its terms and adds it to the vendor terms.
#[test]
fn a_harness_package_with_an_unknown_license_and_no_vendor_terms_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    let odd = npm_package(tmp.path(), "odd", "1.0.0", &[("LICENSE", "Some terms")]);
    let list = serde_json::json!({
        "Unknown": [{ "name": "odd", "versions": ["1.0.0"], "paths": [odd], "license": "Unknown" }],
    });

    let error = harness_notices(&list.to_string(), &accepted()).unwrap_err();

    assert!(format!("{error:#}").contains("odd 1.0.0"), "{error:#}");
}

/// The Computer Image is built from `computer/` alone, so the notices of
/// the Coding Harnesses are stored beside their lockfile and the image
/// copies them.
#[test]
fn the_computer_image_holds_the_stored_notices_of_the_coding_harnesses() {
    assert_eq!(HARNESS_NOTICES, "computer/harnesses/THIRD_PARTY_NOTICES");
    let root = workspace_root();
    assert!(root.join(HARNESS_NOTICES).is_file());
    let dockerfile = std::fs::read_to_string(root.join("computer/Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(
            "COPY harnesses/THIRD_PARTY_NOTICES /usr/share/doc/coding-harnesses/THIRD_PARTY_NOTICES"
        ),
        "{dockerfile}"
    );
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
/// notices of the Server Package, of the Client App, of the Push Relay
/// image and of the iOS app, and checks the stored notices of screend and
/// of the Coding Harnesses. Each part fails on a license that the
/// configuration does not accept, and on a package that has no notice.
#[test]
fn the_gate_step_writes_the_notices_of_each_release_and_checks_the_stored_notices() {
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
    let notices = xtask::target_dir(tmp.path()).join("notices");
    let xtask = |args: String| format!("cargo run --package xtask -- notices {args}");
    assert_eq!(
        lines,
        [
            "pnpm --filter ./ui run build".to_string(),
            xtask(format!(
                "server {}",
                notices.join("THIRD_PARTY_NOTICES").display()
            )),
            xtask(format!(
                "client {}",
                notices.join("client/THIRD_PARTY_NOTICES").display()
            )),
            xtask(format!(
                "relay {}",
                notices.join("relay/THIRD_PARTY_NOTICES").display()
            )),
            xtask(format!(
                "mobile {}",
                notices.join("mobile/Acknowledgements.plist").display()
            )),
            xtask("screend --check".to_string()),
            xtask("harnesses --check".to_string()),
        ]
    );
    for cmd in cmds {
        assert_eq!(cmd.cwd.as_deref(), Some(tmp.path()));
    }
}

// --- the Push Relay image ---

#[test]
fn the_notices_of_the_push_relay_name_its_crates() {
    let tmp = tempfile::tempdir().unwrap();
    let p256 = crate_dir(tmp.path(), "p256", &[]);
    let json = serde_json::json!({
        "licenses": [{
            "name": "MIT License",
            "id": "MIT",
            "text": "MIT text",
            "used_by": [{ "crate": about_crate("p256", "0.13.2", &p256) }],
        }],
        "crates": [{ "package": about_crate("p256", "0.13.2", &p256) }],
    });

    let text = relay_notices(&json.to_string()).unwrap();

    assert!(
        text.starts_with("Third-party notices of the Pagis Push Relay image\n"),
        "{text}"
    );
    assert!(
        text.contains("The Rust crates of the pagis-push-relay program"),
        "{text}"
    );
    assert!(text.contains("p256 0.13.2"), "{text}");
}

/// The image runs on Linux amd64 and arm64, as the Computer Image does.
#[test]
fn the_push_relay_notices_name_the_crates_of_the_linux_targets() {
    assert_eq!(RELAY_MANIFEST, "crates/pagis-push-relay/Cargo.toml");
    assert_eq!(
        LINUX_TARGETS,
        ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]
    );
}

// --- the Mobile App ---

const PACKAGE_RESOLVED: &str = r#"{
  "originHash" : "3e52",
  "pins" : [
    {
      "identity" : "capacitor-swift-pm",
      "kind" : "remoteSourceControl",
      "location" : "https://github.com/ionic-team/capacitor-swift-pm.git",
      "state" : { "revision" : "0b6882e9", "version" : "8.5.2" }
    },
    {
      "identity" : "osbarcodelib-ios",
      "kind" : "remoteSourceControl",
      "location" : "https://github.com/OutSystems/OSBarcodeLib-iOS.git",
      "state" : { "branch" : "main", "revision" : "7f9d1662" }
    }
  ],
  "version" : 3
}"#;

const MIT_TEXT: &str = "MIT License

Copyright (c) 2022 OutSystems

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
";

fn swift_pin(location: &str, version: Option<&str>) -> SwiftPin {
    SwiftPin {
        location: location.into(),
        revision: "0b6882e9".into(),
        version: version.map(String::from),
    }
}

#[test]
fn the_swift_packages_are_the_pins_of_package_resolved() {
    let pins = swift_pins(PACKAGE_RESOLVED).unwrap();

    assert_eq!(
        pins,
        [
            SwiftPin {
                location: "https://github.com/ionic-team/capacitor-swift-pm.git".into(),
                revision: "0b6882e9".into(),
                version: Some("8.5.2".into()),
            },
            SwiftPin {
                location: "https://github.com/OutSystems/OSBarcodeLib-iOS.git".into(),
                revision: "7f9d1662".into(),
                version: None,
            },
        ]
    );
}

/// Xcode resolves the Swift packages of the app into the `Package.resolved`
/// of its project, and the repository holds it.
#[test]
fn the_ios_app_pins_its_swift_packages() {
    let resolved = std::fs::read_to_string(workspace_root().join(SWIFT_PACKAGES)).unwrap();
    let pins = swift_pins(&resolved).unwrap();
    assert!(
        pins.iter()
            .any(|pin| pin.location == "https://github.com/ionic-team/capacitor-swift-pm.git"),
        "{pins:?}"
    );
}

#[test]
fn a_package_resolved_with_no_pins_is_an_error() {
    let error = swift_pins("{ \"version\": 3 }").unwrap_err();
    assert!(
        format!("{error:#}").contains("Package.resolved"),
        "{error:#}"
    );
}

/// A Swift package names no license, so its license comes from its
/// license text, as cargo-about finds the license of a crate that names
/// none. Text after the license, such as a pointer to other notices, does
/// not stop the detection.
#[test]
fn the_license_of_a_swift_package_comes_from_its_license_text() {
    let pin = swift_pin("https://github.com/OutSystems/OSBarcodeLib-iOS.git", None);
    let text =
        format!("{MIT_TEXT}\nThird-party components are included under their own licenses.\n");

    let package = swift_package(&pin, Some(text.clone())).unwrap();

    assert_eq!(
        package.name,
        "https://github.com/OutSystems/OSBarcodeLib-iOS.git"
    );
    assert_eq!(package.version, "0b6882e9");
    assert_eq!(package.identifier.as_deref(), Some("MIT"));
    assert_eq!(package.text.as_deref(), Some(text.as_str()));
}

#[test]
fn a_swift_package_is_named_by_its_version_when_it_has_one() {
    let pin = swift_pin(
        "https://github.com/ionic-team/capacitor-swift-pm.git",
        Some("8.5.2"),
    );
    let package = swift_package(&pin, Some(MIT_TEXT.into())).unwrap();
    assert_eq!(package.version, "8.5.2");
}

#[test]
fn a_swift_package_with_no_license_file_is_an_error() {
    let pin = swift_pin("https://example.com/a.git", Some("1.0.0"));
    let error = swift_package(&pin, None).unwrap_err();
    assert!(
        format!("{error:#}").contains("https://example.com/a.git 1.0.0 has no license file"),
        "{error:#}"
    );
}

#[test]
fn a_swift_package_whose_license_text_is_unknown_is_an_error() {
    let pin = swift_pin("https://example.com/a.git", Some("1.0.0"));
    let error = swift_package(&pin, Some("You may use this on Tuesdays.".into())).unwrap_err();
    assert!(
        format!("{error:#}").contains("no known license"),
        "{error:#}"
    );
}

/// A Swift package is found as an npm package: the first file whose name
/// starts with `license`, `licence` or `copying` in any case.
#[test]
fn the_license_file_of_a_swift_package_is_found_as_vite_finds_it() {
    let names = ["Package.swift", "README.md", "LICENSE.md", "Sources"].map(String::from);
    assert_eq!(license_file_name(&names), Some("LICENSE.md"));
    assert_eq!(license_file_name(&["README.md".to_string()]), None);
}

#[test]
fn the_mobile_notices_hold_the_npm_packages_and_the_swift_packages() {
    let tmp = tempfile::tempdir().unwrap();
    let core = npm_package(tmp.path(), "core", "8.5.2", &[("LICENSE", "core license")]);
    let list = serde_json::json!({
        "MIT": [{
            "name": "@capacitor/core",
            "versions": ["8.5.2"],
            "paths": [core],
            "license": "MIT",
        }],
    });
    let pin = swift_pin(
        "https://github.com/ionic-team/capacitor-swift-pm.git",
        Some("8.5.2"),
    );
    let swift = vec![swift_package(&pin, Some(MIT_TEXT.into())).unwrap()];

    let parts = mobile_parts(&list.to_string(), swift, &accepted()).unwrap();

    assert_eq!(parts.len(), 2);
    assert!(
        parts[0]
            .title
            .starts_with("The npm packages of the Mobile App")
    );
    assert_eq!(parts[0].notices[0].users, ["@capacitor/core 8.5.2"]);
    assert!(
        parts[1]
            .title
            .starts_with("The Swift packages of the iOS app")
    );
    assert_eq!(
        parts[1].notices[0].users,
        ["https://github.com/ionic-team/capacitor-swift-pm.git 8.5.2"]
    );
}

#[test]
fn a_swift_package_license_that_the_configuration_does_not_accept_is_an_error() {
    let pin = swift_pin("https://example.com/a.git", Some("1.0.0"));
    let swift = vec![swift_package(&pin, Some(MIT_TEXT.into())).unwrap()];
    let error = mobile_parts("{}", swift, &["Apache-2.0".to_string()]).unwrap_err();
    assert!(
        format!("{error:#}").contains("does not accept the license MIT"),
        "{error:#}"
    );
}

/// The iOS convention: a Settings bundle holds the acknowledgements as a
/// child pane, and the Settings app shows them under the app. CocoaPods
/// and LicensePlist write the same property list.
#[test]
fn the_acknowledgements_are_a_settings_pane_with_one_group_for_each_notice() {
    let plist = acknowledgements(&[Part {
        title: "The npm packages & <more>".into(),
        notices: vec![
            Notice {
                license: "MIT".into(),
                users: vec!["b 1".into(), "a 1".into()],
                text: "Copyright <a> & b".into(),
            },
            Notice {
                license: "Apache-2.0".into(),
                users: vec!["c 2".into()],
                text: "apache".into(),
            },
        ],
    }]);

    let doc = roxmltree::Document::parse_with_options(
        &plist,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .unwrap();
    let strings: Vec<&str> = doc
        .descendants()
        .filter(|node| node.has_tag_name("string"))
        .filter_map(|node| node.text())
        .collect();
    let at = |value: &str| {
        strings
            .iter()
            .position(|text| *text == value)
            .unwrap_or_else(|| panic!("no {value:?} in {strings:?}"))
    };
    assert!(strings.contains(&"PSGroupSpecifier"), "{strings:?}");
    assert_eq!(
        strings.last(),
        Some(&"Acknowledgements"),
        "the title of the pane"
    );
    let part = at("The npm packages & <more>");
    let apache = at("Apache-2.0");
    let mit = at("MIT");
    assert!(part < apache && apache < mit, "{strings:?}");
    assert_eq!(strings[apache + 1], "Used by: c 2\n\napache");
    assert_eq!(strings[mit + 1], "Used by: b 1, a 1\n\nCopyright <a> & b");
    assert!(
        strings
            .iter()
            .any(|text| text.contains("Pagis itself is distributed under the MIT License")),
        "{strings:?}"
    );
}

/// The Settings bundle of the app opens the acknowledgements that the
/// release writes into it. The repository does not hold them.
#[test]
fn the_ios_app_shows_the_acknowledgements_in_its_settings() {
    let root = workspace_root();
    assert_eq!(
        IOS_ACKNOWLEDGEMENTS,
        "mobile/ios/App/App/Settings.bundle/Acknowledgements.plist"
    );
    let settings =
        std::fs::read_to_string(root.join("mobile/ios/App/App/Settings.bundle/Root.plist"))
            .unwrap();
    for expected in [
        "<string>PSChildPaneSpecifier</string>",
        "<key>File</key>\n\t\t\t<string>Acknowledgements</string>",
    ] {
        assert!(settings.contains(expected), "no {expected}: {settings}");
    }
    let project =
        std::fs::read_to_string(root.join("mobile/ios/App/App.xcodeproj/project.pbxproj")).unwrap();
    assert!(
        project.contains("/* Settings.bundle in Resources */"),
        "the app target copies the Settings bundle"
    );
    let ignored = std::process::Command::new("git")
        .args([
            "check-ignore",
            "--no-index",
            "--quiet",
            IOS_ACKNOWLEDGEMENTS,
        ])
        .current_dir(&root)
        .status()
        .unwrap();
    assert!(
        ignored.success(),
        "git ignores the written acknowledgements"
    );
}
