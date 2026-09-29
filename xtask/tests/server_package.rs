//! The public server-package contract consumed by the client release.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde_json::Value;
use xtask::release::{
    ClientPlatform, RuntimeLockRequest, validate_runtime_lock, validate_runtime_lock_metadata,
    write_fixture_runtime_lock, write_runtime_lock,
};

const IMAGE: &str = "ghcr.io/pagis-co/pagis-computer@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn package(root: &Path) {
    fs::create_dir_all(root).unwrap();
    for (name, body, mode) in [
        ("pagis", b"server".as_slice(), 0o755),
        ("gog", b"provider".as_slice(), 0o755),
        ("LICENSE", b"pagis license".as_slice(), 0o644),
        ("LICENSE.gog", b"license".as_slice(), 0o644),
        ("THIRD_PARTY_NOTICES", b"notices".as_slice(), 0o644),
    ] {
        let path = root.join(name);
        fs::write(&path, body).unwrap();
        #[cfg(unix)]
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}

fn request<'a>(package_dir: &'a Path, asset: &'a Path) -> RuntimeLockRequest<'a> {
    RuntimeLockRequest {
        release: "1.2.3",
        computer_image: IMAGE,
        platform: ClientPlatform::MacArm64,
        team_id: Some("AB12CD34EF"),
        package_dir,
        asset,
    }
}

fn linux_request<'a>(
    platform: ClientPlatform,
    package_dir: &'a Path,
    asset: &'a Path,
) -> RuntimeLockRequest<'a> {
    RuntimeLockRequest {
        release: "1.2.3",
        computer_image: IMAGE,
        platform,
        team_id: None,
        package_dir,
        asset,
    }
}

#[test]
fn final_server_bytes_produce_the_exact_client_runtime_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let package_dir = tmp.path().join("package");
    package(&package_dir);
    let asset = tmp
        .path()
        .join("pagis-server-1.2.3-aarch64-apple-darwin.dmg");
    fs::write(&asset, b"final notarized and stapled dmg").unwrap();
    let lock_path = tmp.path().join("runtime-lock.json");

    write_runtime_lock(&request(&package_dir, &asset), &lock_path).unwrap();

    let lock: Value = serde_json::from_slice(&fs::read(lock_path).unwrap()).unwrap();
    assert_eq!(lock["schema"], 1);
    assert_eq!(lock["release"], "1.2.3");
    assert_eq!(lock["platform"], "darwin");
    assert_eq!(lock["arch"], "arm64");
    assert_eq!(lock["computer_image"], IMAGE);
    assert_eq!(
        lock["asset"]["name"],
        "pagis-server-1.2.3-aarch64-apple-darwin.dmg"
    );
    assert_eq!(lock["asset"]["size"], 31);
    assert_eq!(
        lock["asset"]["sha256"],
        "a271034ad74d34ee5cd62b2d5297c72a753271f92a97a508563a0703eb59bc51"
    );
    assert_eq!(lock["asset"]["team_id"], "AB12CD34EF");
    let entries = lock["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[0]["path"], "pagis");
    assert_eq!(entries[0]["kind"], "executable");
    assert_eq!(entries[0]["codesign_id"], "com.pagis.server");
    assert_eq!(entries[0]["mode"], 493);
    assert_eq!(entries[1]["path"], "gog");
    assert_eq!(entries[1]["codesign_id"], "com.pagis.gog");
    assert_eq!(entries[2]["path"], "LICENSE");
    assert_eq!(entries[2]["kind"], "file");
    assert_eq!(entries[3]["path"], "LICENSE.gog");
    assert_eq!(entries[3]["kind"], "file");
    assert_eq!(entries[4]["path"], "THIRD_PARTY_NOTICES");
}

#[test]
fn missing_or_wrong_layout_content_blocks_the_lock() {
    let tmp = tempfile::tempdir().unwrap();
    let package_dir = tmp.path().join("package");
    package(&package_dir);
    let asset = tmp
        .path()
        .join("pagis-server-1.2.3-aarch64-apple-darwin.dmg");
    fs::write(&asset, b"dmg").unwrap();
    let lock_path = tmp.path().join("runtime-lock.json");

    fs::remove_file(package_dir.join("gog")).unwrap();
    let error = write_runtime_lock(&request(&package_dir, &asset), &lock_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("gog"), "{error}");

    package(&package_dir);
    fs::write(package_dir.join("unexpected"), b"extra").unwrap();
    let error = write_runtime_lock(&request(&package_dir, &asset), &lock_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unexpected"), "{error}");
}

#[test]
fn lock_consumption_rejects_changed_asset_or_mounted_content() {
    let tmp = tempfile::tempdir().unwrap();
    let mounted_dmg = tmp.path().join("mounted-dmg");
    package(&mounted_dmg);
    let asset = tmp
        .path()
        .join("pagis-server-1.2.3-aarch64-apple-darwin.dmg");
    fs::write(&asset, b"final notarized and stapled dmg").unwrap();
    let lock_path = tmp.path().join("runtime-lock.json");
    write_runtime_lock(&request(&mounted_dmg, &asset), &lock_path).unwrap();
    validate_runtime_lock(&lock_path, &mounted_dmg, &asset).unwrap();

    fs::write(&asset, b"changed after the lock was written").unwrap();
    let error = validate_runtime_lock(&lock_path, &mounted_dmg, &asset)
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not match"), "{error}");

    fs::write(&asset, b"final notarized and stapled dmg").unwrap();
    fs::write(mounted_dmg.join("gog"), b"changed provider").unwrap();
    let error = validate_runtime_lock(&lock_path, &mounted_dmg, &asset)
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not match"), "{error}");
}

#[test]
fn a_mutable_or_wrong_repository_computer_image_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let package_dir = tmp.path().join("package");
    package(&package_dir);
    let asset = tmp
        .path()
        .join("pagis-server-1.2.3-aarch64-apple-darwin.dmg");
    fs::write(&asset, b"dmg").unwrap();
    let lock_path = tmp.path().join("runtime-lock.json");

    for image in [
        "ghcr.io/pagis-co/pagis-computer:1.2.3",
        "ghcr.io/someone/pagis-computer@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "ghcr.io/pagis-co/pagis-computer@sha256:AAAA",
    ] {
        let mut request = request(&package_dir, &asset);
        request.computer_image = image;
        let error = write_runtime_lock(&request, &lock_path)
            .unwrap_err()
            .to_string();
        assert!(error.contains("immutable Computer image"), "{error}");
    }
}

#[test]
fn routine_packaging_fixture_is_a_valid_lock_for_only_its_release() {
    let tmp = tempfile::tempdir().unwrap();
    for platform in ClientPlatform::ALL {
        let lock = tmp.path().join(platform.lock_file());
        write_fixture_runtime_lock("1.2.3", platform, &lock).unwrap();
        validate_runtime_lock_metadata(&lock, "1.2.3", platform).unwrap();
        let error = validate_runtime_lock_metadata(&lock, "1.2.4", platform)
            .unwrap_err()
            .to_string();
        assert!(error.contains("1.2.4"), "error: {error}");
    }
}

#[test]
fn a_lock_is_valid_only_for_the_platform_it_names() {
    let tmp = tempfile::tempdir().unwrap();
    let lock = tmp.path().join("runtime-lock-linux-x64.json");
    write_fixture_runtime_lock("1.2.3", ClientPlatform::LinuxX64, &lock).unwrap();

    for other in [ClientPlatform::LinuxArm64, ClientPlatform::MacArm64] {
        let error = validate_runtime_lock_metadata(&lock, "1.2.3", other)
            .unwrap_err()
            .to_string();
        assert!(error.contains(&other.name()), "{error}");
    }
}

#[test]
fn each_platform_has_its_own_lock_file_and_server_package() {
    assert_eq!(
        ClientPlatform::MacArm64.lock_file(),
        "runtime-lock-darwin-arm64.json"
    );
    assert_eq!(
        ClientPlatform::LinuxX64.lock_file(),
        "runtime-lock-linux-x64.json"
    );
    assert_eq!(
        ClientPlatform::LinuxX64.server_package("1.2.3"),
        "pagis-server-1.2.3-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(
        ClientPlatform::LinuxArm64.server_package("1.2.3"),
        "pagis-server-1.2.3-aarch64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(
        ClientPlatform::parse("linux-arm64").unwrap(),
        ClientPlatform::LinuxArm64
    );
    assert!(ClientPlatform::parse("linux-amd64").is_err());
}

/// The Linux lock names the server archive by size and hash, and
/// each file by size, hash and mode. It carries no macOS signing field.
#[test]
fn the_linux_archive_produces_a_lock_with_no_signing_fields() {
    let tmp = tempfile::tempdir().unwrap();
    let package_dir = tmp.path().join("extracted");
    package(&package_dir);
    let asset = tmp
        .path()
        .join("pagis-server-1.2.3-x86_64-unknown-linux-gnu.tar.gz");
    let archive_bytes = b"final server archive";
    fs::write(&asset, archive_bytes).unwrap();
    let lock_path = tmp.path().join("runtime-lock-linux-x64.json");

    write_runtime_lock(
        &linux_request(ClientPlatform::LinuxX64, &package_dir, &asset),
        &lock_path,
    )
    .unwrap();

    let lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    assert_eq!(lock["platform"], "linux");
    assert_eq!(lock["arch"], "x64");
    assert_eq!(lock["asset"]["format"], "tar.gz");
    assert_eq!(
        lock["asset"]["url"],
        "https://github.com/pagis-co/pagis/releases/download/v1.2.3/pagis-server-1.2.3-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(lock["asset"]["size"], archive_bytes.len());
    assert!(lock["asset"].get("team_id").is_none(), "{lock}");
    let entries = lock["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 5);
    assert!(
        entries
            .iter()
            .all(|entry| entry.get("codesign_id").is_none())
    );
    assert_eq!(entries[0]["path"], "pagis");
    assert_eq!(entries[0]["mode"], 493);
    validate_runtime_lock_metadata(&lock_path, "1.2.3", ClientPlatform::LinuxX64).unwrap();
    validate_runtime_lock(&lock_path, &package_dir, &asset).unwrap();

    fs::write(package_dir.join("gog"), b"changed provider").unwrap();
    let error = validate_runtime_lock(&lock_path, &package_dir, &asset)
        .unwrap_err()
        .to_string();
    assert!(error.contains("does not match"), "{error}");
}

#[test]
fn a_linux_lock_refuses_the_wrong_archive_or_a_signing_team() {
    let tmp = tempfile::tempdir().unwrap();
    let package_dir = tmp.path().join("extracted");
    package(&package_dir);
    let lock_path = tmp.path().join("runtime-lock-linux-arm64.json");
    let amd64 = tmp
        .path()
        .join("pagis-server-1.2.3-x86_64-unknown-linux-gnu.tar.gz");
    fs::write(&amd64, b"archive").unwrap();

    let error = write_runtime_lock(
        &linux_request(ClientPlatform::LinuxArm64, &package_dir, &amd64),
        &lock_path,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("aarch64-unknown-linux-gnu"), "{error}");

    let mut signed = linux_request(ClientPlatform::LinuxX64, &package_dir, &amd64);
    signed.team_id = Some("AB12CD34EF");
    let error = write_runtime_lock(&signed, &lock_path)
        .unwrap_err()
        .to_string();
    assert!(error.contains("no signing team"), "{error}");
}

#[test]
fn a_macos_lock_needs_its_signing_team() {
    let tmp = tempfile::tempdir().unwrap();
    let package_dir = tmp.path().join("package");
    package(&package_dir);
    let asset = tmp
        .path()
        .join("pagis-server-1.2.3-aarch64-apple-darwin.dmg");
    fs::write(&asset, b"dmg").unwrap();
    let mut unsigned = request(&package_dir, &asset);
    unsigned.team_id = None;

    let error = write_runtime_lock(&unsigned, &tmp.path().join("lock.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("team identifier"), "{error}");
}

/// A field of the other platform's shape is refused where the lock is
/// read, so the release never publishes a Linux lock with a team or a
/// macOS lock without one.
#[test]
fn a_lock_with_the_other_platforms_fields_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let lock_path = tmp.path().join("runtime-lock-linux-x64.json");
    write_fixture_runtime_lock("1.2.3", ClientPlatform::LinuxX64, &lock_path).unwrap();
    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    lock["asset"]["team_id"] = Value::from("AB12CD34EF");
    fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();

    assert!(validate_runtime_lock_metadata(&lock_path, "1.2.3", ClientPlatform::LinuxX64).is_err());

    let lock_path = tmp.path().join("runtime-lock-darwin-arm64.json");
    write_fixture_runtime_lock("1.2.3", ClientPlatform::MacArm64, &lock_path).unwrap();
    let mut lock: Value = serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    lock["asset"].as_object_mut().unwrap().remove("team_id");
    fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();

    assert!(validate_runtime_lock_metadata(&lock_path, "1.2.3", ClientPlatform::MacArm64).is_err());
}
