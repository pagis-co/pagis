//! The release build (`cargo xtask release <stage>`): the server half of
//! a release — the pinned Computer image and the headless server image on
//! GHCR, and a draft GitHub Release that holds the server package of each
//! Client App platform and the Runtime Lock that names it.
//!
//! A release is seven stages ([`ReleaseStage`]), and the release workflow
//! runs each one in a job of its own on the host that it needs: each
//! architecture of each image on a Linux runner of that architecture, the
//! image manifests and the Linux server packages on Linux, the macOS
//! server package and the draft release on macOS. macOS arm64 builds natively;
//! the Linux targets build through `cross`, which runs the toolchain in a
//! Docker container. Each build of `pagis` is a `cargo auditable` build
//! (see `build_action`).

use std::fs;
use std::io::{BufReader, Read};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::image::{ImagePlatform, builder_step, computer_image_steps, computer_manifest_step};
use crate::server_image::{server_image_steps, server_manifest_step};
use crate::{Action, Cmd, Step, tools};
use anyhow::{Context, Result, bail};
use pagis_versions::{CARGO_AUDITABLE_SHA256, GOG_SHA256};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The GitHub repository the release is published to and the Client App
/// downloads from.
pub const REPO: &str = "pagis-co/pagis";

/// The directory the release assets are written to.
pub const DIST_DIR: &str = "dist";

const COMPUTER_IMAGE_PREFIX: &str = "ghcr.io/pagis-co/pagis-computer@sha256:";

/// The platforms a Client App runs on. Each one embeds its own Runtime
/// Lock, which names the server package of that platform. The names are
/// Node's, because the client compares them with its own process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientPlatform {
    /// macOS arm64: a signed, notarized disk image.
    MacArm64,
    /// Linux amd64: a gzip tar archive.
    LinuxX64,
    /// Linux arm64: a gzip tar archive.
    LinuxArm64,
}

impl ClientPlatform {
    pub const ALL: [ClientPlatform; 3] = [
        ClientPlatform::MacArm64,
        ClientPlatform::LinuxX64,
        ClientPlatform::LinuxArm64,
    ];
    pub const LINUX: [ClientPlatform; 2] = [ClientPlatform::LinuxX64, ClientPlatform::LinuxArm64];

    /// The platform from its `<platform>-<arch>` name, as in
    /// `linux-x64`.
    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|platform| platform.name() == name)
            .with_context(|| {
                format!("{name} is not a Client App platform; use darwin-arm64, linux-x64 or linux-arm64")
            })
    }

    pub fn name(self) -> String {
        format!("{}-{}", self.platform(), self.arch())
    }

    /// Node's `process.platform`.
    pub fn platform(self) -> &'static str {
        match self {
            ClientPlatform::MacArm64 => "darwin",
            ClientPlatform::LinuxX64 | ClientPlatform::LinuxArm64 => "linux",
        }
    }

    /// Node's `process.arch`.
    pub fn arch(self) -> &'static str {
        match self {
            ClientPlatform::MacArm64 | ClientPlatform::LinuxArm64 => "arm64",
            ClientPlatform::LinuxX64 => "x64",
        }
    }

    /// The Rust target the server package of this platform is built for.
    pub fn triple(self) -> &'static str {
        match self {
            ClientPlatform::MacArm64 => "aarch64-apple-darwin",
            ClientPlatform::LinuxX64 => "x86_64-unknown-linux-gnu",
            ClientPlatform::LinuxArm64 => "aarch64-unknown-linux-gnu",
        }
    }

    /// The release asset the client downloads: the disk image on macOS,
    /// the gzip tar archive on Linux.
    pub fn server_package(self, release: &str) -> String {
        match self {
            ClientPlatform::MacArm64 => server_dmg_name(release),
            ClientPlatform::LinuxX64 | ClientPlatform::LinuxArm64 => {
                format!("pagis-server-{release}-{}.tar.gz", self.triple())
            }
        }
    }

    /// The release asset and the `dist/` file that hold this platform's
    /// Runtime Lock.
    pub fn lock_file(self) -> String {
        format!("runtime-lock-{}.json", self.name())
    }

    fn format(self) -> &'static str {
        match self {
            ClientPlatform::MacArm64 => "dmg",
            ClientPlatform::LinuxX64 | ClientPlatform::LinuxArm64 => "tar.gz",
        }
    }
}

/// The final bytes a Runtime Lock is written from.
pub struct RuntimeLockRequest<'a> {
    pub release: &'a str,
    pub computer_image: &'a str,
    pub platform: ClientPlatform,
    /// The team identifier read from the signing certificate. macOS
    /// needs it, and Linux has no platform signature to name.
    pub team_id: Option<&'a str>,
    pub package_dir: &'a Path,
    pub asset: &'a Path,
}

/// The lock of each platform has its own shape: macOS names the signing
/// team and each executable's code identifier, and Linux names neither.
#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "platform")]
enum RuntimeLock {
    #[serde(rename = "darwin")]
    Darwin(PlatformLock<MacAsset, MacEntry>),
    #[serde(rename = "linux")]
    Linux(PlatformLock<LinuxAsset, LinuxEntry>),
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct PlatformLock<A, E> {
    schema: u8,
    release: String,
    arch: String,
    asset: A,
    entries: Vec<E>,
    computer_image: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct MacAsset {
    format: String,
    name: String,
    url: String,
    size: u64,
    sha256: String,
    team_id: String,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct LinuxAsset {
    format: String,
    name: String,
    url: String,
    size: u64,
    sha256: String,
}

/// A macOS entry. An executable carries its code identifier, and a
/// plain file carries none.
#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct MacEntry {
    path: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    codesign_id: Option<String>,
    size: u64,
    sha256: String,
    mode: u32,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct LinuxEntry {
    path: String,
    kind: String,
    size: u64,
    sha256: String,
    mode: u32,
}

/// One file of the server package, the same on every platform.
struct LayoutEntry {
    path: &'static str,
    kind: &'static str,
    codesign_id: Option<&'static str>,
    mode: u32,
}

/// The five root files of every server package, in lock order.
const LAYOUT: [LayoutEntry; 5] = [
    LayoutEntry {
        path: "pagis",
        kind: "executable",
        codesign_id: Some("com.pagis.server"),
        mode: 0o755,
    },
    LayoutEntry {
        path: "gog",
        kind: "executable",
        codesign_id: Some("com.pagis.gog"),
        mode: 0o755,
    },
    LayoutEntry {
        path: "LICENSE",
        kind: "file",
        codesign_id: None,
        mode: 0o644,
    },
    LayoutEntry {
        path: "LICENSE.gog",
        kind: "file",
        codesign_id: None,
        mode: 0o644,
    },
    LayoutEntry {
        path: "THIRD_PARTY_NOTICES",
        kind: "file",
        codesign_id: None,
        mode: 0o644,
    },
];

/// The measured facts of one package file.
struct MeasuredEntry {
    path: String,
    kind: String,
    codesign_id: Option<String>,
    size: u64,
    sha256: String,
    mode: u32,
}

/// Write the client Runtime Lock from the final server package and the
/// exact package tree used to make it: the mounted disk image on macOS,
/// the extracted archive on Linux. The function rejects any other tree or
/// a mutable Computer image reference.
pub fn write_runtime_lock(request: &RuntimeLockRequest<'_>, output: &Path) -> Result<()> {
    let lock = build_runtime_lock(request)?;
    let bytes = serde_json::to_vec_pretty(&lock).context("serialize runtime lock")?;
    fs::write(output, bytes).with_context(|| format!("write {}", output.display()))
}

/// Recompute every locked byte from a downloaded server package and its
/// opened root. Release creation and publication both use this check.
pub fn validate_runtime_lock(lock_path: &Path, package_dir: &Path, asset: &Path) -> Result<()> {
    let locked = read_lock(lock_path)?;
    let platform = locked.platform()?;
    let request = RuntimeLockRequest {
        release: locked.release(),
        computer_image: locked.computer_image(),
        platform,
        team_id: locked.team_id(),
        package_dir,
        asset,
    };
    let actual = build_runtime_lock(&request)?;
    if actual != locked {
        bail!(
            "the final server asset does not match {}",
            lock_path.display()
        );
    }
    Ok(())
}

/// Validate the signed client's lock without reading the server package.
/// Routine packaging uses this check with a deterministic fixture. A tagged
/// release also compares the lock and server bytes with the published tuple.
pub fn validate_runtime_lock_metadata(
    lock_path: &Path,
    release: &str,
    platform: ClientPlatform,
) -> Result<()> {
    let locked = read_lock(lock_path)?;
    if locked.release() != release || locked.platform().ok() != Some(platform) {
        bail!(
            "{} does not name release {release} on {}",
            lock_path.display(),
            platform.name()
        );
    }
    validate_image_digest(locked.computer_image())?;
    let (format, name, url, size, sha256) = match &locked {
        RuntimeLock::Darwin(lock) => {
            validate_team_id(&lock.asset.team_id)?;
            let a = &lock.asset;
            (&a.format, &a.name, &a.url, a.size, &a.sha256)
        }
        RuntimeLock::Linux(lock) => {
            let a = &lock.asset;
            (&a.format, &a.name, &a.url, a.size, &a.sha256)
        }
    };
    let expected_name = platform.server_package(release);
    if format != platform.format()
        || *name != expected_name
        || *url != asset_url(release, &expected_name)
        || size == 0
    {
        bail!("{} has invalid server asset metadata", lock_path.display());
    }
    validate_sha256("server asset", sha256)?;
    let entries = locked.entries();
    if entries.len() != LAYOUT.len() {
        bail!(
            "{} has an invalid server entry inventory",
            lock_path.display()
        );
    }
    for (entry, spec) in entries.iter().zip(&LAYOUT) {
        let codesign_id = signed_identity(platform, spec);
        if entry.path != spec.path
            || entry.kind != spec.kind
            || entry.codesign_id.as_deref() != codesign_id
            || entry.mode != spec.mode
            || entry.size == 0
        {
            bail!(
                "{} has invalid metadata for {}",
                lock_path.display(),
                spec.path
            );
        }
        validate_sha256(spec.path, &entry.sha256)?;
    }
    Ok(())
}

/// Write a deterministic, non-distribution lock for the unsigned package
/// smoke. Publication requires the final lock and the server bytes it
/// names on the release, so this fixture cannot pass a tag build.
pub fn write_fixture_runtime_lock(
    release: &str,
    platform: ClientPlatform,
    output: &Path,
) -> Result<()> {
    let hash = "0".repeat(64);
    let entries = LAYOUT
        .iter()
        .map(|spec| MeasuredEntry {
            path: spec.path.into(),
            kind: spec.kind.into(),
            codesign_id: signed_identity(platform, spec).map(str::to_string),
            size: 1,
            sha256: hash.clone(),
            mode: spec.mode,
        })
        .collect();
    let lock = assemble_lock(
        release,
        &format!("{COMPUTER_IMAGE_PREFIX}{}", "0".repeat(64)),
        platform,
        Some("FIXTURE000"),
        (1, hash.clone()),
        entries,
    )?;
    let parent = output
        .parent()
        .context("the runtime lock has no parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    fs::write(output, serde_json::to_vec_pretty(&lock)?)
        .with_context(|| format!("write {}", output.display()))
}

impl RuntimeLock {
    fn platform(&self) -> Result<ClientPlatform> {
        let (platform, arch) = match self {
            RuntimeLock::Darwin(lock) => ("darwin", &lock.arch),
            RuntimeLock::Linux(lock) => ("linux", &lock.arch),
        };
        ClientPlatform::parse(&format!("{platform}-{arch}"))
    }

    fn release(&self) -> &str {
        match self {
            RuntimeLock::Darwin(lock) => &lock.release,
            RuntimeLock::Linux(lock) => &lock.release,
        }
    }

    fn computer_image(&self) -> &str {
        match self {
            RuntimeLock::Darwin(lock) => &lock.computer_image,
            RuntimeLock::Linux(lock) => &lock.computer_image,
        }
    }

    fn team_id(&self) -> Option<&str> {
        match self {
            RuntimeLock::Darwin(lock) => Some(&lock.asset.team_id),
            RuntimeLock::Linux(_) => None,
        }
    }

    fn entries(&self) -> Vec<MeasuredEntry> {
        match self {
            RuntimeLock::Darwin(lock) => lock
                .entries
                .iter()
                .map(|e| MeasuredEntry {
                    path: e.path.clone(),
                    kind: e.kind.clone(),
                    codesign_id: e.codesign_id.clone(),
                    size: e.size,
                    sha256: e.sha256.clone(),
                    mode: e.mode,
                })
                .collect(),
            RuntimeLock::Linux(lock) => lock
                .entries
                .iter()
                .map(|e| MeasuredEntry {
                    path: e.path.clone(),
                    kind: e.kind.clone(),
                    codesign_id: None,
                    size: e.size,
                    sha256: e.sha256.clone(),
                    mode: e.mode,
                })
                .collect(),
        }
    }
}

fn read_lock(lock_path: &Path) -> Result<RuntimeLock> {
    let bytes = fs::read(lock_path).with_context(|| format!("read {}", lock_path.display()))?;
    let locked: RuntimeLock =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", lock_path.display()))?;
    let schema = match &locked {
        RuntimeLock::Darwin(lock) => lock.schema,
        RuntimeLock::Linux(lock) => lock.schema,
    };
    if schema != 1 {
        bail!("{} has an unsupported schema", lock_path.display());
    }
    locked.platform()?;
    Ok(locked)
}

/// The code identifier a locked entry carries: macOS executables only.
fn signed_identity(platform: ClientPlatform, spec: &LayoutEntry) -> Option<&'static str> {
    match platform {
        ClientPlatform::MacArm64 => spec.codesign_id,
        ClientPlatform::LinuxX64 | ClientPlatform::LinuxArm64 => None,
    }
}

fn asset_url(release: &str, name: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/v{release}/{name}")
}

/// Put measured facts into the lock shape of their platform.
fn assemble_lock(
    release: &str,
    computer_image: &str,
    platform: ClientPlatform,
    team_id: Option<&str>,
    (size, sha256): (u64, String),
    entries: Vec<MeasuredEntry>,
) -> Result<RuntimeLock> {
    let name = platform.server_package(release);
    let url = asset_url(release, &name);
    Ok(match platform {
        ClientPlatform::MacArm64 => {
            let team_id =
                team_id.context("a macOS Runtime Lock needs the signing team identifier")?;
            validate_team_id(team_id)?;
            RuntimeLock::Darwin(PlatformLock {
                schema: 1,
                release: release.into(),
                arch: platform.arch().into(),
                asset: MacAsset {
                    format: platform.format().into(),
                    name,
                    url,
                    size,
                    sha256,
                    team_id: team_id.into(),
                },
                entries: entries
                    .into_iter()
                    .map(|e| MacEntry {
                        path: e.path,
                        kind: e.kind,
                        codesign_id: e.codesign_id,
                        size: e.size,
                        sha256: e.sha256,
                        mode: e.mode,
                    })
                    .collect(),
                computer_image: computer_image.into(),
            })
        }
        ClientPlatform::LinuxX64 | ClientPlatform::LinuxArm64 => RuntimeLock::Linux(PlatformLock {
            schema: 1,
            release: release.into(),
            arch: platform.arch().into(),
            asset: LinuxAsset {
                format: platform.format().into(),
                name,
                url,
                size,
                sha256,
            },
            entries: entries
                .into_iter()
                .map(|e| LinuxEntry {
                    path: e.path,
                    kind: e.kind,
                    size: e.size,
                    sha256: e.sha256,
                    mode: e.mode,
                })
                .collect(),
            computer_image: computer_image.into(),
        }),
    })
}

fn validate_sha256(name: &str, hash: &str) -> Result<()> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("the runtime lock has an invalid SHA-256 for {name}");
    }
    Ok(())
}

fn validate_team_id(team_id: &str) -> Result<()> {
    if team_id.is_empty()
        || !team_id
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
    {
        bail!("the Developer ID team identifier is invalid");
    }
    Ok(())
}

fn build_runtime_lock(request: &RuntimeLockRequest<'_>) -> Result<RuntimeLock> {
    validate_image_digest(request.computer_image)?;
    let platform = request.platform;
    if platform == ClientPlatform::MacArm64 {
        validate_team_id(request.team_id.unwrap_or_default())?;
    } else if request.team_id.is_some() {
        bail!("a Linux Runtime Lock names no signing team");
    }

    let asset_name = request
        .asset
        .file_name()
        .and_then(|name| name.to_str())
        .context("the server package has no UTF-8 file name")?;
    let expected_name = platform.server_package(request.release);
    if asset_name != expected_name {
        bail!("the server package is {asset_name}; expected {expected_name}");
    }

    let mut actual = fs::read_dir(request.package_dir)
        .with_context(|| format!("read {}", request.package_dir.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .context("read server package entry")
        })
        .collect::<Result<Vec<_>>>()?;
    actual.sort();
    let mut expected = LAYOUT
        .iter()
        .map(|spec| spec.path.to_string())
        .collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        let missing = expected
            .iter()
            .filter(|name| !actual.contains(name))
            .cloned()
            .collect::<Vec<_>>();
        let extra = actual
            .iter()
            .filter(|name| !expected.contains(name))
            .cloned()
            .collect::<Vec<_>>();
        bail!(
            "the server package layout is invalid; missing: {}; unexpected: {}",
            missing.join(", "),
            extra.join(", ")
        );
    }

    let entries = LAYOUT
        .iter()
        .map(|spec| {
            let path = spec.path;
            let expected_mode = spec.mode;
            let full = request.package_dir.join(path);
            let metadata = fs::symlink_metadata(&full)
                .with_context(|| format!("read server package entry {path}"))?;
            if !metadata.file_type().is_file() {
                bail!("server package entry {path} is not a regular file");
            }
            #[cfg(unix)]
            let mode = metadata.permissions().mode() & 0o777;
            #[cfg(not(unix))]
            let mode = expected_mode;
            if mode != expected_mode {
                bail!("server package entry {path} has mode {mode:o}; expected {expected_mode:o}");
            }
            Ok(MeasuredEntry {
                path: path.to_string(),
                kind: spec.kind.to_string(),
                codesign_id: signed_identity(platform, spec).map(str::to_string),
                size: metadata.len(),
                sha256: sha256(&full)?,
                mode,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let asset_metadata = fs::metadata(request.asset)
        .with_context(|| format!("read final server asset {}", request.asset.display()))?;
    if !asset_metadata.is_file() {
        bail!("the final server asset is not a regular file");
    }
    assemble_lock(
        request.release,
        request.computer_image,
        platform,
        request.team_id,
        (asset_metadata.len(), sha256(request.asset)?),
        entries,
    )
}

fn validate_image_digest(image: &str) -> Result<()> {
    let Some(digest) = image.strip_prefix(COMPUTER_IMAGE_PREFIX) else {
        bail!("the release needs an immutable Computer image from {COMPUTER_IMAGE_PREFIX}");
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("the release needs an immutable Computer image with a lowercase SHA-256 digest");
    }
    Ok(())
}

fn sha256(path: &Path) -> Result<String> {
    let file = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .with_context(|| format!("hash {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn server_dmg_name(version: &str) -> String {
    format!("pagis-server-{version}-aarch64-apple-darwin.dmg")
}

/// How a target's binary is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builder {
    /// The host toolchain, for the host's own platform.
    Cargo,
    /// `cross`: the target toolchain inside a Docker container.
    Cross,
}

impl Builder {
    /// The pinned cargo-auditable of the place where the build runs it: the
    /// macOS arm64 host for `cargo`, and the linux/amd64 image of the target
    /// for `cross`.
    fn cargo_auditable(self) -> (&'static str, &'static str) {
        match self {
            Builder::Cargo => CARGO_AUDITABLE_SHA256[0],
            Builder::Cross => CARGO_AUDITABLE_SHA256[1],
        }
    }
}

/// The Dockerfile of the image that `cross` builds a Linux `pagis` in: the
/// default image of the target (`CROSS_BASE_IMAGE`), with the pinned
/// cargo-auditable and the `cargo` of [`CROSS_CARGO`] in `/usr/local/bin`.
///
/// The images of `cross` are linux/amd64 only, and the pinned
/// cargo-auditable of the image is an x86_64 executable. A build with no
/// platform on an arm64 host asks for linux/arm64 and finds no image. So
/// the `FROM` names the platform, and Docker runs the image under
/// emulation on an arm64 host.
const CROSS_DOCKERFILE: &str = "\
ARG CROSS_BASE_IMAGE
FROM --platform=linux/amd64 $CROSS_BASE_IMAGE
COPY cargo cargo-auditable /usr/local/bin/
";

/// The `cargo` of the image of `cross`. `cross` starts `cargo` by name, and
/// the directory of the toolchain is last in PATH, after `/usr/local/bin`.
/// So this file starts first, and it starts the next `cargo` in PATH, which
/// is the toolchain's own, as `cargo auditable`. cargo-auditable documents
/// this `cargo` as its drop-in replacement for cargo (`REPLACING_CARGO.md`).
const CROSS_CARGO: &str = r#"#!/bin/sh
IFS=:
for dir in $PATH; do
  if [ -x "$dir/cargo" ] && ! [ "$dir/cargo" -ef "$0" ]; then
    exec "$dir/cargo" auditable "$@"
  fi
done
echo 'no cargo in PATH other than this one' >&2
exit 127
"#;

struct Target {
    triple: &'static str,
    builder: Builder,
    /// True for the target of the macOS stage.
    macos: bool,
    /// The platform of the bundled `gog` archive, and its pinned SHA-256.
    gog_platform: &'static str,
    gog_checksum: &'static str,
}

/// The platforms a release ships: macOS arm64 and Linux on both
/// architectures.
const TARGETS: [Target; 3] = [
    Target {
        triple: "aarch64-apple-darwin",
        builder: Builder::Cargo,
        macos: true,
        gog_platform: GOG_SHA256[0].0,
        gog_checksum: GOG_SHA256[0].1,
    },
    Target {
        triple: "x86_64-unknown-linux-gnu",
        builder: Builder::Cross,
        macos: false,
        gog_platform: GOG_SHA256[1].0,
        gog_checksum: GOG_SHA256[1].1,
    },
    Target {
        triple: "aarch64-unknown-linux-gnu",
        builder: Builder::Cross,
        macos: false,
        gog_platform: GOG_SHA256[2].0,
        gog_checksum: GOG_SHA256[2].1,
    },
];

/// The stages of a release, in the order the release workflow runs them.
/// Each stage runs on the host it needs, and the files it writes under
/// [`DIST_DIR`] are the input of the next stages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseStage {
    /// Build, scan and push the Computer image of each platform by digest.
    ComputerImage,
    /// Join the Computer image digests under the pinned tag, and resolve
    /// the immutable digest of the tag.
    ComputerManifest,
    /// Build, scan and push the Headless Server image of each platform by
    /// digest, against the immutable Computer image.
    ServerImage,
    /// Join the Headless Server image digests under the release tag, and
    /// pull both images with no credentials.
    ServerManifest,
    /// Build the Linux server packages and their Runtime Locks.
    Linux,
    /// Build, sign and notarize the macOS server package and its lock.
    Macos,
    /// Validate every server package and lock, and create the draft
    /// GitHub Release that holds them.
    Draft,
}

impl ReleaseStage {
    pub const ALL: [ReleaseStage; 7] = [
        ReleaseStage::ComputerImage,
        ReleaseStage::ComputerManifest,
        ReleaseStage::ServerImage,
        ReleaseStage::ServerManifest,
        ReleaseStage::Linux,
        ReleaseStage::Macos,
        ReleaseStage::Draft,
    ];

    pub fn parse(name: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|stage| stage.name() == name)
            .with_context(|| {
                format!(
                    "{name} is not a release stage; use computer-image, computer-manifest, \
                     server-image, server-manifest, linux, macos or draft"
                )
            })
    }

    pub fn name(self) -> &'static str {
        match self {
            ReleaseStage::ComputerImage => "computer-image",
            ReleaseStage::ComputerManifest => "computer-manifest",
            ReleaseStage::ServerImage => "server-image",
            ReleaseStage::ServerManifest => "server-manifest",
            ReleaseStage::Linux => "linux",
            ReleaseStage::Macos => "macos",
            ReleaseStage::Draft => "draft",
        }
    }

    /// True for the stages that only a macOS host runs: `codesign`,
    /// `notarytool` and `hdiutil` exist only there.
    pub fn needs_macos(self) -> bool {
        matches!(self, ReleaseStage::Macos | ReleaseStage::Draft)
    }
}

/// What the release is built from: the workspace version, the pinned
/// computer image, whether the registry already holds that image, and
/// the platforms the image stages build.
#[derive(Debug, Clone)]
pub struct ReleaseContext {
    pub version: String,
    pub image: String,
    /// True when the registry already holds the pinned Computer image. A
    /// published image version is never pushed again, so every release
    /// that pins it pulls the same bytes.
    pub computer_published: bool,
    /// The platforms that the computer-image and server-image stages
    /// build. A release job builds one, on a runner of that architecture.
    pub platforms: Vec<ImagePlatform>,
    pub target_dir: std::path::PathBuf,
}

/// The stock upstream binary archive bundled for one Pagis target.
pub fn gog_asset_name(target: &str) -> String {
    let platform = TARGETS
        .iter()
        .find(|candidate| candidate.triple == target)
        .unwrap_or_else(|| panic!("no gog asset for target {target}"))
        .gog_platform;
    format!("gogcli_{}_{}.tar.gz", pagis_versions::GOG_VERSION, platform)
}

/// Plan the steps of one release `stage` for the workspace at `root`.
pub fn release_plan(root: &Path, cx: &ReleaseContext, stage: ReleaseStage) -> Vec<Step> {
    match stage {
        ReleaseStage::ComputerImage => {
            let mut steps = vec![builder_step(root)];
            for platform in &cx.platforms {
                steps.extend(computer_image_steps(
                    root,
                    &cx.image,
                    *platform,
                    &cx.target_dir,
                ));
            }
            skip_when_published(cx, steps)
        }
        ReleaseStage::ComputerManifest => {
            let mut steps = skip_when_published(cx, vec![computer_manifest_step(root, &cx.image)]);
            steps.push(Step {
                name: "image-digest",
                action: image_digest_action(root, cx),
            });
            steps
        }
        ReleaseStage::ServerImage => server_images_plan(root, cx),
        ReleaseStage::ServerManifest => vec![
            server_manifest_step(root, &crate::server_image(&cx.version)),
            // A new person pulls both images with no login, so the
            // release does too before it builds anything that names them.
            crate::anonymous_pull_step(root, &[&cx.image, &crate::server_image(&cx.version)]),
        ],
        ReleaseStage::Linux => server_package_plan(root, cx, false),
        ReleaseStage::Macos => server_package_plan(root, cx, true),
        ReleaseStage::Draft => vec![Step {
            name: "draft",
            action: draft_action(root, cx),
        }],
    }
}

/// Each image is built and exported, its filesystem is scanned for
/// secrets and for known vulnerabilities, and only then is it pushed: a
/// pushed layer of a public package cannot be recalled. A published
/// Computer image version is never pushed again, so its steps skip.
fn skip_when_published(cx: &ReleaseContext, steps: Vec<Step>) -> Vec<Step> {
    if !cx.computer_published {
        return steps;
    }
    steps
        .into_iter()
        .map(|step| Step {
            name: step.name,
            action: Action::Skip(format!(
                "{} is on the registry; a published image version is never pushed again",
                cx.image
            )),
        })
        .collect()
}

/// The headless Linux server image, built against the immutable
/// Computer image that the computer-manifest stage resolved. It is the
/// fourth artifact of the release and it carries the same release
/// number as the server packages.
fn server_images_plan(root: &Path, cx: &ReleaseContext) -> Vec<Step> {
    let server_image = crate::server_image(&cx.version);
    let computer = format!("{DIST_DIR}/computer-image.txt");
    let mut steps = vec![builder_step(root)];
    for platform in &cx.platforms {
        steps.extend(server_image_steps(
            root,
            &server_image,
            *platform,
            Some(&computer),
            &cx.target_dir,
        ));
    }
    steps
}

/// Build, assemble and scan the server package of each target of the
/// stage, then pack it: the disk image on macOS, the archives on Linux.
/// Both read the immutable Computer image that the computer-manifest
/// stage wrote to `dist/computer-image.txt`.
fn server_package_plan(root: &Path, cx: &ReleaseContext, macos: bool) -> Vec<Step> {
    let targets: Vec<&Target> = TARGETS.iter().filter(|t| t.macos == macos).collect();
    let mut steps = vec![Step {
        name: "ui-build",
        action: ui_build_action(root),
    }];
    for target in &targets {
        steps.push(Step {
            name: build_step_name(target.triple),
            action: build_action(root, target, cx),
        });
    }
    steps.push(Step {
        name: "assemble-server",
        action: package_action(root, &targets, cx),
    });
    // Trivy identifies `gog` and the crates of `pagis` in each package
    // tree before the release signs, packs and publishes it.
    let package_dirs: Vec<String> = targets
        .iter()
        .map(|target| format!("{DIST_DIR}/package-{}", target.triple))
        .collect();
    steps.push(crate::advisories::vuln_scan_step(
        root,
        "server-package-vuln-scan",
        &package_dirs,
        &cx.target_dir,
        &[],
    ));
    if macos {
        steps.push(Step {
            name: "macos-server-dmg",
            action: macos_server_dmg_action(root, cx),
        });
    } else {
        steps.push(Step {
            name: "linux-server-archives",
            action: linux_archives_action(root, cx),
        });
        steps.push(Step {
            name: "linux-runtime-locks",
            action: linux_locks_action(root, cx),
        });
    }
    steps
}

fn image_digest_action(root: &Path, cx: &ReleaseContext) -> Action {
    let script = format!(
        "set -eu\nmkdir -p {DIST_DIR}\ndigest=$(docker buildx imagetools inspect '{}' --format '{{{{.Manifest.Digest}}}}')\ncase \"$digest\" in sha256:[0-9a-f][0-9a-f]*) ;; *) echo 'the Computer image did not resolve to a SHA-256 digest' >&2; exit 1 ;; esac\nprintf '%s@%s\\n' '{}' \"$digest\" > {DIST_DIR}/computer-image.txt\n",
        cx.image,
        cx.image.split(':').next().unwrap_or(&cx.image),
    );
    Action::Run(vec![
        Cmd::new("sh", &["-c", &script])
            .in_dir(root)
            .env("CARGO_TARGET_DIR", &cx.target_dir.to_string_lossy()),
    ])
}

/// Step names are `&'static str`, so the per-target names come from a
/// lookup over the fixed target list.
fn build_step_name(triple: &str) -> &'static str {
    match triple {
        "aarch64-apple-darwin" => "build-aarch64-apple-darwin",
        "x86_64-unknown-linux-gnu" => "build-x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu" => "build-aarch64-unknown-linux-gnu",
        other => unreachable!("no step name for target {other}"),
    }
}

/// Build the SPA before the binaries: the release build embeds
/// `ui/dist` into the executable (rust-embed), so a stale or missing
/// bundle would ship as the served UI.
fn ui_build_action(root: &Path) -> Action {
    let ui = root.join("ui");
    Action::Run(vec![
        Cmd::new("npm", &["ci"]).in_dir(ui.clone()),
        Cmd::new("npm", &["run", "build"]).in_dir(ui),
    ])
}

/// Build `pagis` for `target` with `cargo auditable build`. cargo-auditable
/// writes the list of the crates of `pagis` into the executable, so Trivy
/// identifies them in the Server Package. The step checks the pinned
/// cargo-auditable against its SHA-256 (see [`tools::command`]).
///
/// `cross` runs `cargo build` in the Linux image of the target, but it runs
/// every other cargo subcommand, such as `auditable`, on the host. So the
/// Linux build gives `cross` an image of its own (`CROSS_BUILD_DOCKERFILE`,
/// see [`CROSS_DOCKERFILE`]), in which `cargo` is `cargo auditable`.
fn build_action(root: &Path, target: &Target, cx: &ReleaseContext) -> Action {
    let build = format!("build --release -p pagis --target {}", target.triple);
    let build = match target.builder {
        Builder::Cargo => format!("PATH=\"$work:$PATH\" cargo auditable {build}\n"),
        Builder::Cross => format!(
            "cat > \"$work/cargo\" <<'EOF'\n{CROSS_CARGO}EOF\n\
             cat > \"$work/Dockerfile\" <<'EOF'\n{CROSS_DOCKERFILE}EOF\n\
             chmod 755 \"$work/cargo\"\n\
             CROSS_BUILD_DOCKERFILE=\"$work/Dockerfile\" CROSS_BUILD_DOCKERFILE_CONTEXT=\"$work\" \
             cross {build}\n"
        ),
    };
    let body = format!(
        "PAGIS_COMPUTER_IMAGE=$(cat {DIST_DIR}/computer-image.txt)\nexport PAGIS_COMPUTER_IMAGE\n{build}"
    );
    let pin = tools::cargo_auditable(target.builder.cargo_auditable());
    Action::Run(vec![
        tools::command(root, &cx.target_dir, Ok(pin), &body, &[])
            .env("CARGO_TARGET_DIR", &cx.target_dir.to_string_lossy()),
    ])
}

/// Put each built binary, the pinned gog binary, and the license files
/// into one package tree per target.
fn package_action(root: &Path, built: &[&Target], cx: &ReleaseContext) -> Action {
    let mut script = format!("set -eu\nrm -rf {DIST_DIR}/package-*\nmkdir -p {DIST_DIR}\n");
    for target in built {
        let gog_asset = gog_asset_name(target.triple);
        let package_dir = format!("{DIST_DIR}/package-{}", target.triple);
        script.push_str(&format!(
            "curl -fsSL https://github.com/openclaw/gogcli/releases/download/v{}/{gog_asset} -o {DIST_DIR}/{gog_asset}\n",
            pagis_versions::GOG_VERSION,
        ));
        script.push_str(&format!(
            "echo '{}  {DIST_DIR}/{gog_asset}' | shasum -a 256 -c -\n",
            target.gog_checksum,
        ));
        script.push_str(&format!(
            "test -f ui/dist/index.html || {{ echo 'the Product App build is missing' >&2; exit 1; }}\nif find ui/dist -type f -newer \"$CARGO_TARGET_DIR/{}/release/pagis\" -print -quit | grep -q .; then echo 'the server binary contains a stale embedded Product App' >&2; exit 1; fi\nmkdir -p {package_dir}\ncp \"$CARGO_TARGET_DIR/{}/release/pagis\" {package_dir}/pagis\ntar -xzf {DIST_DIR}/{gog_asset} -C {package_dir} ./gog\ncp LICENSE {package_dir}/LICENSE\ncp third_party/gog/LICENSE {package_dir}/LICENSE.gog\ncp third_party/THIRD_PARTY_NOTICES {package_dir}/THIRD_PARTY_NOTICES\nchmod 755 {package_dir}/pagis {package_dir}/gog\nchmod 644 {package_dir}/LICENSE {package_dir}/LICENSE.gog {package_dir}/THIRD_PARTY_NOTICES\n",
            target.triple,
            target.triple,
        ));
    }
    Action::Run(vec![
        Cmd::new("sh", &["-c", &script])
            .in_dir(root)
            .env("CARGO_TARGET_DIR", &cx.target_dir.to_string_lossy()),
    ])
}

/// Pack the Linux server package of each architecture. The Linux Client
/// App installs it; the macOS Client App installs the disk image.
fn linux_archives_action(root: &Path, cx: &ReleaseContext) -> Action {
    // macOS `tar` would add an AppleDouble member for each file that has
    // extended attributes. The Linux Client App refuses any member its
    // lock does not name, so the archives carry the five files alone.
    let mut script = "set -eu\nexport COPYFILE_DISABLE=1\n".to_string();
    for platform in ClientPlatform::LINUX {
        let package_dir = format!("{DIST_DIR}/package-{}", platform.triple());
        let asset = platform.server_package(&cx.version);
        script.push_str(&format!(
            "tar -czf {DIST_DIR}/{asset} -C {package_dir} pagis gog LICENSE LICENSE.gog THIRD_PARTY_NOTICES\n"
        ));
    }
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

/// Write the Runtime Lock of each Linux Client App from the finished
/// server archive of its architecture. The lock names the archive's
/// exact bytes, and each file's bytes and mode as the archive extracts
/// them.
fn linux_locks_action(root: &Path, cx: &ReleaseContext) -> Action {
    let mut script = "set -eu\n".to_string();
    for platform in ClientPlatform::LINUX {
        let archive = format!("{DIST_DIR}/{}", platform.server_package(&cx.version));
        script.push_str(&format!(
            "extracted=$(mktemp -d)\n\
             tar -xzpf {archive} -C \"$extracted\"\n\
             cargo run --quiet -p xtask -- runtime-lock write {} {} \"$(cat {DIST_DIR}/computer-image.txt)\" {archive} \"$extracted\" {DIST_DIR}/{}\n\
             rm -rf \"$extracted\"\n",
            platform.name(),
            cx.version,
            platform.lock_file(),
        ));
    }
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

/// The shell function `notarize <file>`: submit the file to Apple's notary
/// service and wait for the verdict. A local release names a notarytool
/// keychain profile. CI names an App Store Connect API key: the path of
/// its `.p8` file, its key ID and its issuer ID.
pub(crate) const NOTARIZE_FN: &str = r#"notarize() {
  file=$1
  profile=${PAGIS_NOTARY_KEYCHAIN_PROFILE:-${APPLE_KEYCHAIN_PROFILE:-}}
  if [ -n "$profile" ]; then
    set -- --keychain-profile "$profile"
    if [ -n "${APPLE_KEYCHAIN:-}" ]; then set -- "$@" --keychain "$APPLE_KEYCHAIN"; fi
  else
    : "${APPLE_API_KEY:?APPLE_API_KEY or APPLE_KEYCHAIN_PROFILE is required}"
    : "${APPLE_API_KEY_ID:?APPLE_API_KEY_ID is required}"
    : "${APPLE_API_ISSUER:?APPLE_API_ISSUER is required}"
    set -- --key "$APPLE_API_KEY" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER"
  fi
  /usr/bin/xcrun notarytool submit "$file" "$@" --wait
}
"#;

fn macos_server_dmg_action(root: &Path, cx: &ReleaseContext) -> Action {
    let package_dir = format!("{DIST_DIR}/package-aarch64-apple-darwin");
    let dmg = format!("{DIST_DIR}/{}", server_dmg_name(&cx.version));
    let mac_lock = ClientPlatform::MacArm64.lock_file();
    let script = format!(
        "set -eu\n\
         {NOTARIZE_FN}\
         identity=${{PAGIS_SERVER_SIGN_IDENTITY:-${{CSC_NAME:-}}}}\n\
         if [ -z \"$identity\" ]; then\n\
           identity=$(security find-identity -v -p codesigning | sed -n 's/.*) \\([0-9A-F][0-9A-F]*\\) \"Developer ID Application:.*/\\1/p' | head -1)\n\
         fi\n\
         [ -n \"$identity\" ] || {{ echo 'a Developer ID Application identity is required to package the server' >&2; exit 1; }}\n\
         codesign --force --options runtime --timestamp --identifier com.pagis.server --sign \"$identity\" {package_dir}/pagis\n\
         codesign --force --options runtime --timestamp --identifier com.pagis.gog --sign \"$identity\" {package_dir}/gog\n\
         codesign --verify --strict --verbose=2 {package_dir}/pagis\n\
         codesign --verify --strict --verbose=2 {package_dir}/gog\n\
         codesign -dv --verbose=4 {package_dir}/pagis 2>&1 | grep '^Authority=Developer ID Application:' >/dev/null || {{ echo 'pagis is not signed with Developer ID Application' >&2; exit 1; }}\n\
         team=$(codesign -dv --verbose=4 {package_dir}/pagis 2>&1 | sed -n 's/^TeamIdentifier=//p')\n\
         [ -n \"$team\" ] || {{ echo 'the pagis signature has no team identifier' >&2; exit 1; }}\n\
         rm -f {dmg}\n\
         hdiutil create -fs HFS+ -format UDZO -volname 'Pagis Server {}' -srcfolder {package_dir} {dmg}\n\
         codesign --force --timestamp --sign \"$identity\" {dmg}\n\
         codesign --verify --strict --verbose=2 {dmg}\n\
         notarize {dmg}\n\
         xcrun stapler staple {dmg}\n\
         xcrun stapler validate {dmg}\n\
         mount={DIST_DIR}/mounted-server\n\
         rm -rf \"$mount\" && mkdir -p \"$mount\"\n\
         hdiutil attach -readonly -nobrowse -mountpoint \"$mount\" {dmg}\n\
         trap 'hdiutil detach \"$mount\" >/dev/null' EXIT\n\
         cargo run --quiet -p xtask -- runtime-lock write darwin-arm64 {} \"$(cat {DIST_DIR}/computer-image.txt)\" {dmg} \"$mount\" {DIST_DIR}/{mac_lock} \"$team\"\n\
         hdiutil detach \"$mount\" >/dev/null\n\
         trap - EXIT\n",
        cx.version, cx.version
    );
    Action::Run(vec![Cmd::new("sh", &["-c", &script]).in_dir(root)])
}

fn server_validation_cmd(root: &Path, cx: &ReleaseContext) -> Cmd {
    let dmg = format!("{DIST_DIR}/{}", server_dmg_name(&cx.version));
    let mut required = String::new();
    let mut linux_locks = String::new();
    for platform in ClientPlatform::ALL {
        required.push_str(&format!(
            "test -f {DIST_DIR}/{lock} || {{ echo 'missing {lock}' >&2; exit 1; }}\n",
            lock = platform.lock_file(),
        ));
    }
    for platform in ClientPlatform::LINUX {
        let archive = format!("{DIST_DIR}/{}", platform.server_package(&cx.version));
        required.push_str(&format!(
            "test -f {archive} || {{ echo 'missing server archive for {}' >&2; exit 1; }}\n",
            platform.name()
        ));
        linux_locks.push_str(&format!(
            "actual=$(tar -tzf {archive} | sed 's#^./##' | LC_ALL=C sort)\n\
             [ \"$actual\" = \"$expected\" ] || {{ echo 'unexpected package layout in {archive}' >&2; exit 1; }}\n\
             extracted=$(mktemp -d)\n\
             tar -xzpf {archive} -C \"$extracted\"\n\
             cargo run --quiet -p xtask -- runtime-lock validate {DIST_DIR}/{} {archive} \"$extracted\"\n\
             rm -rf \"$extracted\"\n",
            platform.lock_file(),
        ));
    }
    let mac_lock = ClientPlatform::MacArm64.lock_file();
    let script = format!(
        "set -eu\n\
         {required}\
         test -f {dmg} || {{ echo 'missing signed macOS server DMG' >&2; exit 1; }}\n\
         expected=$(printf '%s\\n' LICENSE LICENSE.gog THIRD_PARTY_NOTICES gog pagis)\n\
         {linux_locks}\
         codesign --verify --strict --verbose=2 {dmg}\n\
         xcrun stapler validate {dmg}\n\
         mount={DIST_DIR}/validate-mounted-server\n\
         rm -rf \"$mount\" && mkdir -p \"$mount\"\n\
         hdiutil attach -readonly -nobrowse -mountpoint \"$mount\" {dmg}\n\
         trap 'hdiutil detach \"$mount\" >/dev/null' EXIT\n\
         codesign --verify --strict --verbose=2 \"$mount/pagis\"\n\
         codesign --verify --strict --verbose=2 \"$mount/gog\"\n\
         codesign -dr - \"$mount/pagis\" 2>&1 | grep 'identifier \"com.pagis.server\"' >/dev/null\n\
         codesign -dr - \"$mount/gog\" 2>&1 | grep 'identifier \"com.pagis.gog\"' >/dev/null\n\
         env -i PATH=/usr/bin:/bin \"$mount/pagis\" package-check\n\
         cargo run --quiet -p xtask -- runtime-lock validate {DIST_DIR}/{mac_lock} {dmg} \"$mount\"\n\
         hdiutil detach \"$mount\" >/dev/null\n\
         trap - EXIT\n"
    );
    Cmd::new("sh", &["-c", &script]).in_dir(root)
}

/// Validate every server package and Runtime Lock, then create the draft
/// GitHub Release of the tag with them, and nothing else. The tag must
/// exist: the release workflow runs on its push. After a maintainer
/// approves the release, the publication jobs attach the Client App
/// packages to the draft and publish it (`docs/RELEASING-CLIENT.md`).
fn draft_action(root: &Path, cx: &ReleaseContext) -> Action {
    let tag = format!("v{}", cx.version);
    let title = format!("pagis {}", cx.version);
    let mut args = vec![
        "release".to_string(),
        "create".to_string(),
        tag,
        "--repo".to_string(),
        REPO.to_string(),
        "--draft".to_string(),
        "--verify-tag".to_string(),
        "--title".to_string(),
        title,
        "--generate-notes".to_string(),
    ];
    for platform in ClientPlatform::LINUX {
        args.push(format!(
            "{DIST_DIR}/{}",
            platform.server_package(&cx.version)
        ));
    }
    args.push(format!("{DIST_DIR}/{}", server_dmg_name(&cx.version)));
    for platform in ClientPlatform::ALL {
        args.push(format!("{DIST_DIR}/{}", platform.lock_file()));
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    Action::Run(vec![
        server_validation_cmd(root, cx),
        Cmd::new("gh", &args).in_dir(root),
    ])
}
