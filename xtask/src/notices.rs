//! The third-party notices (`cargo xtask notices`): the license text of
//! each piece of third-party software that a release ships, and the
//! check that each license is one that the configuration accepts
//! (ADR-0037).
//!
//! cargo-about reads the crates that a program links for the release
//! targets, chooses the accepted license of each one, and finds its
//! license text. [`ABOUT_CONFIG`] holds the accepted licenses and the
//! targets. It fails on a license that it cannot satisfy or find. Vite
//! writes the npm packages that the Product App bundles, with the license
//! and the license text of each one, to [`PRODUCT_APP_LICENSES`]. pnpm
//! lists the npm packages that the Client App ships. [`package_notices`]
//! checks the license of each npm package against the same list.
//!
//! `cargo xtask notices server <output>` writes the notices of the Server
//! Package and the Headless Server image: gog, the crates of `pagis`, and
//! the npm packages of the Product App, which `pagis` embeds. The release
//! and the image build write them, and the gate writes them into the
//! target directory, so a release cannot ship without them.
//!
//! `cargo xtask notices client <output>` writes the notices of the npm
//! packages of the Client App, which `cargo xtask desktop` puts into the
//! app before the pack.
//!
//! `cargo xtask notices screend` writes [`SCREEND_NOTICES`], the notices
//! of the crates of `pagis-screend`. The Computer Image is built from
//! `computer/` alone, where xtask is not, so the file is stored beside
//! screend and the image copies it. With `--check` the command writes
//! nothing and fails when the stored file is not what it writes.
//!
//! `cargo xtask notices harnesses [--check]` writes [`HARNESS_NOTICES`],
//! the notices of the npm packages of the Coding Harnesses that the
//! Computer Image holds, in the same way. It installs the packages of the
//! harness lockfile into the target directory and reads them with pnpm.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::{Action, Cmd, Step, target_dir, tools};

/// The configuration of cargo-about: the accepted licenses in the order
/// of preference, and the release targets.
pub const ABOUT_CONFIG: &str = "about.toml";

/// The license lists that the Product App build writes: one for the
/// pages and one for the service worker.
pub const PRODUCT_APP_LICENSES: [&str; 2] = [
    "ui/dist/.vite/license.json",
    "ui/dist/.vite/license-sw.json",
];

/// The stored notices of `pagis-screend`, which the Computer Image holds.
pub const SCREEND_NOTICES: &str = "computer/screend/THIRD_PARTY_NOTICES";

/// The manifest of `pagis-screend`.
pub const SCREEND_MANIFEST: &str = "computer/screend/Cargo.toml";

/// The targets of the Computer Image: Linux amd64 and arm64.
pub const SCREEND_TARGETS: [&str; 2] = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"];

/// The stored notices of the npm packages of the Coding Harnesses, which
/// the Computer Image holds.
pub const HARNESS_NOTICES: &str = "computer/harnesses/THIRD_PARTY_NOTICES";

/// The directory of the harness lockfile, and the files that an install of
/// it reads.
pub const HARNESS_DIR: &str = "computer/harnesses";
const HARNESS_FILES: [&str; 3] = ["package.json", "pnpm-workspace.yaml", "pnpm-lock.yaml"];

/// The npm packages of the Coding Harnesses that come under the terms of
/// their vendor in their license file, and not under an SPDX license. A
/// person reads the terms of a package before it goes into this list. A
/// harness runs as a program of its own, so these terms do not reach the
/// rest of Pagis.
pub const VENDOR_TERMS: [&str; 1] = ["@anthropic-ai/claude-agent-sdk"];

/// The manifest of the `pagis` program.
pub const SERVER_MANIFEST: &str = "crates/pagis/Cargo.toml";

/// The license of gog, which the Server Package also holds as
/// `LICENSE.gog`.
pub const GOG_LICENSE: &str = "third_party/gog/LICENSE";

/// One license text and the software that uses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// The SPDX expression of the license, or `NOTICE` for the NOTICE
    /// file of a crate.
    pub license: String,
    /// The name and the version of each user, in order.
    pub users: Vec<String>,
    pub text: String,
}

/// One part of the notices file: the notices of one kind of software.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub title: String,
    pub notices: Vec<Notice>,
}

/// The accepted licenses of the configuration `about_toml`, in order.
pub fn accepted_licenses(about_toml: &str) -> Result<Vec<String>> {
    #[derive(Deserialize)]
    struct Config {
        accepted: Vec<String>,
    }
    let config: Config = toml::from_str(about_toml)
        .with_context(|| format!("{ABOUT_CONFIG} has no `accepted` list of licenses"))?;
    Ok(config.accepted)
}

#[derive(Deserialize)]
struct About {
    licenses: Vec<AboutLicense>,
    crates: Vec<AboutCrate>,
}

#[derive(Deserialize)]
struct AboutLicense {
    id: String,
    text: String,
    used_by: Vec<AboutUse>,
}

#[derive(Deserialize)]
struct AboutUse {
    #[serde(rename = "crate")]
    package: AboutPackage,
}

#[derive(Deserialize)]
struct AboutCrate {
    package: AboutPackage,
}

#[derive(Deserialize)]
struct AboutPackage {
    name: String,
    version: String,
    manifest_path: String,
}

impl AboutPackage {
    fn user(&self) -> String {
        format!("{} {}", self.name, self.version)
    }
}

/// The notices of the crates in `about_json`, the JSON output of
/// cargo-about: each license text with the crates that use it, and the
/// NOTICE file of each crate that has one. A crate that no license text
/// names, or an empty license text, is an error.
pub fn crate_notices(about_json: &str) -> Result<Vec<Notice>> {
    let about: About =
        serde_json::from_str(about_json).context("read the JSON output of cargo-about")?;
    let mut notices = Vec::new();
    let mut named = Vec::new();
    for license in &about.licenses {
        let mut users: Vec<String> = license.used_by.iter().map(|u| u.package.user()).collect();
        users.sort();
        if license.text.trim().is_empty() {
            bail!(
                "the {} license text of {} is empty",
                license.id,
                users.join(", ")
            );
        }
        named.extend(users.iter().cloned());
        notices.push(Notice {
            license: license.id.clone(),
            users,
            text: license.text.trim().to_string(),
        });
    }
    for item in &about.crates {
        let user = item.package.user();
        if !named.contains(&user) {
            bail!("no license text names the crate {user}");
        }
        let dir = Path::new(&item.package.manifest_path)
            .parent()
            .with_context(|| format!("the manifest of {user} has no directory"))?;
        for file in notice_files(dir)? {
            let text = std::fs::read_to_string(&file)
                .with_context(|| format!("read {}", file.display()))?;
            notices.push(Notice {
                license: "NOTICE".into(),
                users: vec![user.clone()],
                text: text.trim().to_string(),
            });
        }
    }
    Ok(notices)
}

/// The NOTICE files of the crate in `dir`, in name order.
fn notice_files(dir: &Path) -> Result<Vec<std::path::PathBuf>> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_ascii_uppercase();
        if name.starts_with("NOTICE") && entry.file_type()?.is_file() {
            files.push(entry.path());
        }
    }
    files.sort();
    Ok(files)
}

/// One npm package: its license, and the text of its license file when
/// it has one.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub identifier: Option<String>,
    pub text: Option<String>,
}

/// The packages of `license_list`, a license list that Vite writes.
pub fn vite_packages(license_list: &str) -> Result<Vec<Package>> {
    serde_json::from_str(license_list).context("read the license list of Vite")
}

/// The packages of `license_list`, the output of `pnpm licenses list
/// --json`, with the text of the license file of each one. pnpm lists
/// the packages by license, and the directory of each copy of a package.
/// With the hoisted layout one version can have more than one copy, so
/// the version comes from the `package.json` of each directory, and the
/// first copy of a version stands for the others.
pub fn pnpm_packages(license_list: &str) -> Result<Vec<Package>> {
    #[derive(Deserialize)]
    struct Listed {
        name: String,
        paths: Vec<std::path::PathBuf>,
        license: String,
    }
    #[derive(Deserialize)]
    struct Manifest {
        version: String,
    }
    let listed: BTreeMap<String, Vec<Listed>> =
        serde_json::from_str(license_list).context("read the license list of pnpm")?;
    let mut packages: Vec<Package> = Vec::new();
    for item in listed.into_values().flatten() {
        for path in item.paths {
            let manifest = path.join("package.json");
            let Manifest { version } = serde_json::from_str(
                &std::fs::read_to_string(&manifest)
                    .with_context(|| format!("read {}", manifest.display()))?,
            )
            .with_context(|| format!("read the version in {}", manifest.display()))?;
            if packages
                .iter()
                .any(|package| package.name == item.name && package.version == version)
            {
                continue;
            }
            packages.push(Package {
                name: item.name.clone(),
                version,
                identifier: Some(item.license.clone()),
                text: license_file(&path)?,
            });
        }
    }
    Ok(packages)
}

/// The text of the license file in the package directory `dir`, as Vite
/// finds it: the first file whose name starts with `license`, `licence`
/// or `copying` in any case. A directory, as `LICENSES/` of the REUSE
/// layout, is not a license file.
fn license_file(dir: &Path) -> Result<Option<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if ["license", "licence", "copying"]
            .iter()
            .any(|prefix| lower.starts_with(prefix))
            && entry.file_type()?.is_file()
        {
            names.push(name);
        }
    }
    names.sort();
    names
        .first()
        .map(|name| {
            std::fs::read_to_string(dir.join(name))
                .with_context(|| format!("read {}", dir.join(name).display()))
        })
        .transpose()
}

/// The notices of the npm packages `packages`. Packages with the same
/// license and the same text share one notice. A package whose license is
/// not an SPDX expression that `accepted` satisfies is an error. A package
/// with no license file gets the standard text of each license that its
/// accepted choice requires.
pub fn package_notices(packages: Vec<Package>, accepted: &[String]) -> Result<Vec<Notice>> {
    let licensees = accepted
        .iter()
        .map(|license| {
            spdx::Licensee::parse(license)
                .with_context(|| format!("{ABOUT_CONFIG} accepts {license}, which is not SPDX"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut grouped: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    for package in packages {
        let user = format!("{} {}", package.name, package.version);
        let Some(identifier) = package.identifier else {
            bail!("the npm package {user} names no license");
        };
        let expression = spdx::Expression::parse(&identifier).with_context(|| {
            format!("the license {identifier} of the npm package {user} is not SPDX")
        })?;
        let required = expression.minimized_requirements(&licensees).map_err(|_| {
            anyhow::anyhow!(
                "{ABOUT_CONFIG} does not accept the license {identifier} of the npm package {user}"
            )
        })?;
        let text = match package.text.filter(|text| !text.trim().is_empty()) {
            Some(text) => text.trim().to_string(),
            None => standard_texts(&required)
                .with_context(|| format!("the npm package {user} has no license text"))?,
        };
        grouped.entry((identifier, text)).or_default().push(user);
    }
    Ok(grouped
        .into_iter()
        .map(|((license, text), mut users)| {
            users.sort();
            Notice {
                license,
                users,
                text,
            }
        })
        .collect())
}

/// The standard SPDX text of each license of `required`.
fn standard_texts(required: &[spdx::LicenseReq]) -> Result<String> {
    let texts = required
        .iter()
        .map(|req| match req.license {
            spdx::LicenseItem::Spdx { id, .. } => Ok(id.text().trim().to_string()),
            spdx::LicenseItem::Other { .. } => bail!("{req} has no standard text"),
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(texts.join("\n\n"))
}

const DOUBLE_RULE: &str = "======================================================================";
const RULE: &str = "----------------------------------------------------------------------";

/// The notices file of `product`: a heading, then each part with its
/// notices in the order of their license and their first user.
pub fn render(product: &str, parts: &[Part]) -> String {
    let mut out = format!(
        "Third-party notices of {product}\n\n\
         This file gives the license of each piece of third-party software\n\
         in the product above. Pagis itself is distributed under the MIT\n\
         License. Each notice gives a license, the software that uses it,\n\
         and the license text. The name and the version of a crate find its\n\
         source code on crates.io, and those of an npm package find its\n\
         source code on the npm registry.\n"
    );
    for part in parts {
        let _ = write!(out, "\n{DOUBLE_RULE}\n{}\n{DOUBLE_RULE}\n", part.title);
        let mut notices: Vec<&Notice> = part.notices.iter().collect();
        notices.sort_by(|a, b| (&a.license, &a.users).cmp(&(&b.license, &b.users)));
        for notice in notices {
            let _ = write!(out, "\n{RULE}\nLicense: {}\nUsed by:\n", notice.license);
            for user in &notice.users {
                let _ = writeln!(out, "  {user}");
            }
            let _ = write!(out, "\n{}\n", notice.text);
        }
    }
    out
}

/// The notices of the Server Package from `crates_json`, the JSON output
/// of cargo-about for `pagis`, and the files of the workspace at `root`.
pub fn server_notices(root: &Path, crates_json: &str) -> Result<String> {
    let read = |path: &str| {
        std::fs::read_to_string(root.join(path)).with_context(|| format!("read {path}"))
    };
    let accepted = accepted_licenses(&read(ABOUT_CONFIG)?)?;
    let mut packages = Vec::new();
    for list in PRODUCT_APP_LICENSES {
        let body = read(list).context(
            "the Product App license list is missing; build the Product App first \
             (`pnpm --filter ./ui run build`)",
        )?;
        packages.extend(package_notices(vite_packages(&body)?, &accepted)?);
    }
    let gog = Notice {
        license: "MIT".into(),
        users: vec![format!("gog {}", pagis_versions::GOG_VERSION)],
        text: read(GOG_LICENSE)?.trim().to_string(),
    };
    Ok(render(
        "the Pagis Server Package",
        &[
            Part {
                title: "The gog program".into(),
                notices: vec![gog],
            },
            Part {
                title: "The Rust crates of the pagis program".into(),
                notices: crate_notices(crates_json)?,
            },
            Part {
                title: "The npm packages of the Product App, which the pagis program holds".into(),
                notices: packages,
            },
        ],
    ))
}

/// The notices of the Client App from `license_list`, the output of
/// `pnpm licenses list --prod --json` for it. electron-builder also puts
/// the notices of Electron and of Chromium into the app, as
/// `LICENSE.electron.txt` and `LICENSES.chromium.html`.
pub fn client_notices(license_list: &str, accepted: &[String]) -> Result<String> {
    Ok(render(
        "the Pagis Client App",
        &[Part {
            title: "The npm packages of the Client App. LICENSE.electron.txt and \
                    LICENSES.chromium.html in the app hold the notices of \
                    Electron and Chromium."
                .into(),
            notices: package_notices(pnpm_packages(license_list)?, accepted)?,
        }],
    ))
}

/// The notices of `pagis-screend` from `crates_json`, the JSON output of
/// cargo-about for it.
pub fn screend_notices(crates_json: &str) -> Result<String> {
    Ok(render(
        "pagis-screend, the screen daemon of the Pagis Computer Image",
        &[Part {
            title: "The Rust crates of the pagis-screend program".into(),
            notices: crate_notices(crates_json)?,
        }],
    ))
}

/// The notices of the Coding Harnesses from `license_list`, the output of
/// `pnpm licenses list --prod --json` for the harness lockfile. A package
/// of [`VENDOR_TERMS`] gives its license file, and each other package
/// goes through [`package_notices`].
pub fn harness_notices(license_list: &str, accepted: &[String]) -> Result<String> {
    let (vendor, spdx): (Vec<Package>, Vec<Package>) = pnpm_packages(license_list)?
        .into_iter()
        .partition(|package| VENDOR_TERMS.contains(&package.name.as_str()));
    let mut notices = package_notices(spdx, accepted)?;
    for package in vendor {
        let user = format!("{} {}", package.name, package.version);
        let Some(text) = package.text.filter(|text| !text.trim().is_empty()) else {
            bail!("the npm package {user} has no license file");
        };
        notices.push(Notice {
            license: "the terms of its vendor".into(),
            users: vec![user],
            text: text.trim().to_string(),
        });
    }
    Ok(render(
        "the Coding Harnesses of the Pagis Computer Image",
        &[Part {
            title: "The npm packages of the Coding Harnesses. The native package of \
                    an architecture, as @openai/codex-linux-x64, has the license of \
                    its main package."
                .into(),
            notices,
        }],
    ))
}

/// A command that writes the JSON output of the pinned cargo-about for
/// the crate of `manifest` and `targets` to `output`. It fetches the
/// crates of the lockfile first, so cargo-about reads each license from
/// the local crate sources and reaches no network itself.
pub fn cargo_about_cmd(
    root: &Path,
    target_dir: &Path,
    manifest: &str,
    targets: &[&str],
    output: &Path,
) -> Cmd {
    let targets: String = targets
        .iter()
        .map(|target| format!(" --target {target}"))
        .collect();
    let body = format!(
        "cargo fetch --locked --manifest-path \"$1\"\n\
         \"$work/cargo-about\" generate --format json --frozen --fail \
         --config {ABOUT_CONFIG} --manifest-path \"$1\"{targets} --output-file \"$2\"\n"
    );
    let output = output.to_string_lossy();
    tools::command(
        root,
        target_dir,
        tools::cargo_about(),
        &body,
        &[manifest, &output],
    )
}

/// The gate step `notices`: build the Product App for its license list,
/// write the notices of the Server Package and of the Client App into the
/// target directory, and check the stored notices of screend and of the
/// Coding Harnesses.
pub fn notices_step(root: &Path) -> Step {
    let output = target_dir(root).join("notices/THIRD_PARTY_NOTICES");
    let output = output.to_string_lossy();
    let client = target_dir(root).join("notices/client/THIRD_PARTY_NOTICES");
    let client = client.to_string_lossy();
    let xtask = |args: &[&str]| {
        let mut all = vec!["run", "--package", "xtask", "--", "notices"];
        all.extend(args);
        Cmd::new("cargo", &all).in_dir(root)
    };
    Step {
        name: "notices",
        action: Action::Run(vec![
            Cmd::new("pnpm", &["--filter", "./ui", "run", "build"]).in_dir(root),
            xtask(&["server", &output]),
            xtask(&["client", &client]),
            xtask(&["screend", "--check"]),
            xtask(&["harnesses", "--check"]),
        ]),
    }
}

/// `cargo xtask notices server <output>`: write the notices of the
/// Server Package to `output`.
pub fn write_server(root: &Path, output: &Path) -> Result<()> {
    let json = run_cargo_about(root, "pagis", SERVER_MANIFEST, &[])?;
    write(output, &server_notices(root, &json)?)
}

/// `cargo xtask notices client <output>`: write the notices of the
/// Client App to `output`, from the production packages that pnpm
/// installed for it.
pub fn write_client(root: &Path, output: &Path) -> Result<()> {
    let listed = pnpm(
        root,
        &[
            "licenses",
            "list",
            "--prod",
            "--json",
            "--filter",
            "./desktop",
        ],
    )?;
    let text = client_notices(&listed, &read_accepted(root)?)?;
    write(output, &text)
}

/// `cargo xtask notices harnesses [--check]`: install the packages of the
/// harness lockfile into the target directory with no optional package
/// and no script, and write [`HARNESS_NOTICES`], or fail when it is not
/// what the command writes. pnpm does not list the optional native
/// packages, so the install leaves them out.
pub fn write_harnesses(root: &Path, check: bool) -> Result<()> {
    let dir = target_dir(root).join("notices/harnesses");
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    for file in HARNESS_FILES {
        let from = root.join(HARNESS_DIR).join(file);
        std::fs::copy(&from, dir.join(file)).with_context(|| format!("copy {}", from.display()))?;
    }
    pnpm(
        &dir,
        &[
            "install",
            "--frozen-lockfile",
            "--prod",
            "--no-optional",
            "--ignore-scripts",
        ],
    )?;
    let listed = pnpm(&dir, &["licenses", "list", "--prod", "--json"])?;
    let text = harness_notices(&listed, &read_accepted(root)?)?;
    store(root, HARNESS_NOTICES, &text, check, "harnesses")
}

/// Run pnpm with `args` in `dir`, and return its standard output.
fn pnpm(dir: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("pnpm")
        .args(args)
        .current_dir(dir)
        .output()
        .context("start pnpm")?;
    if !output.status.success() {
        bail!(
            "pnpm {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The accepted licenses of [`ABOUT_CONFIG`] at `root`.
fn read_accepted(root: &Path) -> Result<Vec<String>> {
    accepted_licenses(
        &std::fs::read_to_string(root.join(ABOUT_CONFIG))
            .with_context(|| format!("read {ABOUT_CONFIG}"))?,
    )
}

/// Write `text` to `path`, and make its directory first.
fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

/// `cargo xtask notices screend [--check]`: write [`SCREEND_NOTICES`],
/// or fail when it is not what the command writes.
pub fn write_screend(root: &Path, check: bool) -> Result<()> {
    let json = run_cargo_about(root, "pagis-screend", SCREEND_MANIFEST, &SCREEND_TARGETS)?;
    store(
        root,
        SCREEND_NOTICES,
        &screend_notices(&json)?,
        check,
        "screend",
    )
}

/// Write `text` to the stored notices `path` at `root`. With `check`,
/// write nothing, and fail when the stored file is not `text`. `command`
/// names the `cargo xtask notices` command that writes the file.
fn store(root: &Path, path: &str, text: &str, check: bool, command: &str) -> Result<()> {
    let full = root.join(path);
    if check {
        let stored = std::fs::read_to_string(&full).unwrap_or_default();
        if stored != text {
            bail!("{path} does not match its lockfile; run `cargo xtask notices {command}`");
        }
        return Ok(());
    }
    std::fs::write(&full, text).with_context(|| format!("write {}", full.display()))
}

/// Run the pinned cargo-about for `manifest`, and return its JSON output.
fn run_cargo_about(root: &Path, name: &str, manifest: &str, targets: &[&str]) -> Result<String> {
    let target = target_dir(root);
    let output = target.join("notices").join(format!("{name}.json"));
    std::fs::create_dir_all(output.parent().expect("a notices directory"))?;
    let cmd = cargo_about_cmd(root, &target, manifest, targets, &output);
    if crate::run_cmds("cargo-about", &[cmd]) != crate::Outcome::Passed {
        bail!("cargo-about failed for {manifest}");
    }
    std::fs::read_to_string(&output).with_context(|| format!("read {}", output.display()))
}
