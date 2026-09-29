//! The tar of one published version. A publish reads the
//! working copy out of the agent's Computer as one tar and packs the
//! files that belong to the package. Dependencies are not published:
//! the exclusions drop them, and `setup` builds them again in every
//! Computer that materializes the version.
//!
//! A package holds only regular files and directories, as a Plugin
//! upload does. The pack refuses a symbolic link, a hard link or a
//! special file that the version tar would hold. It judges the tar
//! entry headers, before any entry reaches a file system. A link under
//! a path that the exclusions drop, such as `.venv`, stops nothing.
//!
//! The working copy comes from an Agent, so the pack stops at a limit
//! before it holds more than that limit. It reads the tar in two
//! passes. The first pass reads only the `.gitignore` files and the
//! origin marker. The second pass skips the data of each entry that the
//! exclusions drop, and it stops when the kept bytes or the kept
//! entries pass their limits. Only the version tar holds kept data.

use std::ffi::OsStr;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::origin::{ORIGIN_FILE, Origin};

/// The largest package, after the exclusions a publish applies. The
/// count is the bytes that the tar reader returns for each kept file,
/// not the size that the entry header states.
pub const MAX_PACKAGE_BYTES: u64 = 50 * 1024 * 1024;
/// The most entries a package holds after the exclusions: its files,
/// its directories and the entries it refuses. A package is small
/// source code, and the limit stops a tar of many empty files.
pub const MAX_PACKAGE_ENTRIES: usize = 10_000;
/// The most `.gitignore` text that a publish reads. The first pass
/// keeps the ignore rules in memory, so the limit applies to all the
/// `.gitignore` files together, and so to each one.
pub const MAX_GITIGNORE_BYTES: u64 = 1024 * 1024;

/// The entries a publish always drops, whatever `.gitignore` says.
/// The origin marker joins them: a publish reads it and records the
/// origin, so no published Version carries it.
const ALWAYS_EXCLUDED: [&str; 4] = [".git", "node_modules", ".venv", ORIGIN_FILE];

/// What a publish takes out of one working copy.
pub struct Packed {
    /// The version tar. Its entries are relative to the package root.
    pub tar: Vec<u8>,
    /// The origin that the working copy declares, if it declares one.
    pub origin: Option<Origin>,
}

/// Pack the Docker archive of one working copy. The archive endpoint
/// tars the parent of the requested path, so an export of
/// `~/software/weather` carries its entries under `weather/`; the
/// version tar drops that first path element.
///
/// The version tar honours the working copy's `.gitignore` files and
/// drops `.git`, `node_modules` and `.venv`. An entry that is not a
/// regular file or a directory, and that the version tar would hold,
/// refuses the pack. The refusal names each one.
pub fn pack<R: Read + Seek>(mut container_tar: R) -> Result<Packed, String> {
    let (ignore, origin) = read_rules(&mut container_tar)?;
    container_tar
        .rewind()
        .map_err(|error| format!("cannot read the working copy tar: {error}"))?;
    let tar = pack_kept(&mut container_tar, &ignore)?;
    Ok(Packed { tar, origin })
}

/// The first pass: the ignore rules and the origin marker. It reads
/// no other data, and it skips the `.gitignore` files of the trees
/// that a publish always drops.
fn read_rules<R: Read + Seek>(container_tar: R) -> Result<(Gitignore, Option<Origin>), String> {
    let mut archive = tar::Archive::new(container_tar);
    let mut rules = GitignoreBuilder::new("");
    let mut rule_bytes: u64 = 0;
    let mut origin = None;
    for entry in archive
        .entries_with_seek()
        .map_err(|error| format!("cannot read the working copy tar: {error}"))?
    {
        let mut entry = entry.map_err(|error| format!("cannot read a tar entry: {error}"))?;
        let Some(path) = package_path(&entry)? else {
            continue;
        };
        if kind_of(entry.header()) != Kind::File {
            continue;
        }
        if path == Path::new(ORIGIN_FILE) {
            origin = Some(Origin::read(&mut entry)?);
        } else if path.file_name() == Some(OsStr::new(".gitignore")) && !excluded(&path) {
            let mut bytes = Vec::new();
            (&mut entry)
                .take(MAX_GITIGNORE_BYTES - rule_bytes + 1)
                .read_to_end(&mut bytes)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            rule_bytes += bytes.len() as u64;
            if rule_bytes > MAX_GITIGNORE_BYTES {
                return Err(format!(
                    "the .gitignore files are larger than the limit of {MAX_GITIGNORE_BYTES} \
                     bytes; {path:?} passes it"
                ));
            }
            // The rules of every `.gitignore` go into one matcher at the
            // package root.
            for line in String::from_utf8_lossy(&bytes).lines() {
                rules
                    .add_line(Some(path.clone()), line)
                    .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            }
        }
    }
    let ignore = rules
        .build()
        .map_err(|error| format!("cannot read the ignore rules: {error}"))?;
    Ok((ignore, origin))
}

/// The second pass: the version tar of every entry that the
/// exclusions keep. The data of a dropped entry is skipped with a seek
/// and never read. The data of a kept file goes straight into the
/// version tar, and the header there states the bytes that the reader
/// returned.
fn pack_kept<R: Read + Seek>(container_tar: R, ignore: &Gitignore) -> Result<Vec<u8>, String> {
    let mut archive = tar::Archive::new(container_tar);
    let mut builder = tar::Builder::new(Cursor::new(Vec::new()));
    let mut kept_entries: usize = 0;
    let mut kept_bytes: u64 = 0;
    let mut refused = Vec::new();
    for entry in archive
        .entries_with_seek()
        .map_err(|error| format!("cannot read the working copy tar: {error}"))?
    {
        let mut entry = entry.map_err(|error| format!("cannot read a tar entry: {error}"))?;
        let Some(path) = package_path(&entry)? else {
            continue;
        };
        let kind = kind_of(entry.header());
        if excluded(&path)
            || ignore
                .matched_path_or_any_parents(&path, kind == Kind::Directory)
                .is_ignore()
        {
            continue;
        }
        kept_entries += 1;
        if kept_entries > MAX_PACKAGE_ENTRIES {
            return Err(format!(
                "the package has more than the limit of {MAX_PACKAGE_ENTRIES} entries after \
                 exclusions; {path:?} passes it"
            ));
        }
        let mut header = entry.header().clone();
        match kind {
            Kind::Refused(what) => refused.push(format!(
                "{path:?} is {what}; a Software Package holds only regular files and directories"
            )),
            Kind::Directory => {
                header.set_size(0);
                builder
                    .append_data(&mut header, &path, std::io::empty())
                    .map_err(|error| format!("cannot write the version tar: {error}"))?;
            }
            Kind::File => {
                let mut writer = builder
                    .append_writer(&mut header, &path)
                    .map_err(|error| format!("cannot write the version tar: {error}"))?;
                // One byte over the budget shows that the file passes
                // the limit, and the read stops there.
                kept_bytes += std::io::copy(
                    &mut (&mut entry).take(MAX_PACKAGE_BYTES - kept_bytes + 1),
                    &mut writer,
                )
                .map_err(|error| format!("cannot read the entry {}: {error}", path.display()))?;
                writer
                    .finish()
                    .map_err(|error| format!("cannot write the version tar: {error}"))?;
                if kept_bytes > MAX_PACKAGE_BYTES {
                    return Err(format!(
                        "the package is larger than the limit of {MAX_PACKAGE_BYTES} bytes after \
                         exclusions; {path:?} passes it"
                    ));
                }
            }
        }
    }
    if !refused.is_empty() {
        return Err(refused.join("\n"));
    }
    builder
        .into_inner()
        .map(Cursor::into_inner)
        .map_err(|error| format!("cannot close the version tar: {error}"))
}

/// The path of one entry relative to the package root: the Docker
/// archive path without its first element. The exported directory
/// itself has no package path.
fn package_path<R: Read>(entry: &tar::Entry<'_, R>) -> Result<Option<PathBuf>, String> {
    let path: PathBuf = entry
        .path()
        .map_err(|error| format!("a tar entry has no usable path: {error}"))?
        .components()
        .skip(1)
        .collect();
    Ok((!path.as_os_str().is_empty()).then_some(path))
}

/// What one tar entry is, for a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Directory,
    /// A symbolic link, a hard link or a special file, with the words
    /// that name it in a refusal.
    Refused(&'static str),
}

/// The kind of one entry, from its header alone. A package keeps only
/// regular files and directories, which is the rule of the Plugin
/// upload. A GNU sparse entry is a special file: its data expands to
/// its full size when it is read.
fn kind_of(header: &tar::Header) -> Kind {
    match header.entry_type() {
        tar::EntryType::Regular => Kind::File,
        tar::EntryType::Directory => Kind::Directory,
        tar::EntryType::Symlink => Kind::Refused("a symbolic link"),
        tar::EntryType::Link => Kind::Refused("a hard link"),
        _ => Kind::Refused("a special file"),
    }
}

/// Whether a publish always drops `path`, by the name of one of its
/// components.
fn excluded(path: &Path) -> bool {
    path.components().any(|part| {
        part.as_os_str()
            .to_str()
            .is_some_and(|name| ALWAYS_EXCLUDED.contains(&name))
    })
}
