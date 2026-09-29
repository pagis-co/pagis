//! Side-effect-free checks for a staged Server Runtime package.

use std::path::Path;

use anyhow::{Context, Result, bail};

const IMAGE_PREFIX: &str = "ghcr.io/pagis-co/pagis-computer@sha256:";

/// Check the files and pins needed before a staged server may run.
/// This function does not open a Workspace, keychain or network listener.
pub fn validate_server_package(
    executable: &Path,
    product_app_is_built: bool,
    computer_image: &str,
) -> Result<()> {
    if !product_app_is_built {
        bail!("the server binary has no embedded Product App");
    }
    let package_dir = executable
        .parent()
        .context("the server executable has no package directory")?;
    let gog = package_dir.join("gog");
    let metadata = std::fs::metadata(&gog).with_context(|| {
        format!(
            "the pinned gog executable is missing beside pagis at {}",
            gog.display()
        )
    })?;
    if !metadata.is_file() {
        bail!("the pinned gog entry beside pagis is not a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            bail!("the pinned gog entry beside pagis is not executable");
        }
    }
    let Some(digest) = computer_image.strip_prefix(IMAGE_PREFIX) else {
        bail!("the server does not contain an immutable Computer image reference");
    };
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        bail!("the server Computer image digest is invalid");
    }
    Ok(())
}
