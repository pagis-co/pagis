//! Pagis policy for a Harness Permission (ADR-0033).
//!
//! [`evaluate`] is a pure function. The caller reads the live host Grant
//! and the Coding Session, and gives their values. The function reads
//! nothing.
//!
//! The scope check is lexical, because the files are on the Host and
//! the daemon cannot read them. A symbolic link inside the directory
//! that points outside it counts as inside, as in the `acceptEdits` mode
//! of Claude Code and the `workspace-write` mode of Codex. The rule step
//! uses the Host Allow Rules of `host_shell` and their matcher
//! ([`command_allowed`]).

use pagis_core::{SessionApprovalMode, path_is_inside};
use serde::{Deserialize, Serialize};

use crate::command_allowed;

/// The kind of the tool call of a Harness Permission: the ten ACP tool
/// kinds. An unknown or absent kind is `Other`, as ACP reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessToolKind {
    Read,
    Edit,
    Delete,
    Move,
    Search,
    Execute,
    Think,
    Fetch,
    SwitchMode,
    Other,
}

impl HarnessToolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            HarnessToolKind::Read => "read",
            HarnessToolKind::Edit => "edit",
            HarnessToolKind::Delete => "delete",
            HarnessToolKind::Move => "move",
            HarnessToolKind::Search => "search",
            HarnessToolKind::Execute => "execute",
            HarnessToolKind::Think => "think",
            HarnessToolKind::Fetch => "fetch",
            HarnessToolKind::SwitchMode => "switch_mode",
            HarnessToolKind::Other => "other",
        }
    }

    /// The kinds that the scope step can allow: they touch files only.
    fn touches_files_only(self) -> bool {
        matches!(
            self,
            HarnessToolKind::Read
                | HarnessToolKind::Search
                | HarnessToolKind::Think
                | HarnessToolKind::Edit
                | HarnessToolKind::Delete
                | HarnessToolKind::Move
        )
    }
}

/// Who decided a Harness Permission. It is the `decider` of the audit
/// fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decider {
    /// The `auto` mode of the session.
    Auto,
    /// Every location is inside the session's directory.
    Scope,
    /// A Host Allow Rule matches the command.
    Rule,
    /// The supervising Agent.
    Agent,
    /// The Person.
    Person,
}

impl Decider {
    pub fn as_str(self) -> &'static str {
        match self {
            Decider::Auto => "auto",
            Decider::Scope => "scope",
            Decider::Rule => "rule",
            Decider::Agent => "agent",
            Decider::Person => "person",
        }
    }
}

/// What Pagis policy does with a Harness Permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyOutcome {
    /// Pagis allows the request once.
    Allow(Decider),
    /// The request waits for the supervising Agent.
    AskAgent,
    /// The request waits for the Person.
    AskPerson,
}

/// Applies Pagis policy to one Harness Permission, and stops at the
/// first step that answers:
///
/// 1. The `auto` mode allows.
/// 2. A `read`, `search`, `think`, `edit`, `delete` or `move` with every
///    location inside `directory` allows. A request with no location
///    passes this step only for `think`, which touches no file.
/// 3. An `execute` whose command the Host Allow Rules accept allows.
/// 4. The `agent` mode asks the Agent.
/// 5. The `person` mode asks the Person.
///
/// `mode` is the effective Session Approval Mode: the narrower of the
/// mode of the session and the widest mode of the live host Grant.
/// `directory` is the directory that the harness runs in. The paths are
/// POSIX paths, because the Client App runs on macOS and Linux only.
pub fn evaluate(
    mode: SessionApprovalMode,
    kind: HarnessToolKind,
    locations: &[String],
    command: Option<&str>,
    directory: &str,
    allow_rules: &[String],
) -> PolicyOutcome {
    if mode == SessionApprovalMode::Auto {
        return PolicyOutcome::Allow(Decider::Auto);
    }
    if kind.touches_files_only() {
        let inside = if locations.is_empty() {
            kind == HarnessToolKind::Think
        } else {
            locations
                .iter()
                .all(|location| path_is_inside(location, directory))
        };
        if inside {
            return PolicyOutcome::Allow(Decider::Scope);
        }
    }
    if kind == HarnessToolKind::Execute
        && command.is_some_and(|command| command_allowed(command, allow_rules))
    {
        return PolicyOutcome::Allow(Decider::Rule);
    }
    match mode {
        SessionApprovalMode::Agent => PolicyOutcome::AskAgent,
        _ => PolicyOutcome::AskPerson,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HarnessToolKind as K;
    use PolicyOutcome::{Allow, AskAgent, AskPerson};
    use SessionApprovalMode::{Agent, Auto, Person};

    const DIRECTORY: &str = "/repo";

    struct Row {
        name: &'static str,
        mode: SessionApprovalMode,
        kind: HarnessToolKind,
        locations: &'static [&'static str],
        command: Option<&'static str>,
        rules: &'static [&'static str],
        outcome: PolicyOutcome,
    }

    const fn row(
        name: &'static str,
        mode: SessionApprovalMode,
        kind: HarnessToolKind,
        locations: &'static [&'static str],
        command: Option<&'static str>,
        rules: &'static [&'static str],
        outcome: PolicyOutcome,
    ) -> Row {
        Row {
            name,
            mode,
            kind,
            locations,
            command,
            rules,
            outcome,
        }
    }

    const INSIDE: &[&str] = &["/repo/src/main.rs"];
    const OUTSIDE: &[&str] = &["/etc/passwd"];
    const NONE: &[&str] = &[];
    const GIT_STATUS: &[&str] = &["git status"];

    #[rustfmt::skip]
    const ROWS: &[Row] = &[
        // Each mode.
        row("auto allows outside the scope", Auto, K::Execute, OUTSIDE, Some("rm -rf /"), NONE, Allow(Decider::Auto)),
        row("auto allows an other", Auto, K::Other, NONE, None, NONE, Allow(Decider::Auto)),
        row("agent asks the agent", Agent, K::Execute, NONE, Some("rm -rf /"), NONE, AskAgent),
        row("person asks the person", Person, K::Execute, NONE, Some("rm -rf /"), NONE, AskPerson),
        // Each tool kind, inside the directory.
        row("read inside", Person, K::Read, INSIDE, None, NONE, Allow(Decider::Scope)),
        row("edit inside", Person, K::Edit, INSIDE, None, NONE, Allow(Decider::Scope)),
        row("delete inside", Person, K::Delete, INSIDE, None, NONE, Allow(Decider::Scope)),
        row("move inside", Person, K::Move, &["/repo/a.rs", "/repo/b/a.rs"], None, NONE, Allow(Decider::Scope)),
        row("search inside", Person, K::Search, INSIDE, None, NONE, Allow(Decider::Scope)),
        row("think inside", Person, K::Think, INSIDE, None, NONE, Allow(Decider::Scope)),
        row("execute inside is no scope", Person, K::Execute, INSIDE, None, NONE, AskPerson),
        row("fetch inside is no scope", Person, K::Fetch, INSIDE, None, NONE, AskPerson),
        row("switch_mode inside is no scope", Person, K::SwitchMode, INSIDE, None, NONE, AskPerson),
        row("other inside is no scope", Agent, K::Other, INSIDE, None, NONE, AskAgent),
        // The directory itself.
        row("the directory itself", Person, K::Search, &["/repo"], None, NONE, Allow(Decider::Scope)),
        row("the directory with a slash", Person, K::Search, &["/repo/"], None, NONE, Allow(Decider::Scope)),
        // Outside the directory.
        row("read outside", Person, K::Read, OUTSIDE, None, NONE, AskPerson),
        row("one location outside", Person, K::Move, &["/repo/a.rs", "/tmp/a.rs"], None, NONE, AskPerson),
        row("a .. that leaves", Person, K::Edit, &["/repo/../etc/passwd"], None, NONE, AskPerson),
        row("a .. that stays", Person, K::Edit, &["/repo/src/../main.rs"], None, NONE, Allow(Decider::Scope)),
        row("a . inside", Person, K::Edit, &["/repo/./src/main.rs"], None, NONE, Allow(Decider::Scope)),
        row("a sibling with the same prefix", Person, K::Edit, &["/repo-other/main.rs"], None, NONE, AskPerson),
        row("a relative path", Person, K::Edit, &["src/main.rs"], None, NONE, AskPerson),
        row("a .. above the root", Person, K::Read, &["/../../repo/a.rs"], None, NONE, Allow(Decider::Scope)),
        // No location.
        row("think with no location", Person, K::Think, NONE, None, NONE, Allow(Decider::Scope)),
        row("read with no location", Person, K::Read, NONE, None, NONE, AskPerson),
        row("edit with no location", Agent, K::Edit, NONE, None, NONE, AskAgent),
        // The rule step.
        row("execute with a matching rule", Person, K::Execute, NONE, Some("git status --short"), GIT_STATUS, Allow(Decider::Rule)),
        row("execute with no matching rule", Person, K::Execute, NONE, Some("git push"), GIT_STATUS, AskPerson),
        row("execute with no rule", Agent, K::Execute, NONE, Some("git status"), NONE, AskAgent),
        row("execute with no command", Person, K::Execute, NONE, None, GIT_STATUS, AskPerson),
        row("execute with shell syntax", Person, K::Execute, NONE, Some("git status > /etc/motd"), GIT_STATUS, AskPerson),
        row("execute with a substitution", Person, K::Execute, NONE, Some("git status $(rm -rf /)"), GIT_STATUS, AskPerson),
        row("a stored rule for sh", Person, K::Execute, NONE, Some("sh -c 'rm -rf /'"), &["sh"], AskPerson),
        row("a rule is no rule for an edit", Person, K::Edit, OUTSIDE, Some("git status"), GIT_STATUS, AskPerson),
    ];

    #[test]
    fn evaluate_applies_the_steps_in_order() {
        for row in ROWS {
            let locations: Vec<String> =
                row.locations.iter().map(|path| path.to_string()).collect();
            let rules: Vec<String> = row.rules.iter().map(|rule| rule.to_string()).collect();
            let outcome = evaluate(
                row.mode,
                row.kind,
                &locations,
                row.command,
                DIRECTORY,
                &rules,
            );
            assert_eq!(outcome, row.outcome, "{}", row.name);
        }
    }

    #[test]
    fn a_relative_directory_holds_nothing_and_the_root_holds_everything() {
        let inside = vec!["/repo/a.rs".to_string()];
        assert_eq!(evaluate(Person, K::Edit, &inside, None, "", &[]), AskPerson);
        assert_eq!(
            evaluate(Person, K::Edit, &inside, None, "repo", &[]),
            AskPerson
        );
        assert_eq!(
            evaluate(Person, K::Edit, &inside, None, "/", &[]),
            Allow(Decider::Scope)
        );
        assert_eq!(
            evaluate(Person, K::Edit, &inside, None, "/repo/./", &[]),
            Allow(Decider::Scope)
        );
    }
}
