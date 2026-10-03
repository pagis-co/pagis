//! The tests of screend, the screen daemon of the Computer Image
//! (`computer/screend`).
//!
//! screend is Linux-only Wayland code with a workspace and a lockfile of
//! its own, and it builds only in the `screend` stage of
//! `computer/Dockerfile`. So the gate step `screend-test` runs
//! `cargo test --locked` in a container of the image that this stage
//! starts from. The Dockerfile pins that image, and the step reads it
//! from there, so one pin moves both. The stage adds only
//! cargo-auditable, which the release build uses and the tests do not.
//!
//! The Exit Proxy refuses every address of the machine that it runs on,
//! so its tests that need a destination that answers are `#[ignore]`d.
//! The step runs them next, on a Docker network of its own, against
//! `serve_the_proxy_test_targets` in a second container, which
//! `PAGIS_EXIT_TEST_TARGETS` names. The step removes that container and
//! that network when it ends, also when a test fails. Both carry
//! [`TEST_LABEL`], so the sweep of the gate removes them after a step
//! that was killed.
//!
//! Two named volumes keep the Cargo registry and the target directory of
//! the containers, so a later run compiles only what changed. They carry
//! no test label, so the sweep keeps them. To start again with no cache,
//! remove them with `docker volume rm pagis-screend-registry
//! pagis-screend-target`.

use std::path::Path;

use crate::{Action, Cmd, Step, TEST_LABEL};

/// The Dockerfile that builds screend, relative to the repository root.
const DOCKERFILE: &str = "computer/Dockerfile";

/// The stage of [`DOCKERFILE`] that builds screend.
const STAGE: &str = "screend";

/// Run the unit tests, then the ignored tests against the targets in a
/// second container. `$1` is the builder image, `$2` the directory of
/// screend, and `$3` the key of the test label.
///
/// The targets container shares the target directory and runs the test
/// binary that the unit tests compiled, so it serves in about a second.
/// Cargo releases its lock on the directory before it runs a test, so
/// the tests that come after it do not wait. The targets print `the proxy
/// test targets serve` when each of their ports listens, and
/// `--nocapture` sends that line to the log of the container. When
/// another run holds the volumes, Cargo makes this run wait, so the step
/// waits up to 300 seconds for the targets.
const SCRIPT: &str = r#"set -eu
image=$1
source=$2
label="$3=$$-screend"
network=pagis-screend-test-$$
targets=pagis-screend-targets-$$
cleanup() {
  docker rm -f "$targets" >/dev/null 2>&1 || true
  docker network rm "$network" >/dev/null 2>&1 || true
}
trap cleanup EXIT
# A signal ends the step through `exit`, which runs the cleanup.
trap 'exit 1' HUP INT TERM
builder() {
  docker run --volume "$source:/src/screend:ro" \
    --volume pagis-screend-registry:/usr/local/cargo/registry \
    --volume pagis-screend-target:/target \
    --env CARGO_TARGET_DIR=/target --workdir /src/screend "$@"
}
builder --rm "$image" cargo test --locked
docker network create --label "$label" "$network" >/dev/null
builder --detach --name "$targets" --network "$network" --label "$label" \
  "$image" cargo test --locked serve_the_proxy_test_targets -- --ignored --nocapture >/dev/null
waited=0
until docker logs "$targets" 2>&1 | grep -q 'the proxy test targets serve'; do
  if [ "$(docker inspect --format '{{.State.Running}}' "$targets")" != true ] || [ "$waited" -ge 300 ]; then
    docker logs "$targets" >&2 || true
    echo "the proxy test targets in $targets do not serve" >&2
    exit 1
  fi
  sleep 1
  waited=$((waited + 1))
done
builder --rm --network "$network" --env "PAGIS_EXIT_TEST_TARGETS=$targets" \
  "$image" cargo test --locked -- --ignored --skip serve_the_proxy_test_targets
"#;

/// The image of the `screend` stage of `dockerfile`: the image of its
/// `FROM <image> AS screend` line. Stage names are case-insensitive, as
/// in Docker.
pub fn builder_image(dockerfile: &str) -> Option<&str> {
    dockerfile.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        if !words.next()?.eq_ignore_ascii_case("FROM") {
            return None;
        }
        let words: Vec<&str> = words.filter(|word| !word.starts_with("--")).collect();
        match words.as_slice() {
            [image, keyword, name]
                if keyword.eq_ignore_ascii_case("AS") && name.eq_ignore_ascii_case(STAGE) =>
            {
                Some(*image)
            }
            _ => None,
        }
    })
}

/// The gate step that runs every test of screend at the repository
/// `root`. A host without Docker skips it.
pub fn test_step(root: &Path, docker_available: bool) -> Step {
    let action = if docker_available {
        Action::Run(vec![test_cmd(root)])
    } else {
        Action::Skip("Docker unavailable".into())
    };
    Step {
        name: "screend-test",
        action,
    }
}

/// The command of [`test_step`], or a command that fails and says why
/// the tests cannot run.
fn test_cmd(root: &Path) -> Cmd {
    let dockerfile = match std::fs::read_to_string(root.join(DOCKERFILE)) {
        Ok(text) => text,
        Err(error) => {
            return Cmd::refusal(&format!("cannot read {DOCKERFILE}: {error}")).in_dir(root);
        }
    };
    let Some(image) = builder_image(&dockerfile) else {
        return Cmd::refusal(&format!("{DOCKERFILE} has no `{STAGE}` stage")).in_dir(root);
    };
    let source = root.join("computer/screend");
    Cmd::new(
        "sh",
        &[
            "-c",
            SCRIPT,
            "sh",
            image,
            &source.to_string_lossy(),
            TEST_LABEL,
        ],
    )
    .in_dir(root)
}
