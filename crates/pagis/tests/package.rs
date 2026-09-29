//! The side-effect-free server package check used before installation.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const IMAGE: &str = "ghcr.io/pagis-co/pagis-computer@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn a_complete_server_package_passes_without_using_path() {
    let tmp = tempfile::tempdir().unwrap();
    let pagis = tmp.path().join("pagis");
    let gog = tmp.path().join("gog");
    fs::write(&pagis, b"server").unwrap();
    fs::write(&gog, b"provider").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&gog, fs::Permissions::from_mode(0o755)).unwrap();

    pagis::validate_server_package(&pagis, true, IMAGE).unwrap();
}

#[test]
fn package_check_names_missing_ui_gog_and_mutable_image() {
    let tmp = tempfile::tempdir().unwrap();
    let pagis = tmp.path().join("pagis");
    fs::write(&pagis, b"server").unwrap();

    let error = pagis::validate_server_package(&pagis, false, IMAGE)
        .unwrap_err()
        .to_string();
    assert!(error.contains("Product App"), "{error}");

    let error = pagis::validate_server_package(&pagis, true, IMAGE)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("gog executable is missing beside pagis"),
        "{error}"
    );

    let gog = tmp.path().join("gog");
    fs::write(&gog, b"provider").unwrap();
    #[cfg(unix)]
    fs::set_permissions(&gog, fs::Permissions::from_mode(0o755)).unwrap();
    let error =
        pagis::validate_server_package(&pagis, true, "ghcr.io/pagis-co/pagis-computer:latest")
            .unwrap_err()
            .to_string();
    assert!(error.contains("immutable Computer image"), "{error}");
}
