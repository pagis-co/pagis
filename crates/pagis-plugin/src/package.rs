//! The files of a Plugin Package, held in memory.

use std::collections::BTreeMap;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};

/// One file and the executable flag that git stores with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    pub bytes: Vec<u8>,
    pub executable: bool,
}

/// Relative paths and file contents. A directory exists when a file is under it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageFiles {
    files: BTreeMap<PathBuf, PackageFile>,
}

/// Why a path or archive cannot be read as a Plugin Package.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct PackageError(pub String);

impl PackageFiles {
    /// Add a file. Its path must be relative and must stay inside the package.
    pub fn insert(
        &mut self,
        path: impl AsRef<Path>,
        bytes: Vec<u8>,
        executable: bool,
    ) -> Result<(), PackageError> {
        let path = path.as_ref();
        if !stays_inside(path) || normalize(path).as_os_str().is_empty() {
            return Err(PackageError(format!(
                "{} is not a relative file path inside the plugin root",
                path.display()
            )));
        }
        self.files
            .insert(normalize(path), PackageFile { bytes, executable });
        Ok(())
    }

    pub fn get(&self, path: impl AsRef<Path>) -> Option<&PackageFile> {
        self.files.get(&normalize(path.as_ref()))
    }

    pub fn is_file(&self, path: impl AsRef<Path>) -> bool {
        self.get(path).is_some()
    }

    pub fn is_dir(&self, path: impl AsRef<Path>) -> bool {
        let path = normalize(path.as_ref());
        self.files
            .keys()
            .any(|file| file != &path && file.starts_with(&path))
    }

    /// Files in path order.
    pub fn iter(&self) -> impl Iterator<Item = (&PathBuf, &PackageFile)> {
        self.files.iter()
    }

    pub(crate) fn read_text(&self, path: &str) -> io::Result<&str> {
        let file = self
            .get(path)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        std::str::from_utf8(&file.bytes).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })
    }

    /// Read an uncompressed tar without writing files. Remove one shared root
    /// directory, and keep the executable flag of each regular file.
    pub fn from_tar(bytes: &[u8]) -> Result<Self, PackageError> {
        let mut archive = tar::Archive::new(bytes);
        let mut entries = Vec::new();
        for entry in archive
            .entries()
            .map_err(|error| PackageError(format!("the upload is not a tar: {error}")))?
        {
            let mut entry =
                entry.map_err(|error| PackageError(format!("cannot read the upload: {error}")))?;
            if !entry.header().entry_type().is_file() {
                continue;
            }
            let path = entry
                .path()
                .map_err(|error| PackageError(format!("an entry has no path: {error}")))?
                .into_owned();
            if !stays_inside(&path) {
                return Err(PackageError(format!(
                    "the upload carries {}, which leaves the plugin root",
                    path.display()
                )));
            }
            let executable = entry.header().mode().unwrap_or(0o644) & 0o111 != 0;
            let mut data = Vec::new();
            entry
                .read_to_end(&mut data)
                .map_err(|error| PackageError(format!("cannot read an entry: {error}")))?;
            entries.push((normalize(&path), data, executable));
        }
        if entries.is_empty() {
            return Err(PackageError("the upload holds no file".to_string()));
        }
        let strip = shared_root(&entries).map(ToOwned::to_owned);
        let mut files = Self::default();
        for (path, data, executable) in entries {
            let path = match &strip {
                Some(root) => path
                    .strip_prefix(root)
                    .expect("the shared root")
                    .to_path_buf(),
                None => path,
            };
            if path.as_os_str().is_empty() {
                continue;
            }
            files.insert(path, data, executable)?;
        }
        Ok(files)
    }

    /// Read the regular files of a native checkout. Empty directories and
    /// symbolic links are not files in a Plugin Package.
    #[cfg(feature = "native")]
    pub fn from_directory(root: &Path) -> io::Result<Self> {
        let mut files = Self::default();
        let mut directories = vec![root.to_path_buf()];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    directories.push(entry.path());
                } else if kind.is_file() {
                    #[cfg(unix)]
                    let executable = {
                        use std::os::unix::fs::PermissionsExt;
                        entry.metadata()?.permissions().mode() & 0o111 != 0
                    };
                    #[cfg(not(unix))]
                    let executable = false;
                    let path = entry.path();
                    files.files.insert(
                        path.strip_prefix(root)
                            .expect("a checkout path")
                            .to_path_buf(),
                        PackageFile {
                            bytes: std::fs::read(path)?,
                            executable,
                        },
                    );
                }
            }
        }
        Ok(files)
    }
}

/// A path is relative and has no parent directory component.
pub(crate) fn stays_inside(path: &Path) -> bool {
    path.components()
        .all(|part| matches!(part, Component::Normal(_) | Component::CurDir))
}

fn normalize(path: &Path) -> PathBuf {
    path.components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect()
}

fn shared_root(entries: &[(PathBuf, Vec<u8>, bool)]) -> Option<&std::ffi::OsStr> {
    let first = entries.first()?.0.components().next()?.as_os_str();
    entries
        .iter()
        .all(|(path, _, _)| {
            path.components().count() > 1
                && path.components().next().map(|part| part.as_os_str()) == Some(first)
        })
        .then_some(first)
}
