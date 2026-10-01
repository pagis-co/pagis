//! The Linux Client App packaging (`cargo xtask desktop` on a Linux host,
//! or with `--linux`). It builds an AppImage and a deb package for amd64
//! and arm64 with electron-builder, checks what they embed, and smoke
//! tests the package of the host's architecture.
//!
//! Linux has no platform notary. A tagged release prepares the exact
//! packages, and after a maintainer approves the release it publishes
//! those bytes with a checksum list that the Pagis release key signs
//! (ADR-0025), and that the Update Key signs for the Client App
//! (ADR-0027). No key enters the plan: `gpg` reads the release key from
//! the keyring of the publication job, and the job reads the Update Key
//! from its environment. Each package embeds the public Update Key, and
//! the Update feed of each architecture names its two packages.

use std::path::Path;

use crate::desktop::DesktopContext;
use crate::image::ANONYMOUS_PULL_FN;
use crate::release::{ClientPlatform, REPO};
use crate::{Action, Cmd, Step};

/// The public half of the Pagis release key, in the repository. A
/// person checks the checksum signature with it, and publication checks
/// the signature against this exact file.
pub const RELEASE_KEY: &str = "docs/release-key.asc";

/// The public half of the Update Key, an Ed25519 key in SPKI PEM. Each
/// package embeds it, and the Client App checks the `.sig` of the checksum
/// list of an Update with it. Publication checks its signature against
/// this exact file.
pub const UPDATE_KEY: &str = "docs/update-key.pem";

/// The AppImage electron-builder writes for one architecture. It names
/// amd64 `x86_64`, as AppImage tools do.
pub fn appimage_name(version: &str, platform: ClientPlatform) -> String {
    let arch = match platform {
        ClientPlatform::LinuxX64 => "x86_64",
        _ => "arm64",
    };
    format!("Pagis-{version}-{arch}.AppImage")
}

/// The deb package electron-builder writes for one architecture, with
/// Debian's architecture name.
pub fn deb_name(version: &str, platform: ClientPlatform) -> String {
    let arch = match platform {
        ClientPlatform::LinuxX64 => "amd64",
        _ => "arm64",
    };
    format!("Pagis-{version}-{arch}.deb")
}

/// The Update feed that electron-updater reads for one architecture. It
/// names the AppImage and the deb, and electron-updater takes the file of
/// its own package.
pub fn feed_name(platform: ClientPlatform) -> &'static str {
    match platform {
        ClientPlatform::LinuxX64 => "latest-linux.yml",
        _ => "latest-linux-arm64.yml",
    }
}

/// The checksum list of the four Linux packages. Its detached signature
/// is this name with `.asc`.
pub fn checksums_name(version: &str) -> String {
    format!("Pagis-{version}-linux.SHA256SUMS")
}

/// The directory electron-builder unpacks one architecture into. The
/// default architecture, amd64, carries no suffix.
fn unpacked_dir(platform: ClientPlatform) -> &'static str {
    match platform {
        ClientPlatform::LinuxX64 => "release/linux-unpacked",
        _ => "release/linux-arm64-unpacked",
    }
}

/// The name of the Client App's executable in the package. It is not
/// `pagis`, which is the server's own command.
pub const EXECUTABLE: &str = "pagis-client";

/// The inputs a Linux publication needs that the environment does not
/// carry: the fingerprint of the release key in the local `gpg` agent,
/// and the private Update Key in PKCS#8 PEM.
pub fn missing_signing_inputs(is_set: &dyn Fn(&str) -> bool) -> Vec<String> {
    ["PAGIS_RELEASE_GPG_KEY", "PAGIS_UPDATE_SIGNING_KEY"]
        .into_iter()
        .filter(|name| !is_set(name))
        .map(String::from)
        .collect()
}

pub fn linux_plan(root: &Path, cx: &DesktopContext) -> Vec<Step> {
    let desktop = root.join("desktop");
    let reuse = "publishing the exact packages that were prepared";
    let validate_locks = ClientPlatform::LINUX
        .into_iter()
        .map(|platform| {
            Cmd::new(
                "cargo",
                &[
                    "run",
                    "--quiet",
                    "-p",
                    "xtask",
                    "--",
                    "runtime-lock",
                    "validate-metadata",
                    &format!("dist/{}", platform.lock_file()),
                    &cx.version,
                    &platform.name(),
                ],
            )
            .in_dir(root)
        })
        .collect();
    let build = |command: Cmd| {
        if cx.publish_existing {
            Action::Skip(reuse.into())
        } else {
            Action::Run(vec![command])
        }
    };
    vec![
        Step {
            name: "credentials",
            action: credentials_action(root, cx),
        },
        Step {
            name: "runtime-locks",
            action: Action::Run(validate_locks),
        },
        Step {
            name: "deps",
            action: build(Cmd::new("npm", &["ci"]).in_dir(desktop.clone())),
        },
        Step {
            name: "pack",
            action: build(Cmd::new("npm", &["run", "pack:linux"]).in_dir(desktop.clone())),
        },
        Step {
            name: "inventory",
            action: build(Cmd::new("sh", &["-c", &inventory_script(&cx.version)]).in_dir(&desktop)),
        },
        Step {
            name: "packaged-runtime",
            action: build(
                Cmd::new(
                    "node",
                    &[
                        "scripts/check-packaged-runtime.mjs",
                        unpacked_dir(ClientPlatform::LinuxX64),
                    ],
                )
                .in_dir(&desktop),
            ),
        },
        Step {
            name: "smoke",
            action: build(Cmd::new("sh", &["-c", SMOKE_SCRIPT]).in_dir(&desktop)),
        },
        Step {
            name: "checksums",
            action: build(Cmd::new("sh", &["-c", &checksums_script(&cx.version)]).in_dir(&desktop)),
        },
        Step {
            name: "released-tuple",
            action: released_tuple_action(root, cx),
        },
        Step {
            name: "sign-checksums",
            action: sign_action(root, cx),
        },
        Step {
            name: "publish",
            action: publish_action(root, cx),
        },
    ]
}

/// Check both architectures: the unpacked tree carries the client alone,
/// the lock of its architecture and the public release key, and the
/// finished deb carries the same lock and key. The deb is checked from its
/// exact bytes, which `dpkg-deb` can read on any architecture. It must
/// hold the `package-type` file from which electron-updater knows a deb.
/// The feed must name both packages with their true SHA-512 and size.
fn inventory_script(version: &str) -> String {
    let mut script = "set -eu\n".to_string();
    for platform in ClientPlatform::LINUX {
        let unpacked = unpacked_dir(platform);
        let lock = format!("../dist/{}", platform.lock_file());
        let key = format!("../{UPDATE_KEY}");
        let appimage = format!("release/{}", appimage_name(version, platform));
        let deb = format!("release/{}", deb_name(version, platform));
        let feed = format!("release/{}", feed_name(platform));
        script.push_str(&format!(
            "cmp {lock} {unpacked}/resources/runtime-lock.json\n\
             node scripts/check-package.mjs {unpacked} {appimage} {deb}\n\
             node scripts/check-update-feed.mjs {feed} {version} {appimage} {deb}\n\
             extracted=$(mktemp -d)\n\
             dpkg-deb -x {deb} \"$extracted\"\n\
             cmp {lock} \"$extracted/opt/Pagis/resources/runtime-lock.json\"\n\
             cmp {key} \"$extracted/opt/Pagis/resources/update-key.pem\"\n\
             [ \"$(cat \"$extracted/opt/Pagis/resources/package-type\")\" = deb ] || {{ echo '{deb} has no package-type file that names a deb' >&2; exit 1; }}\n\
             test -x \"$extracted/opt/Pagis/{EXECUTABLE}\"\n\
             test -f \"$extracted/opt/Pagis/resources/apparmor-profile\"\n\
             rm -rf \"$extracted\"\n",
        ));
    }
    script
}

/// Start the client of the host's architecture with no installed server.
/// It must open its local setup page. A machine with no display runs it
/// under a virtual one.
const SMOKE_SCRIPT: &str = concat!(
    "set -eu\n",
    "case \"$(uname -m)\" in\n",
    "  x86_64) app=release/linux-unpacked ;;\n",
    "  aarch64|arm64) app=release/linux-arm64-unpacked ;;\n",
    "  *) echo \"no Linux client for $(uname -m)\" >&2; exit 1 ;;\n",
    "esac\n",
    "[ -x \"$app/pagis-client\" ] || { echo \"the packaged client is missing from $app\" >&2; exit 1; }\n",
    "if [ -z \"${DISPLAY:-}\" ] && [ -z \"${WAYLAND_DISPLAY:-}\" ]; then\n",
    "  xvfb-run -a \"$app/pagis-client\" --smoke\n",
    "else\n",
    "  \"$app/pagis-client\" --smoke\n",
    "fi\n",
);

/// The checksum list of the four packages, beside them. A tag signs it
/// at publication; a routine build keeps it as the record of what it
/// built.
fn checksums_script(version: &str) -> String {
    let mut files = Vec::new();
    for platform in ClientPlatform::LINUX {
        files.push(appimage_name(version, platform));
        files.push(deb_name(version, platform));
    }
    format!(
        "set -eu\ncd release\nsha256sum {} > {}\n",
        files.join(" "),
        checksums_name(version)
    )
}

fn credentials_action(root: &Path, cx: &DesktopContext) -> Action {
    if cx.tag.is_none() || !cx.publish_existing {
        return Action::Skip(
            "only publication signs, with the release key and the Update Key".into(),
        );
    }
    if cx.missing_credentials.is_empty() {
        return Action::Run(vec![Cmd::new("sh", &["-c", "true"]).in_dir(root)]);
    }
    fail_action(
        root,
        &format!(
            "Linux publication needs {}",
            cx.missing_credentials.join("; ")
        ),
    )
}

/// The published server tuple must equal the locks the packages embed:
/// the lock files, the archives they name and the Computer image.
fn released_tuple_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: no published server tuple is required".into());
    };
    if cx.prepare_only {
        return Action::Skip("the prepared packages await publication".into());
    }
    let mut script =
        format!("set -eu\n{ANONYMOUS_PULL_FN}tmp=$(mktemp -d)\ntrap 'rm -rf \"$tmp\"' EXIT\n");
    for platform in ClientPlatform::LINUX {
        let lock = platform.lock_file();
        let archive = platform.server_package(&cx.version);
        script.push_str(&format!(
            "gh release download {tag} --repo {REPO} --dir \"$tmp\" --pattern {lock} --pattern {archive}\n\
             cmp dist/{lock} \"$tmp/{lock}\"\n\
             locked=$(node -e \"process.stdout.write(require('./dist/{lock}').asset.sha256)\")\n\
             actual=$(shasum -a 256 \"$tmp/{archive}\" | awk '{{print $1}}')\n\
             [ \"$actual\" = \"$locked\" ] || {{ echo 'the published {archive} does not match its Runtime Lock' >&2; exit 1; }}\n\
             image=$(node -e \"process.stdout.write(require('./dist/{lock}').computer_image)\")\n\
             anonymous_pull \"$image\"\n",
        ));
    }
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

/// Sign the checksum list with the release key for a person who checks it
/// with `gpg`, and with the Update Key for the Client App (ADR-0027). The
/// Update Key makes a raw Ed25519 signature of the exact list. Its private
/// half goes from the environment to a file that only the job user reads,
/// and the file is gone before the checks. Then prove each signature with
/// the public key that the repository publishes, and prove each listed
/// hash.
fn sign_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(_) = &cx.tag else {
        return Action::Skip("no release tag: nothing is signed".into());
    };
    if cx.prepare_only {
        return Action::Skip("the prepared packages await publication".into());
    }
    let sums = format!("desktop/release/{}", checksums_name(&cx.version));
    let script = format!(
        "set -eu\n\
         test -f {RELEASE_KEY} || {{ echo 'the release key {RELEASE_KEY} is missing; publication needs it' >&2; exit 1; }}\n\
         test -f {UPDATE_KEY} || {{ echo 'the Update Key {UPDATE_KEY} is missing; publication needs it' >&2; exit 1; }}\n\
         : \"${{PAGIS_RELEASE_GPG_KEY:?PAGIS_RELEASE_GPG_KEY names the release key}}\"\n\
         : \"${{PAGIS_UPDATE_SIGNING_KEY:?PAGIS_UPDATE_SIGNING_KEY holds the private Update Key}}\"\n\
         (cd desktop/release && sha256sum -c {name})\n\
         rm -f {sums}.asc {sums}.sig\n\
         gpg --batch --yes --armor --detach-sign --local-user \"$PAGIS_RELEASE_GPG_KEY\" --output {sums}.asc {sums}\n\
         keyring=$(mktemp)\n\
         key=$(mktemp)\n\
         trap 'rm -f \"$keyring\" \"$key\"' EXIT\n\
         chmod 600 \"$key\"\n\
         printf '%s\\n' \"$PAGIS_UPDATE_SIGNING_KEY\" > \"$key\"\n\
         openssl pkeyutl -sign -rawin -inkey \"$key\" -in {sums} -out {sums}.sig\n\
         rm -f \"$key\"\n\
         gpg --dearmor < {RELEASE_KEY} > \"$keyring\"\n\
         gpgv --keyring \"$keyring\" {sums}.asc {sums}\n\
         openssl pkeyutl -verify -pubin -inkey {UPDATE_KEY} -rawin -in {sums} -sigfile {sums}.sig\n",
        name = checksums_name(&cx.version),
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

/// Attach the four packages, the checksum list with its two signatures and
/// the two Update feeds to the draft release of the tag. `gh release
/// upload` without `--clobber` refuses a file that the draft holds.
fn publish_action(root: &Path, cx: &DesktopContext) -> Action {
    let Some(tag) = &cx.tag else {
        return Action::Skip("no release tag: the packages are built and smoke tested only".into());
    };
    if cx.prepare_only {
        return Action::Skip("the prepared packages await publication".into());
    }
    if !cx.missing_credentials.is_empty() {
        return fail_action(
            root,
            &format!(
                "Linux publication needs {}",
                cx.missing_credentials.join("; ")
            ),
        );
    }
    let mut files = Vec::new();
    for platform in ClientPlatform::LINUX {
        files.push(format!(
            "desktop/release/{}",
            appimage_name(&cx.version, platform)
        ));
        files.push(format!(
            "desktop/release/{}",
            deb_name(&cx.version, platform)
        ));
    }
    let sums = format!("desktop/release/{}", checksums_name(&cx.version));
    files.push(sums.clone());
    files.push(format!("{sums}.asc"));
    files.push(format!("{sums}.sig"));
    for platform in ClientPlatform::LINUX {
        files.push(format!("desktop/release/{}", feed_name(platform)));
    }
    let mut args = vec!["release", "upload", tag.as_str()];
    args.extend(files.iter().map(String::as_str));
    args.extend(["--repo", REPO]);
    Action::Run(vec![Cmd::new("gh", &args).in_dir(root)])
}

fn fail_action(root: &Path, reason: &str) -> Action {
    let script = format!("echo '{}' >&2; exit 1", reason.replace('\'', "'\\''"));
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}
