//! The pinned tools of the checks and the release: gitleaks, cargo-deny,
//! Trivy, gog and cargo-auditable.
//!
//! `pagis-versions` pins the version of each tool and the SHA-256 of its
//! release archive for each host that runs the gate or the release, and
//! for cargo-auditable also for the images of `cross` (see
//! [`cargo_auditable`]).
//!
//! A step checks the kept archive against the pin before each run, and
//! downloads it again when it is missing or does not match. The download
//! address of each tool is fixed here. The archives are kept in the target
//! directory, so one download serves each later run, and the checks need
//! no global install.

use pagis_versions::{
    CARGO_AUDITABLE_VERSION, CARGO_DENY_SHA256, CARGO_DENY_VERSION, GITLEAKS_SHA256,
    GITLEAKS_VERSION, GOG_SHA256, GOG_VERSION, TRIVY_SHA256, TRIVY_VERSION,
};
use std::path::Path;

use crate::Cmd;

/// The pinned release archive of one tool for this host.
#[derive(Debug, Clone)]
pub struct Pin {
    /// The name of the executable. It also names the directory of the
    /// target directory that keeps the archive.
    pub tool: &'static str,
    pub url: String,
    pub sha256: String,
    /// The path of the executable in the archive.
    pub member: String,
}

/// The pinned gitleaks archive of this host, or why there is none.
pub fn gitleaks() -> Result<Pin, String> {
    let (version, platform, sha256) = host_archive("gitleaks", GITLEAKS_VERSION, &GITLEAKS_SHA256)?;
    Ok(Pin {
        tool: "gitleaks",
        url: format!(
            "https://github.com/gitleaks/gitleaks/releases/download/v{version}/gitleaks_{version}_{platform}.tar.gz"
        ),
        sha256,
        member: "gitleaks".into(),
    })
}

/// The pinned cargo-deny archive of this host, or why there is none.
pub fn cargo_deny() -> Result<Pin, String> {
    let (version, triple, sha256) =
        host_archive("cargo-deny", CARGO_DENY_VERSION, &CARGO_DENY_SHA256)?;
    Ok(Pin {
        tool: "cargo-deny",
        url: format!(
            "https://github.com/EmbarkStudios/cargo-deny/releases/download/{version}/cargo-deny-{version}-{triple}.tar.gz"
        ),
        sha256,
        member: format!("cargo-deny-{version}-{triple}/cargo-deny"),
    })
}

/// The pinned Trivy archive of this host, or why there is none.
pub fn trivy() -> Result<Pin, String> {
    let (version, platform, sha256) = host_archive("trivy", TRIVY_VERSION, &TRIVY_SHA256)?;
    Ok(Pin {
        tool: "trivy",
        url: format!(
            "https://github.com/aquasecurity/trivy/releases/download/v{version}/trivy_{version}_{platform}.tar.gz"
        ),
        sha256,
        member: "trivy".into(),
    })
}

/// The pinned gog archive of this host, or why there is none.
pub fn gog() -> Result<Pin, String> {
    let (version, platform, sha256) = host_archive("gog", GOG_VERSION, &GOG_SHA256)?;
    Ok(Pin {
        tool: "gog",
        url: format!(
            "https://github.com/openclaw/gogcli/releases/download/v{version}/gogcli_{version}_{platform}.tar.gz"
        ),
        sha256,
        member: "./gog".into(),
    })
}

/// The pinned cargo-auditable archive of one platform of
/// `CARGO_AUDITABLE_SHA256`: the target triple where it runs, and the
/// SHA-256 of its archive. The release runs it on the macOS host and in the
/// linux/amd64 images of `cross`, so the platform is not always the host.
pub fn cargo_auditable((platform, sha256): (&'static str, &'static str)) -> Pin {
    Pin {
        tool: "cargo-auditable",
        url: format!(
            "https://github.com/rust-secure-code/cargo-auditable/releases/download/v{CARGO_AUDITABLE_VERSION}/cargo-auditable-{platform}.tar.xz"
        ),
        sha256: sha256.into(),
        member: format!("cargo-auditable-{platform}/cargo-auditable"),
    }
}

/// The position of this host in each archive table of `pagis-versions`,
/// which lists macOS arm64, Linux amd64 and Linux arm64 in that order, or
/// why `tool` has no pin for this host.
pub fn host(tool: &str) -> Result<usize, String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok(0),
        ("linux", "x86_64") => Ok(1),
        ("linux", "aarch64") => Ok(2),
        (os, arch) => Err(format!(
            "{tool} is pinned for macOS arm64, Linux amd64 and Linux arm64; this host is {os} {arch}"
        )),
    }
}

/// `version`, and the platform name and the SHA-256 of the archive of
/// this host in `archives`.
fn host_archive(
    tool: &str,
    version: &str,
    archives: &[(&str, &str); 3],
) -> Result<(String, String, String), String> {
    let (platform, sha256) = archives[host(tool)?];
    Ok((version.into(), platform.into(), sha256.into()))
}

/// A command that runs `body` in `sh` at `root` with the tool of `pin`.
///
/// Before `body`, it checks the kept archive against the pinned SHA-256,
/// downloads the archive again when it is missing or does not match, and
/// unpacks the executable to `$work/<tool>`. `$work` is a temporary
/// directory that the command removes when it ends, and `$cache` is the
/// directory that keeps the archive. `body` reads `args` as `$1` and on.
/// A host that has no pin gets a command that fails and says why.
pub fn command(
    root: &Path,
    target_dir: &Path,
    pin: Result<Pin, String>,
    body: &str,
    args: &[&str],
) -> Cmd {
    let Pin {
        tool,
        url,
        sha256,
        member,
    } = match pin {
        Ok(pin) => pin,
        Err(refusal) => return Cmd::refusal(&refusal).in_dir(root),
    };
    let asset = url.rsplit('/').next().unwrap_or(&url);
    // tar finds the compression of the archive, gzip or xz, by itself.
    let mut unpack = format!("tar -xf \"$archive\" -C \"$work\" {member}\n");
    if member.strip_prefix("./").unwrap_or(&member) != tool {
        unpack.push_str(&format!("mv \"$work/{member}\" \"$work/{tool}\"\n"));
    }
    let script = format!(
        "set -eu\n\
         cache=$1\n\
         shift\n\
         archive=\"$cache/{asset}\"\n\
         pinned() {{ echo '{sha256}  '\"$1\" | shasum -a 256 -c - >/dev/null 2>&1; }}\n\
         if ! pinned \"$archive\"; then\n\
           mkdir -p \"$cache\"\n\
           if ! curl -fsSL {url} -o \"$archive.$$\" || ! pinned \"$archive.$$\"; then\n\
             rm -f \"$archive.$$\"\n\
             echo 'the {tool} download failed or does not match its pinned SHA-256' >&2\n\
             exit 1\n\
           fi\n\
           mv \"$archive.$$\" \"$archive\"\n\
         fi\n\
         work=$(mktemp -d)\n\
         trap 'rm -rf \"$work\"' EXIT\n\
         {unpack}\
         {body}"
    );
    let cache = target_dir.join(tool).to_string_lossy().into_owned();
    let mut cmd_args = vec!["-c", &script, "sh", &cache];
    cmd_args.extend(args);
    Cmd::new("sh", &cmd_args).in_dir(root)
}
