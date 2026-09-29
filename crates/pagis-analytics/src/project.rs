//! The PostHog project a release build sends to, and the reasons a
//! daemon sends nothing.

use serde::Serialize;

/// The PostHog US Cloud ingestion host.
pub const HOST: &str = "https://us.i.posthog.com";

/// The PostHog project a daemon sends to. The release build sets
/// `PAGIS_POSTHOG_PROJECT_ID` and `PAGIS_POSTHOG_TOKEN` in its
/// environment, and the compiler writes them into the binary. The
/// repository holds neither.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub id: String,
    /// The project token. It can only send events: it reads nothing.
    pub token: String,
    /// The ingestion host: [`HOST`], or a test server.
    pub host: String,
}

impl Project {
    /// The project this build holds, or `None` for a build from source.
    pub fn of_this_build() -> Option<Project> {
        Self::from_build(
            option_env!("PAGIS_POSTHOG_PROJECT_ID"),
            option_env!("PAGIS_POSTHOG_TOKEN"),
        )
    }

    /// A project only when the build set both values.
    fn from_build(id: Option<&str>, token: Option<&str>) -> Option<Project> {
        let id = id.map(str::trim).filter(|id| !id.is_empty())?;
        let token = token.map(str::trim).filter(|token| !token.is_empty())?;
        Some(Project {
            id: id.to_string(),
            token: token.to_string(),
            host: HOST.to_string(),
        })
    }
}

/// Why a daemon sends nothing, whatever the System Setting says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Blocked {
    /// The build holds no PostHog project: a build from source or a
    /// development build.
    Build,
    /// The environment sets `DO_NOT_TRACK`.
    DoNotTrack,
}

/// Where this daemon sends, or why it sends nothing. `do_not_track` is
/// the value of the `DO_NOT_TRACK` variable. Any value but empty, `0` or
/// `false` stops the daemon, as the Console Do Not Track convention asks.
pub fn destination(
    project: Option<Project>,
    do_not_track: Option<&str>,
) -> Result<Project, Blocked> {
    let project = project.ok_or(Blocked::Build)?;
    let do_not_track = do_not_track.map(str::trim).unwrap_or_default();
    if !(do_not_track.is_empty()
        || do_not_track == "0"
        || do_not_track.eq_ignore_ascii_case("false"))
    {
        return Err(Blocked::DoNotTrack);
    }
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project::from_build(Some("12345"), Some("phc_test")).expect("a project")
    }

    #[test]
    fn a_build_holds_a_project_only_with_both_values() {
        assert_eq!(project().id, "12345");
        assert_eq!(project().token, "phc_test");
        assert_eq!(project().host, HOST);
        assert_eq!(Project::from_build(None, None), None);
        assert_eq!(Project::from_build(Some("12345"), None), None);
        assert_eq!(Project::from_build(None, Some("phc_test")), None);
        assert_eq!(Project::from_build(Some(" "), Some("phc_test")), None);
        assert_eq!(Project::from_build(Some("12345"), Some("")), None);
    }

    #[test]
    fn a_build_with_no_project_sends_nothing() {
        assert_eq!(destination(None, None), Err(Blocked::Build));
        assert_eq!(destination(None, Some("1")), Err(Blocked::Build));
    }

    #[test]
    fn do_not_track_stops_a_release_build() {
        for value in ["1", "true", "yes", " 1 "] {
            assert_eq!(
                destination(Some(project()), Some(value)),
                Err(Blocked::DoNotTrack),
                "{value:?}"
            );
        }
        for value in [None, Some(""), Some("0"), Some("false"), Some("FALSE")] {
            assert_eq!(
                destination(Some(project()), value),
                Ok(project()),
                "{value:?}"
            );
        }
    }
}
