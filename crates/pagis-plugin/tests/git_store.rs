//! The installed states of a Plugin (ADR-0017): the bare
//! repository, the checkout that is `PLUGIN_ROOT`, the data directory
//! that survives an update, and the diff two states have.

use crate::support;

use pagis_core::WorkspaceId;
use pagis_plugin::{ChangeStatus, GitStoreError, PluginGitStore};
use support::minimal;

fn store() -> (PluginGitStore, tempfile::TempDir, WorkspaceId) {
    let home = tempfile::tempdir().expect("a data root");
    let store = PluginGitStore::new(home.path().join("plugins"));
    (store, home, WorkspaceId::generate())
}

#[tokio::test]
async fn an_install_commits_the_tree_and_checks_it_out() {
    let (store, _home, workspace) = store();
    let package = minimal().file("server.js", "// one");

    let commit = store
        .install_state(&workspace, "plug-1", package.root(), "v1")
        .await
        .expect("the first state");

    assert_eq!(commit.len(), 40, "the state is one commit");
    let paths = store.paths(&workspace, "plug-1");
    assert!(paths.repository.join("HEAD").is_file(), "a bare repository");
    assert_eq!(
        std::fs::read_to_string(paths.root.join("server.js")).expect("the checkout"),
        "// one"
    );
    assert!(paths.data.is_dir(), "the data directory is made");
}

#[tokio::test]
async fn a_second_state_commits_on_the_first_and_has_a_diff() {
    let (store, _home, workspace) = store();
    let first = minimal()
        .file("server.js", "// one")
        .file("old.txt", "gone");
    let base = store
        .install_state(&workspace, "plug-1", first.root(), "v1")
        .await
        .expect("the first state");

    let second = minimal()
        .file("server.js", "// two")
        .file("README.md", "new");
    let head = store
        .install_state(&workspace, "plug-1", second.root(), "v2")
        .await
        .expect("the second state");

    let changed = store
        .diff(&workspace, "plug-1", &base, &head)
        .await
        .expect("the diff");
    let mut summary: Vec<(String, ChangeStatus)> = changed
        .files
        .into_iter()
        .map(|file| (file.path, file.status))
        .collect();
    summary.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(
        summary,
        vec![
            ("README.md".to_string(), ChangeStatus::Added),
            ("old.txt".to_string(), ChangeStatus::Deleted),
            ("server.js".to_string(), ChangeStatus::Modified),
        ]
    );
}

#[tokio::test]
async fn a_new_state_replaces_the_checkout() {
    let (store, _home, workspace) = store();
    let first = minimal().file("old.txt", "gone");
    store
        .install_state(&workspace, "plug-1", first.root(), "v1")
        .await
        .expect("the first state");

    let second = minimal().file("new.txt", "here");
    store
        .install_state(&workspace, "plug-1", second.root(), "v2")
        .await
        .expect("the second state");

    let paths = store.paths(&workspace, "plug-1");
    assert!(!paths.root.join("old.txt").exists(), "the old file is gone");
    assert!(paths.root.join("new.txt").is_file());
}

#[tokio::test]
async fn the_data_directory_survives_an_update() {
    let (store, _home, workspace) = store();
    let package = minimal();
    store
        .install_state(&workspace, "plug-1", package.root(), "v1")
        .await
        .expect("the first state");
    let paths = store.paths(&workspace, "plug-1");
    std::fs::write(paths.data.join("state.json"), "{}").expect("plugin state");

    let second = minimal().file("README.md", "new");
    store
        .install_state(&workspace, "plug-1", second.root(), "v2")
        .await
        .expect("the second state");

    assert!(paths.data.join("state.json").is_file());
}

#[tokio::test]
async fn a_state_equal_to_the_installed_one_is_refused() {
    let (store, _home, workspace) = store();
    let package = minimal();
    store
        .install_state(&workspace, "plug-1", package.root(), "v1")
        .await
        .expect("the first state");

    let again = store
        .install_state(&workspace, "plug-1", package.root(), "v2")
        .await;

    assert_eq!(again, Err(GitStoreError::NoChange));
}

#[tokio::test]
async fn a_nested_tree_keeps_its_shape_and_its_file_mode() {
    let (store, _home, workspace) = store();
    let package = minimal().file("skills/forecast/SKILL.md", "# Forecast");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = package.root().join("bin/run.sh");
        std::fs::create_dir_all(script.parent().expect("bin")).expect("bin");
        std::fs::write(&script, "#!/bin/sh\n").expect("the script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("the mode");
    }

    store
        .install_state(&workspace, "plug-1", package.root(), "v1")
        .await
        .expect("the first state");

    let paths = store.paths(&workspace, "plug-1");
    assert!(paths.root.join("skills/forecast/SKILL.md").is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(paths.root.join("bin/run.sh"))
            .expect("the script")
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "the script stays executable");
    }
}

#[tokio::test]
async fn a_remove_deletes_every_directory_of_the_plugin() {
    let (store, _home, workspace) = store();
    let package = minimal();
    store
        .install_state(&workspace, "plug-1", package.root(), "v1")
        .await
        .expect("the first state");
    let paths = store.paths(&workspace, "plug-1");

    store.remove(&workspace, "plug-1").await.expect("removed");

    assert!(!paths.repository.exists());
    assert!(!paths.root.exists());
    assert!(!paths.data.exists());
}

/// The modes of a checkout. The Plugin Computer mounts the checkout
/// read-only, and the plugin server in it runs as a uid that does not
/// own the files.
#[cfg(unix)]
mod checkout_modes {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    use rustix::fs::Mode;

    use super::{minimal, store, support};

    /// Every uid reads the checkout, also when the daemon runs under
    /// umask 077.
    #[tokio::test]
    async fn every_uid_reads_a_checkout_that_the_daemon_writes_under_umask_077() {
        let (store, _home, workspace) = store();
        let package = executable_package();

        let installed = {
            let _umask = Umask::set(Mode::RWXG | Mode::RWXO);
            store
                .install_state(&workspace, "plug-1", package.root(), "v1")
                .await
        };

        installed.expect("the first state");
        assert_readable(&store.paths(&workspace, "plug-1").root);
    }

    /// A restore gives every file of a Backup the owner's bits alone.
    /// The daemon makes the checkout readable to every uid again before
    /// a Computer mounts it.
    #[tokio::test]
    async fn a_checkout_that_only_the_owner_reads_is_made_readable_again() {
        let (store, _home, workspace) = store();
        let package = executable_package();
        store
            .install_state(&workspace, "plug-1", package.root(), "v1")
            .await
            .expect("the first state");
        let root = store.paths(&workspace, "plug-1").root;
        for path in tree(&root) {
            let mode = std::fs::metadata(&path)
                .expect("a path")
                .permissions()
                .mode();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o700))
                .expect("the owner's bits alone");
        }

        store
            .make_checkout_readable(&workspace, "plug-1")
            .await
            .expect("the checkout is readable again");

        assert_readable(&root);
        // A Plugin with no checkout, as after an uninstall, is no error.
        store
            .make_checkout_readable(&workspace, "plug-2")
            .await
            .expect("no checkout, nothing to change");
    }

    /// A package with a nested file and an executable script.
    fn executable_package() -> support::Package {
        let package = minimal().file("lib/tools/forecast.js", "// forecast");
        let script = package.root().join("bin/run.sh");
        std::fs::create_dir_all(script.parent().expect("bin")).expect("bin");
        std::fs::write(&script, "#!/bin/sh\n").expect("the script");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("the mode");
        package
    }

    /// Every directory and executable of the checkout is mode 755, and
    /// every other file is mode 644.
    fn assert_readable(root: &Path) {
        let paths = tree(root);
        assert!(paths.iter().any(|path| path.ends_with("bin/run.sh")));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("lib/tools/forecast.js"))
        );
        for path in paths {
            let mode = std::fs::metadata(&path)
                .expect("a path")
                .permissions()
                .mode()
                & 0o777;
            let expected = if path.is_dir() || path.ends_with("bin/run.sh") {
                0o755
            } else {
                0o644
            };
            assert_eq!(mode, expected, "{} is mode {mode:o}", path.display());
        }
    }

    /// Every path under `root`, `root` too.
    fn tree(root: &Path) -> Vec<PathBuf> {
        let mut paths = vec![root.to_path_buf()];
        let mut next = 0;
        while next < paths.len() {
            let path = paths[next].clone();
            next += 1;
            if path.is_dir() {
                for entry in std::fs::read_dir(&path).expect("a directory") {
                    paths.push(entry.expect("an entry").path());
                }
            }
        }
        paths
    }

    /// The umask of this test process, until the value drops. nextest
    /// runs each test in a process of its own, so no other test sees it.
    struct Umask(Mode);

    impl Umask {
        fn set(mask: Mode) -> Self {
            Self(rustix::process::umask(mask))
        }
    }

    impl Drop for Umask {
        fn drop(&mut self) {
            rustix::process::umask(self.0);
        }
    }
}
