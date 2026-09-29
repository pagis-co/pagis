use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use xtask::desktop;
use xtask::{
    all_green, dev_lanes, docker_available, execute, execute_lanes, execute_until_failure,
    full_lanes, local_orphaned_test_objects, named_steps, run_cmds, summary, target_dir,
    test_object_sweep,
};

fn workspace_root() -> PathBuf {
    PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level under the workspace root")
        .to_path_buf()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("dev") => {
            let root = workspace_root();
            let paths = changed_paths(&root)?;
            println!("checking {} changed path(s)", paths.len());
            let docker = docker_available();
            sweep_test_objects(docker);
            exit_on_red(run_lanes(dev_lanes(&root, &paths, docker)?))
        }
        Some("full") => exit_on_red(run_full()),
        Some("step") if args.len() > 1 => {
            let steps = named_steps(&workspace_root(), &args[1..], docker_available())?;
            let results = execute(&steps);
            println!("\n{}", summary(&results));
            exit_on_red(all_green(&results))
        }
        Some("advisories") => {
            let root = workspace_root();
            let lane = xtask::advisories::published_advisory_lane(&root, &target_dir(&root));
            exit_on_red(run_lanes(vec![lane]))
        }
        Some("image") => run_image(&workspace_root(), args.iter().any(|a| a == "--dry-run")),
        Some("server-image") => {
            run_server_image(&workspace_root(), args.iter().any(|a| a == "--dry-run"))
        }
        Some("release") if args.len() > 1 => run_release(
            xtask::ReleaseStage::parse(&args[1])?,
            flag_value(&args, "--tag"),
            args.iter().any(|a| a == "--dry-run"),
        ),
        Some("desktop") => run_desktop(&args),
        Some("runtime-lock") => run_runtime_lock(&args),
        Some("emergency-numbers") => {
            xtask::emergency::run(&workspace_root(), args.iter().any(|a| a == "--check"))
        }
        Some("pins") if args.get(1).map(String::as_str) == Some("--check") => {
            xtask::pins::run(&workspace_root())
        }
        _ => bail!(
            "usage: cargo xtask <dev | full | step <name>... | advisories | image [--dry-run] | \
             server-image [--dry-run] | release <images | linux | macos | draft> [--tag <tag>] [--dry-run] | desktop [--linux] [--tag <tag>] [--prepare | --publish-existing] [--dry-run] | \
             emergency-numbers [--check] | pins --check>"
        ),
    }
}

/// End the process with a failure status when a check is red.
fn exit_on_red(green: bool) -> Result<()> {
    if !green {
        std::process::exit(1);
    }
    Ok(())
}

fn run_runtime_lock(args: &[String]) -> Result<()> {
    use xtask::release::ClientPlatform;
    match args.get(1).map(String::as_str) {
        Some("write") if args.len() == 8 || args.len() == 9 => xtask::release::write_runtime_lock(
            &xtask::release::RuntimeLockRequest {
                platform: ClientPlatform::parse(&args[2])?,
                release: &args[3],
                computer_image: &args[4],
                asset: Path::new(&args[5]),
                package_dir: Path::new(&args[6]),
                team_id: args.get(8).map(String::as_str),
            },
            Path::new(&args[7]),
        ),
        Some("validate") if args.len() == 5 => xtask::release::validate_runtime_lock(
            Path::new(&args[2]),
            Path::new(&args[4]),
            Path::new(&args[3]),
        ),
        Some("validate-metadata") if args.len() == 5 => {
            xtask::release::validate_runtime_lock_metadata(
                Path::new(&args[2]),
                &args[3],
                ClientPlatform::parse(&args[4])?,
            )
        }
        Some("write-fixture") if args.len() == 5 => xtask::release::write_fixture_runtime_lock(
            &args[2],
            ClientPlatform::parse(&args[3])?,
            Path::new(&args[4]),
        ),
        _ => bail!(
            "usage: cargo xtask runtime-lock write <platform> <release> <image-digest> \
             <server-package> <package-dir> <output> [<team-id>] | \
             runtime-lock validate <lock> <server-package> <package-dir> | \
             runtime-lock validate-metadata <lock> <release> <platform> | \
             runtime-lock write-fixture <release> <platform> <output>; \
             <platform> is darwin-arm64, linux-x64 or linux-arm64, and darwin-arm64 needs <team-id>"
        ),
    }
}

/// `cargo xtask image`: build the pinned computer image for both
/// architectures, scan its filesystem for secrets and for known
/// vulnerabilities, and push it to GHCR, which is where the daemon pulls
/// it from the first time an agent wakes. The push needs a Docker login
/// to GHCR with `write:packages`.
fn run_image(root: &Path, dry_run: bool) -> Result<()> {
    let image = pagis_versions::COMPUTER_IMAGE;
    let dockerfile = std::fs::read_to_string(root.join("computer/Dockerfile"))?;
    if let Err(reason) = xtask::check_pin(
        &dockerfile,
        "computer/Dockerfile",
        xtask::VERSION_LABEL,
        pagis_versions::COMPUTER_IMAGE_VERSION,
    ) {
        bail!("the image publish is refused: {reason}");
    }
    let steps = xtask::image_plan(root, image, &xtask::target_dir(root));

    if dry_run {
        println!("{image}");
        println!("{}", xtask::plan_summary(&steps));
        return Ok(());
    }

    if !docker_available() {
        bail!("Docker is unreachable; the image build and the push need it");
    }
    let results = execute_until_failure(&steps);
    println!("\n{}", summary(&results));
    if !all_green(&results) {
        bail!("the image publish failed");
    }
    println!("pushed {image}");
    Ok(())
}

/// `cargo xtask server-image`: build the headless Linux server image
/// for both architectures, scan its filesystem for secrets and for known
/// vulnerabilities, and push it to GHCR, which is where a team's VM pulls
/// it from. The push needs a Docker login to GHCR with `write:packages`.
fn run_server_image(root: &Path, dry_run: bool) -> Result<()> {
    let version = workspace_version(root)?;
    let image = xtask::server_image(&version);
    let dockerfile = std::fs::read_to_string(root.join("Dockerfile"))?;
    // The image says which release is in it, and a release is one
    // number across the artifacts, so the label and the workspace
    // version agree before anything reaches the registry.
    if let Err(reason) = xtask::check_pin(
        &dockerfile,
        "Dockerfile",
        xtask::server_image::VERSION_LABEL,
        &version,
    ) {
        bail!("the server image publish is refused: {reason}");
    }
    let steps = xtask::server_image_plan(root, &image, &xtask::target_dir(root));

    if dry_run {
        println!("{image}");
        println!("{}", xtask::plan_summary(&steps));
        return Ok(());
    }

    if !docker_available() {
        bail!("Docker is unreachable; the image build and the push need it");
    }
    let results = execute_until_failure(&steps);
    println!("\n{}", summary(&results));
    if !all_green(&results) {
        bail!("the server image publish failed");
    }
    println!("pushed {image}");
    Ok(())
}

/// `cargo xtask release <stage>`: one stage of a release. The release
/// workflow runs the gate first, then each stage in a job of its own
/// (`docs/RELEASING-SERVER.md`). The images stage runs the advisory checks
/// before it builds. Each stage stops at its first failed step, so nothing
/// is pushed or published after a failed secret scan or vulnerability
/// scan. `--tag` names the tag the workflow runs on, which must name the
/// workspace version. `--dry-run` prints the plan and stops.
fn run_release(stage: xtask::ReleaseStage, tag: Option<String>, dry_run: bool) -> Result<()> {
    let root = workspace_root();
    let version = workspace_version(&root)?;
    if let Some(tag) = &tag {
        desktop::check_tag(tag, &version)?;
    }
    let image = pagis_versions::COMPUTER_IMAGE.to_string();
    if stage.needs_macos() && !cfg!(target_os = "macos") && !dry_run {
        bail!("the {} stage runs on a macOS host", stage.name());
    }
    let needs_docker = matches!(
        stage,
        xtask::ReleaseStage::Images | xtask::ReleaseStage::Linux
    );
    if needs_docker && !dry_run && !docker_available() {
        bail!("Docker is unreachable; the image builds and the cross builds need it");
    }
    let cx = xtask::ReleaseContext {
        version,
        computer_published: stage == xtask::ReleaseStage::Images
            && !dry_run
            && xtask::image_published(&image)?,
        image,
        target_dir: std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("target")),
    };
    let steps = xtask::release_plan(&root, &cx, stage);

    if dry_run {
        println!("pagis {} {} → {}", cx.version, stage.name(), cx.image);
        println!("{}", xtask::plan_summary(&steps));
        return Ok(());
    }

    if stage == xtask::ReleaseStage::Images {
        println!("== release: advisories");
        let advisories = xtask::advisories::advisory_lane(&root, &cx.target_dir, true);
        if !run_lanes(vec![advisories]) {
            bail!("release refused: an advisory check is red");
        }
    }

    let results = execute_until_failure(&steps);
    println!("\n{}", summary(&results));
    if !all_green(&results) {
        bail!("the release {} stage failed", stage.name());
    }
    println!("pagis {} {} stage done", cx.version, stage.name());
    Ok(())
}

/// `cargo xtask desktop`: pack and test the client-only Electron app.
/// A macOS host packs the macOS client and a Linux host the Linux one;
/// `--linux` names the Linux plan on any host, which is how a macOS host
/// publishes Linux packages that CI prepared. A tagged prepare run makes
/// exact bytes. A publish-existing run publishes those bytes after a
/// maintainer approves the release.
fn run_desktop(args: &[String]) -> Result<()> {
    let root = workspace_root();
    let dry_run = args.iter().any(|arg| arg == "--dry-run");
    let publish_existing = args.iter().any(|arg| arg == "--publish-existing");
    let platform = if args.iter().any(|arg| arg == "--linux") || cfg!(target_os = "linux") {
        xtask::DesktopPlatform::Linux
    } else if cfg!(target_os = "macos") {
        xtask::DesktopPlatform::Mac
    } else {
        bail!("the Client App packs on macOS and Linux only");
    };
    if platform == xtask::DesktopPlatform::Linux
        && !cfg!(target_os = "linux")
        && !publish_existing
        && !dry_run
    {
        bail!(
            "the Linux Client App packs on a Linux host; another host can only --publish-existing"
        );
    }
    let version = workspace_version(&root)?;
    desktop::check_version(&root, &version)?;
    let tag = flag_value(args, "--tag");
    if let Some(tag) = &tag {
        desktop::check_tag(tag, &version)?;
    }
    let is_set = |name: &str| std::env::var(name).is_ok_and(|value| !value.trim().is_empty());
    let missing = match platform {
        // The publication of the Mac client uploads the bytes that were
        // signed and notarized when they were prepared, and signs nothing.
        xtask::DesktopPlatform::Mac if publish_existing => Vec::new(),
        xtask::DesktopPlatform::Mac => desktop::missing_signing_inputs(&is_set),
        // Linux signs only at publication, with the release key.
        xtask::DesktopPlatform::Linux if publish_existing => {
            xtask::desktop_linux::missing_signing_inputs(&is_set)
        }
        xtask::DesktopPlatform::Linux => Vec::new(),
    };
    let cx = xtask::DesktopContext {
        platform,
        version,
        tag,
        missing_credentials: missing,
        prepare_only: args.iter().any(|arg| arg == "--prepare"),
        publish_existing,
    };
    if cx.prepare_only && cx.publish_existing {
        bail!("--prepare and --publish-existing cannot be used together");
    }
    if (cx.prepare_only || cx.publish_existing) && cx.tag.is_none() {
        bail!("--prepare and --publish-existing require --tag <tag>");
    }
    if cx.tag.is_some() && !cx.prepare_only && !cx.publish_existing {
        bail!("a tagged client needs --prepare or --publish-existing");
    }
    if cx.tag.is_some() && !cx.missing_credentials.is_empty() && !dry_run {
        bail!(
            "release signing needs {}",
            cx.missing_credentials.join("; ")
        );
    }
    let steps = xtask::desktop_plan(&root, &cx);

    if dry_run {
        println!("{}", xtask::plan_summary(&steps));
        return Ok(());
    }

    let results = execute(&steps);
    println!("\n{}", summary(&results));
    if !all_green(&results) {
        bail!("the desktop packaging failed");
    }
    Ok(())
}

/// The value that follows a flag, as in `--tag v1.2.3`.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    args.get(at + 1).cloned()
}

/// The version every release asset is named after: the one version the
/// whole workspace shares.
fn workspace_version(root: &Path) -> Result<String> {
    let manifest = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let value: toml::Value = toml::from_str(&manifest)?;
    value["workspace"]["package"]["version"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("no workspace.package.version in Cargo.toml"))
}

/// The full gate.
fn run_full() -> bool {
    let root = workspace_root();
    let docker = docker_available();
    sweep_test_objects(docker);
    run_lanes(full_lanes(&root, docker))
}

/// Remove the Docker objects of test processes that no longer run. A
/// test removes its own objects when it ends, but a test that is killed
/// leaves them, and each Tenant Network it leaves holds one subnet of
/// Docker's address pools until no new network fits. A failed removal
/// does not fail the check.
fn sweep_test_objects(docker: bool) {
    if docker {
        run_cmds(
            "test objects",
            &test_object_sweep(&local_orphaned_test_objects()),
        );
    }
}

fn run_lanes(lanes: Vec<xtask::Lane>) -> bool {
    let results = execute_lanes(&lanes);
    println!("\n{}", summary(&results));
    all_green(&results)
}

/// The paths that differ from `origin/main`: the commits of the branch,
/// the changes of the working tree, and the untracked files.
fn changed_paths(root: &Path) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    append_git_paths(
        root,
        &["diff", "--name-only", "origin/main...HEAD"],
        &mut paths,
    )?;
    append_git_paths(root, &["diff", "--name-only", "HEAD"], &mut paths)?;
    append_git_paths(
        root,
        &["ls-files", "--others", "--exclude-standard"],
        &mut paths,
    )?;
    paths.sort();
    paths.dedup();
    Ok(paths)
}

fn append_git_paths(root: &Path, args: &[&str], paths: &mut Vec<String>) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    paths.extend(
        String::from_utf8(output.stdout)?
            .lines()
            .filter(|path| !path.is_empty())
            .map(str::to_string),
    );
    Ok(())
}
