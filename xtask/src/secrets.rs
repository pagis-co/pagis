//! The secret scan. gitleaks scans the tracked files in the gate, and the
//! exported filesystem of each image before a publish pushes it. Anyone
//! can read the public tree and pull a public image, and a secret stays
//! valid after its published copy is deleted, so the scan runs before
//! either becomes public.
//!
//! The scan fails on every finding. `.gitleaks.toml` holds each
//! exception with its reason. The scan ignores `gitleaks:allow` comments
//! and refuses a `.gitleaksignore` file, because they hold no reason.
//! gitleaks prints each finding with its secret redacted, so no log
//! holds it.

use std::path::Path;

use crate::{Action, Cmd, Step, tools};

/// The stored configuration of the scan: the default rules and the
/// allowlist, at the repository root.
pub const CONFIG: &str = ".gitleaks.toml";

/// Scan the tracked files of the working tree at `root`, as the working
/// tree holds them: a tracked change shows before it is staged, and an
/// untracked or ignored file, such as a local `.env`, is not read.
///
/// The scan reads a copy of the index in which `git add --update` puts
/// the tracked files of the working tree, so the real index, the refs
/// and the working tree do not change. The copy of the tree carries its
/// own `.gitleaks.toml`, so the allowlist that applies is the tracked
/// one.
pub fn tree_secret_scan_step(root: &Path, target_dir: &Path) -> Step {
    Step {
        name: "secret-scan",
        action: Action::Run(vec![gitleaks_cmd(
            root,
            target_dir,
            "index=\"$work/index\"\n\
             cp \"$(git rev-parse --git-path index)\" \"$index\"\n\
             GIT_INDEX_FILE=\"$index\" git add --update\n\
             GIT_INDEX_FILE=\"$index\" git checkout-index --all --prefix=\"$work/tree/\"\n\
             cd \"$work/tree\"\n\
             scan . .gitleaks.toml\n",
            &[],
        )]),
    }
}

/// Scan `export`, the directory a build step exported the filesystem of
/// an image to, with the `.gitleaks.toml` of the repository at `root`.
/// gitleaks follows no symbolic link, so an absolute link in the image
/// does not lead the scan out of `export`.
pub fn image_secret_scan_step(
    root: &Path,
    name: &'static str,
    export: &str,
    target_dir: &Path,
) -> Step {
    Step {
        name,
        action: Action::Run(vec![gitleaks_cmd(
            root,
            target_dir,
            "scan \"$1\" .gitleaks.toml\n",
            &[export],
        )]),
    }
}

/// A command that runs `body` in `sh` at `root`, with the pinned gitleaks
/// at `$work/gitleaks` (see [`tools::command`]).
/// `body` scans a directory with `scan <directory> <config>`, and reads
/// `args` as `$1` and on.
fn gitleaks_cmd(root: &Path, target_dir: &Path, body: &str, args: &[&str]) -> Cmd {
    let body = format!(
        "scan() {{\n\
           if [ -e \"$1/.gitleaksignore\" ]; then\n\
             echo \"$1/.gitleaksignore hides findings without a reason; put each exception in {CONFIG} with its reason\" >&2\n\
             exit 1\n\
           fi\n\
           \"$work/gitleaks\" dir --no-banner --redact --verbose --ignore-gitleaks-allow \
         --gitleaks-ignore-path \"$work\" --config \"$2\" \"$1\"\n\
         }}\n\
         {body}"
    );
    tools::command(root, target_dir, tools::gitleaks(), &body, args)
}
