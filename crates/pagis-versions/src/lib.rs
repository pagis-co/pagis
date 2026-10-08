//! Release versions shared by runtime crates and development tools.

pub const COMPUTER_IMAGE_VERSION: &str = "0.21.0";
pub const COMPUTER_IMAGE: &str = match option_env!("PAGIS_COMPUTER_IMAGE") {
    Some(image) => image,
    None => "ghcr.io/pagis-co/pagis-computer:0.21.0",
};
/// gog, the runner of the Google Connections, which each release bundles
/// and the gate checks the Google adapter against.
pub const GOG_VERSION: &str = "0.42.0";
/// The SHA-256 of the gog release archive of each platform that a
/// release bundles, from the checksums file of the gogcli release. The
/// platform name is the one in the archive name.
pub const GOG_SHA256: [(&str, &str); 3] = [
    (
        "darwin_arm64",
        "6a92b35473ed057c55677c2ba7d5af8e154d1bad23fa35e74994bc2f3bce4672",
    ),
    (
        "linux_amd64",
        "1967a962a57d689958c408dd0abc784792c3712da9d0a90650bb76ab7e3de388",
    ),
    (
        "linux_arm64",
        "84ce3002acea162596068c8b25e364aade634d204ae6122b714e686ca783b028",
    ),
];

/// gitleaks, which scans the tracked tree in the gate and the filesystem
/// of each image before the release pushes it.
pub const GITLEAKS_VERSION: &str = "8.30.1";
/// The SHA-256 of the gitleaks release archive of each host platform
/// that runs the gate or the release, from the checksums file of the
/// gitleaks release. The platform name is the one in the archive name.
pub const GITLEAKS_SHA256: [(&str, &str); 3] = [
    (
        "darwin_arm64",
        "b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5",
    ),
    (
        "linux_x64",
        "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb",
    ),
    (
        "linux_arm64",
        "e4a487ee7ccd7d3a7f7ec08657610aa3606637dab924210b3aee62570fb4b080",
    ),
];

/// cargo-deny, which checks the Cargo lockfiles against the published
/// advisories in `cargo xtask advisories`, which the daily advisory workflow
/// and `cargo xtask release` run.
pub const CARGO_DENY_VERSION: &str = "0.20.2";
/// The SHA-256 of the cargo-deny release archive of each host platform
/// that runs the gate or the release, from the `.sha256` file of each
/// archive of the cargo-deny release. The platform name is the target
/// triple in the archive name.
pub const CARGO_DENY_SHA256: [(&str, &str); 3] = [
    (
        "aarch64-apple-darwin",
        "fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3",
    ),
    (
        "x86_64-unknown-linux-musl",
        "9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f",
    ),
    (
        "aarch64-unknown-linux-musl",
        "995c82be0defc7a025cae49a2aa2644ce8245c9a3318fc4103907c6a285e8c7d",
    ),
];

/// cargo-auditable, which writes the list of the crates of an executable
/// into it, so Trivy identifies them: `pagis` in the Headless Server image
/// and in each Server Package, and `pagis-screend` in the Computer Image.
/// `Dockerfile` and `computer/Dockerfile` install this version from
/// crates.io.
pub const CARGO_AUDITABLE_VERSION: &str = "0.7.6";
/// The SHA-256 of the cargo-auditable release archive of each platform
/// that the release runs it on, from the `.sha256` file of each archive of
/// the cargo-auditable release. The macOS arm64 host builds the macOS
/// Server Package, and the images of `cross`, which are linux/amd64, build
/// the Linux ones. The platform name is the target triple in the archive
/// name.
pub const CARGO_AUDITABLE_SHA256: [(&str, &str); 2] = [
    (
        "aarch64-apple-darwin",
        "bf42fb077380f41a72ab9e217651bffcb6d46e641852a79ad1358bcc308323ff",
    ),
    (
        "x86_64-unknown-linux-musl",
        "42b66c852fbb9074a9ca356279a92eb753f48dde16017b8c82f48dcd05d6c856",
    ),
];

/// Trivy, which scans the Computer Image in `cargo xtask advisories`, and
/// each image and Server Package before the release pushes or publishes
/// it, for known vulnerabilities.
pub const TRIVY_VERSION: &str = "0.74.0";
/// The SHA-256 of the Trivy release archive of each host platform that
/// runs the gate or the release, from the checksums file of the Trivy
/// release. The platform name is the one in the archive name.
pub const TRIVY_SHA256: [(&str, &str); 3] = [
    (
        "macOS-ARM64",
        "1caada5e0e2091909357c7525d3aa76f4b660b13821bc143b190c7483e31cc11",
    ),
    (
        "Linux-64bit",
        "2ae6fe3ee734b7fdf11335663e18c75ea12dccc76062f09f164a3b0f8be4371a",
    ),
    (
        "Linux-ARM64",
        "b94ce1976bbf3c15b514b605ee88be7c6d94a29be2302847ff01cb794d47aad5",
    ),
];
