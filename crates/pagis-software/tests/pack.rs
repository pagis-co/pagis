//! What a publish carries and what it leaves behind.

use std::io::{Cursor, Read, Seek, SeekFrom, Write};

use pagis_software::{
    MAX_GITIGNORE_BYTES, MAX_ORIGIN_BYTES, MAX_PACKAGE_BYTES, MAX_PACKAGE_ENTRIES, ORIGIN_FILE,
    Packed, pack,
};

/// The Docker archive of a working copy called `weather`: the named
/// files, each with mode 0644, under the directory's own name.
fn tar_of(files: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("weather/{path}"), content.as_bytes())
            .expect("append");
    }
    builder.into_inner().expect("tar")
}

/// Pack a container tar that is held in memory.
fn pack_of(container_tar: &[u8]) -> Result<Packed, String> {
    pack(Cursor::new(container_tar))
}

/// The refusal of a pack. A pack that succeeds fails the test with the
/// size of its version tar, not with the whole tar.
fn refusal(outcome: Result<Packed, String>) -> String {
    match outcome {
        Ok(packed) => panic!(
            "the pack is not refused; the version tar holds {} bytes",
            packed.tar.len()
        ),
        Err(refused) => refused,
    }
}

fn paths_in(tar: &[u8]) -> Vec<String> {
    let mut archive = tar::Archive::new(tar);
    archive
        .entries()
        .expect("entries")
        .map(|entry| {
            entry
                .expect("entry")
                .path()
                .expect("path")
                .display()
                .to_string()
        })
        .collect()
}

#[test]
fn the_version_tar_keeps_the_package_files() {
    let packed = pack_of(&tar_of(&[
        ("pagis-software.toml", "[package]"),
        ("bin/forecast.py", "print(1)"),
    ]))
    .expect("pack");

    assert_eq!(
        paths_in(&packed.tar),
        vec!["pagis-software.toml", "bin/forecast.py"]
    );
    assert_eq!(packed.origin, None);
}

#[test]
fn the_version_tar_drops_git_node_modules_and_venv() {
    let packed = pack_of(&tar_of(&[
        ("pagis-software.toml", "[package]"),
        (".git/config", "[core]"),
        ("node_modules/left-pad/index.js", "module.exports = 1"),
        (".venv/pyvenv.cfg", "home = /usr"),
    ]))
    .expect("pack");

    assert_eq!(paths_in(&packed.tar), vec!["pagis-software.toml"]);
}

#[test]
fn the_version_tar_honours_the_working_copy_gitignore() {
    let packed = pack_of(&tar_of(&[
        (".gitignore", "*.log\nbuild/\n"),
        ("pagis-software.toml", "[package]"),
        ("run.log", "noise"),
        ("build/out.bin", "bytes"),
        ("bin/forecast.py", "print(1)"),
    ]))
    .expect("pack");

    assert_eq!(
        paths_in(&packed.tar),
        vec![".gitignore", "pagis-software.toml", "bin/forecast.py"]
    );
}

/// The archive endpoint writes the files of a directory in name order,
/// so a file can come before the `.gitignore` that names it.
#[test]
fn a_gitignore_rule_drops_a_file_that_comes_before_it() {
    let packed = pack_of(&tar_of(&[
        ("-scratch.log", "noise"),
        (".gitignore", "*.log\n"),
        ("pagis-software.toml", "[package]"),
    ]))
    .expect("pack");

    assert_eq!(
        paths_in(&packed.tar),
        vec![".gitignore", "pagis-software.toml"]
    );
}

#[test]
fn the_version_tar_keeps_the_executable_bit_and_the_content() {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(9);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, "weather/bin/forecast", &b"print(1)\n"[..])
        .expect("append");
    let working_copy = builder.into_inner().expect("tar");

    let packed = pack_of(&working_copy).expect("pack");

    let mut archive = tar::Archive::new(packed.tar.as_slice());
    let mut entry = archive
        .entries()
        .expect("entries")
        .next()
        .expect("one entry")
        .expect("entry");
    assert_eq!(entry.header().mode().expect("mode") & 0o777, 0o755);
    assert_eq!(entry.header().size().expect("size"), 9);
    let mut content = String::new();
    entry.read_to_string(&mut content).expect("read");
    assert_eq!(content, "print(1)\n");
}

#[test]
fn the_docker_archive_loses_its_first_path_element() {
    let container = tar_of(&[
        ("pagis-software.toml", "name = \"weather\""),
        ("bin/forecast.py", "print(1)"),
    ]);

    let packed = pack_of(&container).expect("pack");

    assert_eq!(
        paths_in(&packed.tar),
        vec![
            "pagis-software.toml".to_string(),
            "bin/forecast.py".to_string()
        ]
    );
}

#[test]
fn the_exported_directory_itself_leaves_no_entry() {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(0);
    header.set_mode(0o755);
    header.set_entry_type(tar::EntryType::Directory);
    header.set_cksum();
    builder
        .append_data(&mut header, "weather/", &[][..])
        .expect("append");
    let container = builder.into_inner().expect("tar");

    let packed = pack_of(&container).expect("pack");

    assert_eq!(paths_in(&packed.tar), Vec::<String>::new());
}

/// The fork writes the marker, the pack reads it, and no version tar
/// holds it.
#[test]
fn the_origin_marker_is_read_and_left_out_of_the_version_tar() {
    let packed = pack_of(&tar_of(&[
        ("pagis-software.toml", "[package]"),
        (ORIGIN_FILE, "package = \"weather\"\nversion = \"v2\"\n"),
    ]))
    .expect("pack");

    assert_eq!(paths_in(&packed.tar), vec!["pagis-software.toml"]);
    let origin = packed.origin.expect("the origin");
    assert_eq!(origin.package, "weather");
    assert_eq!(origin.version, "v2");
}

/// The marker is TOML that parses: comment lines take it past the limit.
#[test]
fn an_origin_marker_over_its_limit_is_refused() {
    let padding = "#\n".repeat(MAX_ORIGIN_BYTES as usize / 2);
    let marker = format!("{padding}package = \"weather\"\nversion = \"v2\"\n");

    let refused = refusal(pack_of(&tar_of(&[
        ("pagis-software.toml", "[package]"),
        (ORIGIN_FILE, &marker),
    ])));

    assert_eq!(
        refused,
        format!("{ORIGIN_FILE} is larger than the limit of {MAX_ORIGIN_BYTES} bytes")
    );
}

/// One entry with no data: a path, a type and a link target. A
/// directory, a FIFO or a device has no link target.
fn append_other(
    builder: &mut tar::Builder<Vec<u8>>,
    path: &str,
    kind: tar::EntryType,
    target: &str,
) {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(kind);
    header.set_size(0);
    header.set_mode(0o644);
    let path = format!("weather/{path}");
    if target.is_empty() {
        builder
            .append_data(&mut header, path, std::io::empty())
            .expect("append");
    } else {
        builder
            .append_link(&mut header, path, target)
            .expect("append");
    }
}

#[test]
fn the_version_tar_refuses_every_entry_that_is_not_a_file_or_a_directory() {
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(10);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(
            &mut header,
            "weather/pagis-software.toml",
            &b"[package]\n"[..],
        )
        .expect("append");
    append_other(&mut builder, "bin", tar::EntryType::Directory, "");
    append_other(&mut builder, "bin/tool", tar::EntryType::Symlink, "/bin/sh");
    append_other(
        &mut builder,
        "copy.toml",
        tar::EntryType::Link,
        "pagis-software.toml",
    );
    append_other(&mut builder, "pipe", tar::EntryType::Fifo, "");
    append_other(&mut builder, "tty", tar::EntryType::Char, "");
    append_other(&mut builder, "disk", tar::EntryType::Block, "");
    let working_copy = builder.into_inner().expect("tar");

    let refused = refusal(pack_of(&working_copy));

    let rule = "a Software Package holds only regular files and directories";
    assert_eq!(
        refused,
        [
            format!("\"bin/tool\" is a symbolic link; {rule}"),
            format!("\"copy.toml\" is a hard link; {rule}"),
            format!("\"pipe\" is a special file; {rule}"),
            format!("\"tty\" is a special file; {rule}"),
            format!("\"disk\" is a special file; {rule}"),
        ]
        .join("\n")
    );
}

/// Dependency trees and build output hold links, and a publish drops
/// them. A link there does not stop the publish.
#[test]
fn links_under_excluded_paths_leave_the_version_tar_without_a_refusal() {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in [
        (".gitignore", "build/\n"),
        ("pagis-software.toml", "[package]"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("weather/{path}"), content.as_bytes())
            .expect("append");
    }
    append_other(
        &mut builder,
        ".venv/bin/python",
        tar::EntryType::Symlink,
        "/usr/bin/python3",
    );
    append_other(
        &mut builder,
        "node_modules/.bin/left-pad",
        tar::EntryType::Symlink,
        "../left-pad/cli.js",
    );
    append_other(
        &mut builder,
        "build/latest",
        tar::EntryType::Symlink,
        "/etc",
    );
    append_other(&mut builder, "build/pipe", tar::EntryType::Fifo, "");
    let working_copy = builder.into_inner().expect("tar");

    let packed = pack_of(&working_copy).expect("the excluded links refuse nothing");

    assert_eq!(
        paths_in(&packed.tar),
        vec![".gitignore", "pagis-software.toml"]
    );
}

/// A reader that counts the bytes a pack reads through it. A seek
/// reads nothing, so a skipped entry adds nothing to the count.
struct Counted<R> {
    inner: R,
    read: u64,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.read += read as u64;
        Ok(read)
    }
}

impl<R: Seek> Seek for Counted<R> {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(position)
    }
}

/// One entry of a spooled container tar: a path under `weather/`, the
/// size its header states, and the bytes at the start of its data.
/// The rest of the data is a hole in the file, which reads as zeros
/// and takes no disk.
struct Spooled<'a> {
    path: &'a str,
    size: u64,
    content: &'a [u8],
}

/// A container tar in a temporary file, as the daemon spools it.
fn spooled(entries: &[Spooled<'_>]) -> Counted<std::fs::File> {
    let mut file = tempfile::tempfile().expect("a spool file");
    for entry in entries {
        let mut header = tar::Header::new_gnu();
        header
            .set_path(format!("weather/{}", entry.path))
            .expect("path");
        header.set_size(entry.size);
        header.set_mode(0o644);
        header.set_cksum();
        file.write_all(header.as_bytes()).expect("header");
        file.write_all(entry.content).expect("content");
        let padded = entry.size.div_ceil(512) * 512;
        file.seek(SeekFrom::Current(
            (padded - entry.content.len() as u64) as i64,
        ))
        .expect("the hole");
    }
    file.write_all(&[0; 1024]).expect("the end of the archive");
    file.rewind().expect("rewind");
    Counted {
        inner: file,
        read: 0,
    }
}

/// The Docker archive endpoint writes a sparse file of the Computer as
/// a regular entry of its full logical size, so a small sparse file on
/// the Computer arrives here as a large regular entry. (A GNU sparse
/// entry is refused as a special file.) The kept-byte limit stops the
/// read of the entry soon after the limit, far before its end.
#[test]
fn an_entry_whose_logical_size_passes_the_package_limit_is_refused_before_its_end() {
    let size = 2 * MAX_PACKAGE_BYTES;
    let mut container = spooled(&[
        Spooled {
            path: "pagis-software.toml",
            size: 9,
            content: b"[package]",
        },
        Spooled {
            path: "data/sparse.bin",
            size,
            content: b"",
        },
    ]);

    let refused = refusal(pack(&mut container));

    assert_eq!(
        refused,
        format!(
            "the package is larger than the limit of {MAX_PACKAGE_BYTES} bytes after exclusions; \
             \"data/sparse.bin\" passes it"
        )
    );
    assert!(
        container.read <= MAX_PACKAGE_BYTES + 64 * 1024,
        "the pack read {} bytes of an entry of {size}",
        container.read
    );
}

/// The data of a dropped entry is skipped with a seek: the pack never
/// reads it, so it holds none of it.
#[test]
fn the_data_of_a_dropped_entry_is_never_read() {
    let size = 2 * MAX_PACKAGE_BYTES;
    let mut container = spooled(&[
        Spooled {
            path: ".gitignore",
            size: 7,
            content: b"build/\n",
        },
        Spooled {
            path: "pagis-software.toml",
            size: 9,
            content: b"[package]",
        },
        Spooled {
            path: "node_modules/heavy/blob.bin",
            size,
            content: b"",
        },
        Spooled {
            path: "build/out.bin",
            size,
            content: b"",
        },
    ]);

    let packed = pack(&mut container).expect("the pack");

    assert_eq!(
        paths_in(&packed.tar),
        vec![".gitignore", "pagis-software.toml"]
    );
    assert!(
        container.read < 64 * 1024,
        "the pack read {} bytes",
        container.read
    );
}

/// `count` empty files under `weather/`, as `<directory>/<index>`.
fn many_files(directory: &str, count: usize) -> Vec<(String, &'static str)> {
    (0..count)
        .map(|index| (format!("{directory}/{index}"), ""))
        .collect()
}

fn tar_of_owned(files: &[(String, &str)]) -> Vec<u8> {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, content)| (path.as_str(), *content))
        .collect();
    tar_of(&borrowed)
}

#[test]
fn a_package_at_the_entry_limit_packs() {
    let files = many_files("data", MAX_PACKAGE_ENTRIES);

    let packed = pack_of(&tar_of_owned(&files)).expect("the pack");

    assert_eq!(paths_in(&packed.tar).len(), MAX_PACKAGE_ENTRIES);
}

#[test]
fn a_package_over_the_entry_limit_is_refused() {
    let files = many_files("data", MAX_PACKAGE_ENTRIES + 1);

    let refused = refusal(pack_of(&tar_of_owned(&files)));

    assert_eq!(
        refused,
        format!(
            "the package has more than the limit of {MAX_PACKAGE_ENTRIES} entries after \
             exclusions; \"data/{MAX_PACKAGE_ENTRIES}\" passes it"
        )
    );
}

/// A dependency tree holds many small files, and a publish drops it.
/// Its entries do not count toward the entry limit.
#[test]
fn the_entries_of_a_dropped_tree_do_not_count_toward_the_entry_limit() {
    let mut files = vec![("pagis-software.toml".to_string(), "[package]")];
    files.extend(many_files("node_modules/left-pad", 2 * MAX_PACKAGE_ENTRIES));

    let packed = pack_of(&tar_of_owned(&files)).expect("the pack");

    assert_eq!(paths_in(&packed.tar), vec!["pagis-software.toml"]);
}

#[test]
fn a_gitignore_over_its_limit_is_refused() {
    let rules = "*.log\n".repeat(MAX_GITIGNORE_BYTES as usize / 6 + 1);

    let refused = refusal(pack_of(&tar_of(&[
        (".gitignore", &rules),
        ("pagis-software.toml", "[package]"),
    ])));

    assert_eq!(
        refused,
        format!(
            "the .gitignore files are larger than the limit of {MAX_GITIGNORE_BYTES} bytes; \
             \".gitignore\" passes it"
        )
    );
}

/// The first pass holds every rule in memory, so the limit counts the
/// `.gitignore` files together, and not each one alone.
#[test]
fn gitignore_files_over_the_limit_together_are_refused() {
    let rules = "*.log\n".repeat(MAX_GITIGNORE_BYTES as usize / 6 / 2 + 1);

    let refused = refusal(pack_of(&tar_of(&[
        (".gitignore", &rules),
        ("pagis-software.toml", "[package]"),
        ("src/.gitignore", &rules),
    ])));

    assert_eq!(
        refused,
        format!(
            "the .gitignore files are larger than the limit of {MAX_GITIGNORE_BYTES} bytes; \
             \"src/.gitignore\" passes it"
        )
    );
}

/// A dependency tree carries `.gitignore` files of its own, and a
/// publish drops the tree. The first pass does not read them.
#[test]
fn the_gitignore_files_of_a_dropped_tree_are_not_read() {
    let rules = "*.log\n".repeat(MAX_GITIGNORE_BYTES as usize / 6 + 1);

    let packed = pack_of(&tar_of(&[
        ("pagis-software.toml", "[package]"),
        ("node_modules/left-pad/.gitignore", &rules),
    ]))
    .expect("the pack");

    assert_eq!(paths_in(&packed.tar), vec!["pagis-software.toml"]);
}
