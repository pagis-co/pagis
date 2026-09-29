//! The Docker objects of one test over the real runtime, and their
//! removal. The daemon removes no Tenant Network (ADR-0014), so a test
//! that creates Docker objects removes them itself.

use std::process::Command;

use crate::TEST_LABEL;

/// The mark of one test on the Docker objects it creates.
///
/// A runtime built with [`TestDocker::labels`] puts the mark on every
/// container, volume and Tenant Network it creates. Dropping the guard
/// removes every object with the mark, when the test passes and when it
/// panics. A test process that is killed drops nothing, so the mark
/// starts with the process id, and `cargo xtask` removes the objects of
/// a process that does not run.
///
/// The Docker CLI does the removal, because a drop cannot wait for an
/// async call on the runtime the test runs on.
pub struct TestDocker {
    mark: String,
}

impl TestDocker {
    pub fn new() -> Self {
        Self {
            mark: format!("{}-{:016x}", std::process::id(), rand::random::<u64>()),
        }
    }

    /// The labels a runtime of this test gives every object, for
    /// [`RuntimeOptions::labels`](crate::RuntimeOptions::labels).
    pub fn labels(&self) -> Vec<(String, String)> {
        vec![(TEST_LABEL.to_string(), self.mark.clone())]
    }

    /// The value of [`TEST_LABEL`] on every object of this test.
    pub fn mark(&self) -> &str {
        &self.mark
    }

    /// Remove every object with the mark. The containers go first,
    /// because Docker does not remove a network while a container is
    /// attached to it.
    fn remove(&self) -> Result<(), String> {
        for (list, remove) in [
            (&["ps", "-aq"][..], &["rm", "-f"][..]),
            (&["network", "ls", "-q"], &["network", "rm"]),
            (&["volume", "ls", "-q"], &["volume", "rm", "-f"]),
        ] {
            let ids = list_marked(list, &self.mark)?;
            if ids.is_empty() {
                continue;
            }
            let output = Command::new("docker")
                .args(remove)
                .args(&ids)
                .output()
                .map_err(|error| format!("docker {}: {error}", remove.join(" ")))?;
            if !output.status.success() {
                return Err(format!(
                    "docker {} {}: {}",
                    remove.join(" "),
                    ids.join(" "),
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
        }
        Ok(())
    }
}

/// The names of the containers, Tenant Networks and volumes whose
/// [`TEST_LABEL`] is `mark`, in that order.
pub fn marked_objects(mark: &str) -> Result<Vec<String>, String> {
    let mut names = list_marked(&["ps", "-a", "--format", "{{.Names}}"], mark)?;
    names.extend(list_marked(
        &["network", "ls", "--format", "{{.Name}}"],
        mark,
    )?);
    names.extend(list_marked(
        &["volume", "ls", "--format", "{{.Name}}"],
        mark,
    )?);
    Ok(names)
}

/// The objects of one listing whose [`TEST_LABEL`] is `mark`.
fn list_marked(args: &[&str], mark: &str) -> Result<Vec<String>, String> {
    let filter = format!("label={TEST_LABEL}={mark}");
    let output = Command::new("docker")
        .args(args)
        .args(["--filter", &filter])
        .output()
        .map_err(|error| format!("docker {}: {error}", args.join(" ")))?;
    if !output.status.success() {
        return Err(format!(
            "docker {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect())
}

impl Default for TestDocker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TestDocker {
    /// A drop that panics during a test's own panic aborts the process
    /// and hides the test's failure, so a failed removal is reported and
    /// the sweep of `cargo xtask` removes what is left.
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!(
                "the Docker objects with {TEST_LABEL}={} are not removed: {error}",
                self.mark
            );
        }
    }
}
