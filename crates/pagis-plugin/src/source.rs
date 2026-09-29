//! Where the files of a Plugin come from (ADR-0017): a git URL
//! with an optional ref, or an upload of the directory as one tar.
//!
//! The clone runs the user's own `git`. The workspace builds `git2`
//! without a TLS transport, and the user's `git` already carries the
//! credentials and the SSH keys a private repository needs, which is
//! how other plugin hosts reach one.

use std::path::Path;

use pagis_core::PluginSource;

/// The schemes a plugin may be cloned from. `ext::` and every other
/// scheme is refused: `ext::` makes git run a command the URL names.
const CLONE_SCHEMES: [&str; 3] = ["https", "ssh", "file"];

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

/// Read the files into the empty directory `into`, and answer with the
/// source the record keeps.
pub async fn fetch(input: &SourceInput, into: &Path) -> Result<PluginSource, SourceError> {
    match input {
        SourceInput::Git { url, reference } => {
            clone(url, reference.as_deref(), into).await?;
            Ok(PluginSource::Git {
                url: url.clone(),
                reference: reference.clone(),
            })
        }
        SourceInput::Upload { tar } => {
            unpack(tar, into)?;
            Ok(PluginSource::Upload)
        }
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

/// Unpack the upload. An archive that carries one top directory, as a
/// zip of a folder does, is unpacked without it, so the root of the
/// tree is the plugin root.
fn unpack(tar: &[u8], into: &Path) -> Result<(), SourceError> {
    let mut archive = tar::Archive::new(tar);
    let mut entries = Vec::new();
    for entry in archive
        .entries()
        .map_err(|error| SourceError::Refused(format!("the upload is not a tar: {error}")))?
    {
        use std::io::Read;
        let mut entry = entry
            .map_err(|error| SourceError::Refused(format!("cannot read the upload: {error}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry
            .path()
            .map_err(|error| SourceError::Refused(format!("an entry has no path: {error}")))?
            .into_owned();
        if !crate::validate::stays_inside(&path) {
            return Err(SourceError::Refused(format!(
                "the upload carries {}, which leaves the plugin root",
                path.display()
            )));
        }
        let executable = entry.header().mode().unwrap_or(0o644) & 0o111 != 0;
        let mut data = Vec::new();
        entry
            .read_to_end(&mut data)
            .map_err(|error| SourceError::Refused(format!("cannot read an entry: {error}")))?;
        entries.push((path, data, executable));
    }
    if entries.is_empty() {
        return Err(SourceError::Refused("the upload holds no file".to_string()));
    }

    let strip = shared_root(&entries);
    for (path, data, executable) in &entries {
        let path = match strip {
            Some(root) => match path.strip_prefix(root) {
                Ok(rest) => rest.to_path_buf(),
                Err(_) => path.clone(),
            },
            None => path.clone(),
        };
        if path.as_os_str().is_empty() {
            continue;
        }
        let target = into.join(&path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                SourceError::Failed(format!("cannot write the upload: {error}"))
            })?;
        }
        std::fs::write(&target, data)
            .map_err(|error| SourceError::Failed(format!("cannot write the upload: {error}")))?;
        #[cfg(unix)]
        if *executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).map_err(
                |error| SourceError::Failed(format!("cannot set the file mode: {error}")),
            )?;
        }
    }
    Ok(())
}

/// The one top directory every entry is under, when there is one.
fn shared_root(entries: &[(std::path::PathBuf, Vec<u8>, bool)]) -> Option<&std::ffi::OsStr> {
    let first = entries
        .first()?
        .0
        .components()
        .next()
        .map(|part| part.as_os_str())?;
    let shared = entries.iter().all(|(path, _, _)| {
        path.components().count() > 1
            && path.components().next().map(|part| part.as_os_str()) == Some(first)
    });
    shared.then_some(first)
}
