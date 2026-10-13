//! The package source seam and the inputs of a Plugin install.

use crate::PackageFiles;
use async_trait::async_trait;

/// Why the files could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    #[error("{0}")]
    Refused(String),
    #[error("{0}")]
    Failed(String),
}

/// What the user asked to install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInput {
    Git {
        url: String,
        reference: Option<String>,
    },
    /// The plugin directory as one uncompressed tar.
    Upload { tar: Vec<u8> },
}

/// An Embedder supplies the files of a source address.
#[async_trait]
pub trait PackageSource: Send + Sync {
    async fn fetch(&self, url: &str, reference: Option<&str>) -> Result<PackageFiles, SourceError>;
}

#[cfg(feature = "native")]
pub use native::GitPackageSource;

#[cfg(feature = "native")]
mod native {
    use super::*;
    use std::path::Path;

    /// The schemes a plugin may be cloned from. `ext::` and every other
    /// scheme is refused: `ext::` makes git run a command the URL names.
    const CLONE_SCHEMES: [&str; 3] = ["https", "ssh", "file"];

    /// The daemon fetches with git, using its URL and reference checks.
    pub struct GitPackageSource;

    #[async_trait]
    impl PackageSource for GitPackageSource {
        async fn fetch(
            &self,
            url: &str,
            reference: Option<&str>,
        ) -> Result<PackageFiles, SourceError> {
            let scratch =
                tempfile::tempdir().map_err(|error| SourceError::Failed(error.to_string()))?;
            let tree = scratch.path().join("plugin");
            clone(url, reference, &tree).await?;
            PackageFiles::from_directory(&tree)
                .map_err(|error| SourceError::Failed(error.to_string()))
        }
    }

    /// Clone one repository and leave the files without their history: the
    /// daemon keeps its own repository of installed states, and the
    /// remote's history is not part of a state.
    async fn clone(url: &str, reference: Option<&str>, into: &Path) -> Result<(), SourceError> {
        check_url(url)?;
        if let Some(reference) = reference {
            check_reference(reference)?;
        }
        run(tokio::process::Command::new("git")
            .arg("clone")
            .arg("--quiet")
            .arg("--")
            .arg(url)
            .arg(into))
        .await?;
        if let Some(reference) = reference {
            run(tokio::process::Command::new("git")
                .arg("-C")
                .arg(into)
                .arg("checkout")
                .arg("--quiet")
                .arg("--detach")
                .arg(reference))
            .await?;
        }
        std::fs::remove_dir_all(into.join(".git"))
            .map_err(|error| SourceError::Failed(format!("cannot drop the clone history: {error}")))
    }

    async fn run(command: &mut tokio::process::Command) -> Result<(), SourceError> {
        let output = command.output().await.map_err(|error| {
            SourceError::Failed(format!("cannot run git: {error}; is git installed?"))
        })?;
        if output.status.success() {
            return Ok(());
        }
        Err(SourceError::Failed(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ))
    }

    fn check_url(url: &str) -> Result<(), SourceError> {
        let parsed = url::Url::parse(url)
            .map_err(|error| SourceError::Refused(format!("{url:?} is not a URL: {error}")))?;
        if !CLONE_SCHEMES.contains(&parsed.scheme()) {
            return Err(SourceError::Refused(format!(
                "{url:?} is not an https, ssh or file address"
            )));
        }
        Ok(())
    }

    /// A ref that starts with `-` would reach git as an option.
    fn check_reference(reference: &str) -> Result<(), SourceError> {
        if reference.starts_with('-') || reference.trim().is_empty() {
            return Err(SourceError::Refused(format!(
                "{reference:?} is not a branch, tag or commit"
            )));
        }
        Ok(())
    }
}
