//! Host Allow Rules: the rules that the approval card's "Always allow"
//! proposes for a command, and the match check that lets a Host Grant
//! approve a command with no card (ADR-0015).
//!
//! A rule names a command class: the leading words of a command, stored
//! joined with one space (for example `git status` or `echo`). The check
//! parses the command with tree-sitter-bash and does not split it on
//! whitespace. A rule matches only a command that the parser fully
//! understands: a list of simple commands joined by `&&`, `||`, `;`, `|`
//! or a line break. Each word of a simple command is a plain word, a
//! single-quoted string, or a double-quoted string with no expansion.
//! An environment assignment, a redirection, a here-document, an
//! expansion (parameter, arithmetic, brace, tilde or glob), a
//! substitution, a subshell or any other shell syntax puts the command
//! outside this shape, and the command shows the approval card. A
//! simple command matches a rule when the words of the rule are its
//! leading words after quote removal, and the command is allowed only
//! when every simple command matches a rule. A rule accepts every flag
//! and argument after its words, including a flag that writes a file or
//! runs another program; the card states this next to "Always allow".
//!
//! No rule can name a program that runs other programs, because such a
//! rule approves every program. Derivation proposes no such rule, the
//! settings refuse one, and the match check ignores one that is already
//! stored. Derivation also proposes no rule when a flag takes the place
//! of the operation word of a multi-word CLI (`git -C /repo status`). The
//! Client App runs a command that a rule approved under `/bin/sh`, the
//! dialect that the parser checked, and a command that the person
//! approved on its card in the person's own shell.

use tree_sitter::{Node, Parser};

/// The cap on rules one request proposes.
pub const MAX_PROPOSED_RULES: usize = 5;

/// CLIs whose first argument names the operation. A derived rule for
/// them keeps that argument, so `git push` never rides on `git status`.
const MULTI_WORD_CLIS: &[&str] = &[
    "apt",
    "apt-get",
    "brew",
    "cargo",
    "docker",
    "gh",
    "git",
    "go",
    "kubectl",
    "npm",
    "npx",
    "pip",
    "pip3",
    "pnpm",
    "systemctl",
    "yarn",
];

/// Programs and shell builtins that run other programs or shell code,
/// or that change which program a later word runs. A rule for one of
/// them approves every program.
const PROGRAM_RUNNERS: &[&str] = &[
    ".",
    "alias",
    "bash",
    "builtin",
    "caffeinate",
    "chroot",
    "command",
    "coproc",
    "csh",
    "dash",
    "doas",
    "enable",
    "env",
    "eval",
    "exec",
    "find",
    "fish",
    "hash",
    "ionice",
    "ksh",
    "nice",
    "nohup",
    "script",
    "sh",
    "source",
    "stdbuf",
    "su",
    "sudo",
    "tcsh",
    "time",
    "timeout",
    "trap",
    "watch",
    "xargs",
    "zsh",
];

/// The tokens that join the simple commands of an allowed list. A line
/// break also joins two commands; the parser keeps no token for it.
const SEPARATORS: &[&str] = &["&&", "||", ";", "|"];

/// Derive the rules that a request proposes for its command: one rule
/// for each simple command, deduplicated, capped at
/// [`MAX_PROPOSED_RULES`].
///
/// A command that a rule cannot approve proposes no rule, so the card
/// offers a one-time approval only. This is a command outside the parsed
/// shape, a command that starts with a program that runs other programs,
/// and a multi-word CLI with no operation word after the program (for
/// example `git -C /repo status`, where a flag hides the operation).
pub fn derive_rules(command: &str) -> Vec<String> {
    let Some(commands) = simple_commands(command) else {
        return Vec::new();
    };
    let mut rules = Vec::new();
    for words in &commands {
        let Some(rule) = derive_rule(words) else {
            return Vec::new();
        };
        if !rules.contains(&rule) {
            rules.push(rule);
        }
    }
    rules.truncate(MAX_PROPOSED_RULES);
    rules
}

fn derive_rule(words: &[String]) -> Option<String> {
    let program = words.first()?;
    if runs_other_programs(program) {
        return None;
    }
    if !MULTI_WORD_CLIS.contains(&program_name(program)) {
        return Some(program.clone());
    }
    let operation = words.get(1)?;
    let names_the_operation = !operation.is_empty()
        && !operation.starts_with('-')
        && !operation.contains(char::is_whitespace);
    names_the_operation.then(|| format!("{program} {operation}"))
}

/// True when every simple command of `command` matches an Allow Rule.
/// A command outside the parsed shape, an empty command and an empty
/// rule list never match, and a rule for a program that runs other
/// programs matches nothing.
pub fn command_allowed(command: &str, rules: &[String]) -> bool {
    let Some(commands) = simple_commands(command) else {
        return false;
    };
    let rules: Vec<Vec<&str>> = rules
        .iter()
        .filter(|rule| !runs_other_programs(rule))
        .map(|rule| rule.split_whitespace().collect::<Vec<_>>())
        .filter(|words| !words.is_empty())
        .collect();
    !commands.is_empty()
        && commands.iter().all(|words| {
            rules.iter().any(|rule| {
                words.len() >= rule.len() && rule.iter().zip(words).all(|(r, w)| r == w)
            })
        })
}

/// True when the rule names a program that runs other programs. The
/// check reads the file name of the program, so `/usr/bin/env` is `env`.
pub fn runs_other_programs(rule: &str) -> bool {
    rule.split_whitespace()
        .next()
        .is_some_and(|program| PROGRAM_RUNNERS.contains(&program_name(program)))
}

/// The file name of a program word: `/usr/bin/git` is `git`.
fn program_name(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// The simple commands of `command`, each as its words after quote
/// removal, or `None` when the command is not a list of simple commands
/// that the parser fully understands.
fn simple_commands(command: &str) -> Option<Vec<Vec<String>>> {
    // The parser takes some characters as word separators that the
    // shell keeps inside a word, such as a carriage return. A command
    // with one of them could read as one program here and run another.
    if command
        .chars()
        .any(|c| (c.is_control() || c.is_whitespace()) && !matches!(c, ' ' | '\t' | '\n'))
    {
        return None;
    }
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .expect("the bash grammar matches the tree-sitter version");
    let tree = parser.parse(command, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    // An explicit stack, because a long `a && b && ...` nests one list
    // node for each operator.
    let mut stack = vec![root];
    let mut command_nodes = Vec::new();
    let mut cursor = root.walk();
    while let Some(node) = stack.pop() {
        if node.is_missing() {
            return None;
        }
        match node.kind() {
            "program" | "list" | "pipeline" => {
                for child in node.children(&mut cursor) {
                    if child.is_named() {
                        stack.push(child);
                    } else if child.is_missing() || !SEPARATORS.contains(&child.kind()) {
                        return None;
                    }
                }
            }
            "command" => command_nodes.push(node),
            _ => return None,
        }
    }
    command_nodes.sort_by_key(Node::start_byte);
    let source = command.as_bytes();
    command_nodes
        .into_iter()
        .map(|node| command_words(node, source))
        .collect()
}

/// The words of one simple command after quote removal, or `None` when
/// the command holds anything but a program name and plain or quoted
/// words.
fn command_words(command: Node, source: &[u8]) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut cursor = command.walk();
    for child in command.children(&mut cursor) {
        if child.is_missing() {
            return None;
        }
        let word = match child.kind() {
            "command_name" => {
                let name = child.child(0).filter(|_| child.child_count() == 1)?;
                if name.kind() != "word" {
                    return None;
                }
                plain_word(name, source)?
            }
            "word" | "number" => plain_word(child, source)?,
            "string" => double_quoted(child, source)?,
            "raw_string" => single_quoted(child, source)?,
            _ => return None,
        };
        words.push(word);
    }
    Some(words)
}

/// An unquoted word, when every character in it is literal to the
/// shell. A glob character, a tilde, a brace, a backslash or a dollar
/// sign makes the shell change the word, so the word is refused.
fn plain_word(node: Node, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    let literal = !text.is_empty()
        && text
            .chars()
            .all(|c| !c.is_ascii() || c.is_ascii_alphanumeric() || "%+,-./:=@_".contains(c));
    literal.then(|| text.to_string())
}

/// A double-quoted string with no expansion and no escape. A dollar
/// sign, a backquote or a backslash inside it is refused.
fn double_quoted(node: Node, source: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    let children: Vec<Node> = node.children(&mut cursor).collect();
    let (first, rest) = children.split_first()?;
    let (last, content) = rest.split_last()?;
    if first.kind() != "\"" || last.kind() != "\"" || last.is_missing() {
        return None;
    }
    let mut text = String::new();
    for part in content {
        if part.kind() != "string_content" {
            return None;
        }
        text.push_str(part.utf8_text(source).ok()?);
    }
    (!text.contains(['$', '`', '\\'])).then_some(text)
}

/// A single-quoted string. The shell takes everything inside it
/// literally.
fn single_quoted(node: Node, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    text.strip_prefix('\'')?
        .strip_suffix('\'')
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(list: &[&str]) -> Vec<String> {
        list.iter().map(|r| r.to_string()).collect()
    }

    fn words(list: &[&[&str]]) -> Option<Vec<Vec<String>>> {
        Some(list.iter().map(|command| rules(command)).collect())
    }

    #[test]
    fn a_list_parses_into_the_words_of_each_simple_command() {
        assert_eq!(
            simple_commands("git add . && git commit || echo fail; ls | wc -l\ntrue"),
            words(&[
                &["git", "add", "."],
                &["git", "commit"],
                &["echo", "fail"],
                &["ls"],
                &["wc", "-l"],
                &["true"],
            ])
        );
    }

    #[test]
    fn quote_removal_gives_the_words_the_program_receives() {
        assert_eq!(
            simple_commands(r#"echo "a && b" 'c; $d' "" head -n 5"#),
            words(&[&["echo", "a && b", "c; $d", "", "head", "-n", "5"]])
        );
    }

    #[test]
    fn derive_keeps_the_subcommand_for_known_clis() {
        assert_eq!(derive_rules("git status --short"), rules(&["git status"]));
        assert_eq!(derive_rules("npm run build"), rules(&["npm run"]));
        assert_eq!(derive_rules("ls -la /tmp"), rules(&["ls"]));
        assert_eq!(
            derive_rules("/usr/bin/git status"),
            rules(&["/usr/bin/git status"])
        );
    }

    #[test]
    fn derive_dedupes_and_caps_at_five() {
        assert_eq!(
            derive_rules("git add . && git add -A && git commit -m x"),
            rules(&["git add", "git commit"])
        );
        assert_eq!(
            derive_rules("a; b; c; d; e; f; g").len(),
            MAX_PROPOSED_RULES
        );
    }

    #[test]
    fn derive_of_an_empty_command_is_empty() {
        assert_eq!(derive_rules("  "), Vec::<String>::new());
    }

    #[test]
    fn derive_proposes_no_rule_that_hides_the_operation_or_runs_other_programs() {
        assert_eq!(derive_rules("git -C /repo status"), Vec::<String>::new());
        assert_eq!(derive_rules("env git status"), Vec::<String>::new());
        assert_eq!(derive_rules("xargs rm"), Vec::<String>::new());
        assert_eq!(derive_rules("find . -exec rm {} ;"), Vec::<String>::new());
    }

    /// The bare program of a multi-word CLI covers every operation, and
    /// a path does not hide the program from the check.
    #[test]
    fn derive_proposes_no_bare_multi_word_cli_and_reads_the_program_behind_a_path() {
        assert_eq!(derive_rules("git"), Vec::<String>::new());
        assert_eq!(derive_rules("git ''"), Vec::<String>::new());
        assert_eq!(
            derive_rules("/usr/bin/git -C . status"),
            Vec::<String>::new()
        );
        assert_eq!(
            derive_rules("/usr/bin/env git status"),
            Vec::<String>::new()
        );
        assert_eq!(derive_rules("sudo git status"), Vec::<String>::new());
    }

    /// A rule that would not approve the same command again is not
    /// offered, so "Always allow" never promises what it cannot do.
    #[test]
    fn derive_proposes_no_rule_for_a_command_that_no_rule_can_approve() {
        assert_eq!(
            derive_rules("RUST_LOG=debug cargo test"),
            Vec::<String>::new()
        );
        assert_eq!(derive_rules("echo x > f"), Vec::<String>::new());
        assert_eq!(
            derive_rules("git status && git -C . push"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_rule_matches_its_command_class() {
        assert!(command_allowed(
            "git status --short",
            &rules(&["git status"])
        ));
        assert!(command_allowed("echo hi there", &rules(&["echo"])));
        assert!(command_allowed(r#"git "status""#, &rules(&["git status"])));
        assert!(command_allowed("echo 'a > b'", &rules(&["echo"])));
        assert!(!command_allowed("git push origin", &rules(&["git status"])));
        assert!(!command_allowed("git", &rules(&["git status"])));
        assert!(!command_allowed(
            "/usr/bin/git status",
            &rules(&["git status"])
        ));
        assert!(!command_allowed("echo hi", &rules(&[""])));
    }

    /// An environment assignment changes what the approved program
    /// does: `GIT_CONFIG_*` can name a program that git runs, and `PATH`
    /// can put another program in the place of git.
    #[test]
    fn an_environment_assignment_never_auto_approves() {
        let allow = rules(&["git status"]);

        assert!(!command_allowed("FOO=1 git status", &allow));
        assert!(!command_allowed(
            "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.fsmonitor GIT_CONFIG_VALUE_0=x git status",
            &allow
        ));
        assert!(!command_allowed("PATH=/tmp:$PATH git status", &allow));
    }

    #[test]
    fn a_redirection_or_here_document_never_auto_approves() {
        assert!(!command_allowed("git status > f", &rules(&["git status"])));
        assert!(!command_allowed(
            "git status <<EOF",
            &rules(&["git status"])
        ));

        let allow = rules(&["echo"]);
        assert!(!command_allowed("echo x > f", &allow));
        assert!(!command_allowed("echo x >> ~/.zshrc", &allow));
        assert!(!command_allowed("echo x <<< y", &allow));
        assert!(!command_allowed("echo x 2>&1", &allow));
    }

    #[test]
    fn a_parameter_expansion_never_auto_approves() {
        let allow = rules(&["echo"]);

        assert!(!command_allowed("echo $HOME", &allow));
        assert!(!command_allowed("echo ${IFS}", &allow));
        assert!(!command_allowed(r#"echo "$HOME""#, &allow));
        assert!(!command_allowed(r#"echo "a$""#, &allow));
    }

    #[test]
    fn an_arithmetic_brace_tilde_or_glob_expansion_never_auto_approves() {
        let allow = rules(&["echo", "ls"]);

        assert!(!command_allowed("echo $((1 + 2))", &allow));
        assert!(!command_allowed("echo {a,b}", &allow));
        assert!(!command_allowed("echo a{b,c}", &allow));
        assert!(!command_allowed("ls ~/x", &allow));
        assert!(!command_allowed("ls *.rs", &allow));
        assert!(!command_allowed("ls a?b", &allow));
        assert!(!command_allowed("ls [ab]", &allow));
    }

    #[test]
    fn substitution_never_auto_approves() {
        let allow = rules(&["echo", "git status"]);

        assert!(!command_allowed("echo $(rm -rf /)", &allow));
        assert!(!command_allowed(r#"echo "$(id)""#, &allow));
        assert!(!command_allowed("echo `date`", &allow));
        assert!(!command_allowed("echo <(date)", &allow));
        assert!(!command_allowed("(git status)", &allow));
        assert!(!command_allowed("{ git status; }", &allow));
    }

    /// An escape, an ANSI-C string, a backgrounded command, a comment and
    /// a negation are shell syntax outside the allowed shape. So is a
    /// command that does not parse.
    #[test]
    fn other_shell_syntax_never_auto_approves() {
        let allow = rules(&["echo", "git status", "cat"]);

        assert!(!command_allowed(r"echo a\ b", &allow));
        assert!(!command_allowed(r#"echo "a\"b""#, &allow));
        assert!(!command_allowed("echo $'x'", &allow));
        assert!(!command_allowed("echo --a=\"b c\"", &allow));
        assert!(!command_allowed("git status &", &allow));
        assert!(!command_allowed("git status |& cat", &allow));
        assert!(!command_allowed("! git status", &allow));
        assert!(!command_allowed("echo x # y", &allow));
        assert!(!command_allowed("echo \"", &allow));
        assert!(!command_allowed("git status\r", &allow));
        assert!(!command_allowed("echo\u{a0}x", &allow));
    }

    /// A rule for a program that runs other programs approves every
    /// program. A rule that is already stored does not match either.
    #[test]
    fn a_stored_rule_for_a_program_that_runs_other_programs_never_matches() {
        assert!(!command_allowed("env git status", &rules(&["env"])));
        assert!(!command_allowed("bash -c x", &rules(&["bash"])));
        assert!(!command_allowed("xargs rm", &rules(&["xargs"])));
        assert!(!command_allowed(
            "/usr/bin/env git status",
            &rules(&["/usr/bin/env"])
        ));
    }

    #[test]
    fn a_rule_that_names_a_program_runner_is_recognized() {
        for rule in ["env", "/usr/bin/env", "bash -c", "sudo", ".", "xargs rm"] {
            assert!(runs_other_programs(rule), "{rule}");
        }
        for rule in ["git status", "echo", "environment", ""] {
            assert!(!runs_other_programs(rule), "{rule}");
        }
    }

    #[test]
    fn a_plain_list_of_simple_commands_auto_approves() {
        assert!(command_allowed(
            "git status --short",
            &rules(&["git status"])
        ));
        assert!(command_allowed(
            "git status && echo done",
            &rules(&["git status", "echo"])
        ));
        assert!(command_allowed("ls -la /tmp", &rules(&["ls"])));
    }

    #[test]
    fn every_simple_command_must_match_for_the_command_to_be_allowed() {
        let allow = rules(&["git status", "echo"]);

        assert!(command_allowed("git status && echo done", &allow));
        assert!(command_allowed(
            "git status | echo done\necho again;",
            &allow
        ));
        assert!(!command_allowed("git status && rm -rf /", &allow));
        assert!(!command_allowed("git push", &allow));
        assert!(!command_allowed("", &allow));
        assert!(!command_allowed("echo hi", &[]));
    }
}
