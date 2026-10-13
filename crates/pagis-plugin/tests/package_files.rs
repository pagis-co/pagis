//! Archive and directory checks for a Plugin Package held in memory.

use pagis_plugin::PackageFiles;

fn archive(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, bytes, mode) in entries {
        let mut header = tar::Header::new_gnu();
        // Set the bytes directly so a test can hold a path tar::Builder refuses.
        header.as_mut_bytes()[..path.len()].copy_from_slice(path.as_bytes());
        header.set_size(bytes.len() as u64);
        header.set_mode(*mode);
        header.set_cksum();
        builder.append(&header, *bytes).unwrap();
    }
    builder.into_inner().unwrap()
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_archive_path_that_leaves_the_package_is_refused() {
    for path in ["../outside", "root/../../outside", "/outside"] {
        let error = PackageFiles::from_tar(&archive(&[(path, b"data", 0o644)]))
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            format!("the upload carries {path}, which leaves the plugin root")
        );
    }
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn one_shared_root_is_removed_and_executable_bits_are_kept() {
    let files = PackageFiles::from_tar(&archive(&[
        ("weather/plugin.json", b"manifest", 0o644),
        ("weather/bin/run", b"script", 0o751),
    ]))
    .unwrap();
    assert_eq!(files.get("plugin.json").unwrap().bytes, b"manifest");
    assert!(!files.get("plugin.json").unwrap().executable);
    assert_eq!(files.get("bin/run").unwrap().bytes, b"script");
    assert!(files.get("bin/run").unwrap().executable);
    assert!(files.get("weather/plugin.json").is_none());
    assert!(files.is_dir("bin"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn files_without_a_shared_root_keep_their_paths() {
    let files = PackageFiles::from_tar(&archive(&[
        ("./plugin.json", b"manifest", 0o644),
        ("./skills/forecast/SKILL.md", b"skill", 0o644),
    ]))
    .unwrap();
    assert!(files.is_file("plugin.json"));
    assert!(files.is_dir("skills/forecast"));
    assert!(!files.is_file("SKILL.md"));
    let files = PackageFiles::from_tar(&archive(&[("one/a", b"a", 0o644), ("two/b", b"b", 0o644)]))
        .unwrap();
    assert!(files.is_file("one/a"));
    assert!(files.is_file("two/b"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn directories_exist_only_when_a_file_is_under_them() {
    let mut files = PackageFiles::default();
    assert!(!files.is_dir("."));
    files
        .insert("skills/forecast/scripts/run", b"script".to_vec(), true)
        .unwrap();
    for directory in [
        ".",
        "",
        "skills",
        "skills/forecast",
        "skills/forecast/scripts",
    ] {
        assert!(files.is_dir(directory), "{directory}");
    }
    assert!(!files.is_dir("skills/fore"));
    assert!(!files.is_dir("skills/forecast/scripts/run"));
    assert!(!files.is_file("skills/forecast"));
    assert!(!files.is_dir("empty"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn an_empty_or_invalid_archive_is_refused() {
    assert_eq!(
        PackageFiles::from_tar(&archive(&[]))
            .unwrap_err()
            .to_string(),
        "the upload holds no file"
    );
    assert!(PackageFiles::from_tar(b"not a tar").is_err());
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn inserted_paths_stay_inside_the_package() {
    let mut files = PackageFiles::default();
    for path in ["../outside", "/outside", "", "."] {
        assert!(files.insert(path, vec![], false).is_err(), "{path}");
    }
    files
        .insert("./bin/./run", b"script".to_vec(), true)
        .unwrap();
    assert!(files.is_file("bin/run"));
}

#[cfg(all(feature = "native", not(target_arch = "wasm32")))]
#[test]
fn a_checkout_read_keeps_file_bytes_and_executable_flags() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("bin")).unwrap();
    std::fs::create_dir(root.path().join("empty")).unwrap();
    let script = root.path().join("bin/run");
    std::fs::write(&script, b"script").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o751)).unwrap();
    }
    let files = PackageFiles::from_directory(root.path()).unwrap();
    assert_eq!(files.get("bin/run").unwrap().bytes, b"script");
    #[cfg(unix)]
    assert!(files.get("bin/run").unwrap().executable);
    assert!(!files.is_dir("empty"));
}

#[cfg_attr(target_arch = "wasm32", wasm_bindgen_test::wasm_bindgen_test)]
#[cfg_attr(not(target_arch = "wasm32"), test)]
fn archive_directories_and_links_are_not_package_files() {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, kind) in [
        ("empty", tar::EntryType::Directory),
        ("link", tar::EntryType::Symlink),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(0);
        header.set_mode(0o755);
        header.set_entry_type(kind);
        if kind.is_symlink() {
            header.set_link_name("outside").unwrap();
        }
        header.set_cksum();
        builder.append_data(&mut header, path, &[][..]).unwrap();
    }
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "file", &b"data"[..])
        .unwrap();
    let files = PackageFiles::from_tar(&builder.into_inner().unwrap()).unwrap();
    assert!(files.is_file("file"));
    assert!(!files.is_dir("empty"));
    assert!(files.get("link").is_none());
}
