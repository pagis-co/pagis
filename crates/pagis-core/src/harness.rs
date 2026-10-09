//! The Harness Catalog (ADR-0033): the one list of Coding Harnesses that
//! Pagis starts.
//!
//! Each entry names a harness, its pinned version, the command that
//! starts its ACP agent, the programs that command needs on the Person's
//! `PATH`, whether the harness asks permission, and the vendor's own
//! sign-in for each sign-in method.
//!
//! The catalog ships with the release, as the Provider Catalog does
//! (ADR-0012). Pagis does not fetch the ACP registry at run time, and the
//! client holds no list. A release moves a pin by editing this file, so a
//! new harness release changes nothing until a Pagis release pins it.
//!
//! An npx entry is pinned exactly by `<package>@<version>`. A binary
//! entry on a Host runs the Person's own installed program. Its version
//! is the version that the release is tested with, and its archives pin
//! the build of each platform at that version with their SHA-256.
//!
//! A harness that a Harness Model Endpoint can serve also has a Computer
//! launch: the Computer Image ships it at the same pins
//! (`computer/harnesses/package.json` and `ARG OPENCODE_VERSION` in
//! `computer/Dockerfile`), and the launch runs the installed program.

/// One Coding Harness that Pagis starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessEntry {
    /// The id a Coding Session records, and the `<id>` of the Host
    /// capability `harness:<id>`.
    pub id: &'static str,
    /// The name a person reads.
    pub label: &'static str,
    /// The id of the entry in the ACP registry.
    pub registry_id: &'static str,
    /// The exact version that the release pins.
    pub version: &'static str,
    pub launch: Launch,
    /// The programs that must be on the Person's `PATH` besides the
    /// launcher.
    pub requires: &'static [&'static str],
    /// Whether the harness sends `session/request_permission` before a
    /// tool acts. A harness that never asks works in an Unattended Mode,
    /// so on a Host it starts only where the host Grant allows one.
    pub asks_permission: bool,
    /// One Harness Sign-In for each sign-in method that the harness has.
    pub sign_in: &'static [SignIn],
    /// How the harness starts in the Agent's Computer, or `None` for a
    /// harness that does not run there.
    pub computer: Option<ComputerLaunch>,
}

/// How a harness starts in the Agent's Computer. The Computer Image
/// installs the harness, so a launch never names `npx` and a session
/// downloads nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComputerLaunch {
    /// The npm packages that the Computer Image installs from
    /// `computer/harnesses/package.json`, as `<name>@<version>`.
    pub packages: &'static [&'static str],
    /// The program on the `PATH` of the Computer.
    pub program: &'static str,
    pub args: &'static [&'static str],
}

/// How a harness starts, from its distribution in the ACP registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Launch {
    /// `npx` runs the npm package. `package` is `<name>@<version>`.
    Npx {
        package: &'static str,
        args: &'static [&'static str],
    },
    /// A native program. `program` is its name on the Person's `PATH`.
    Binary {
        program: &'static str,
        args: &'static [&'static str],
        archives: &'static [Archive],
    },
}

/// The release archive of a binary harness for one platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Archive {
    /// The platform as the ACP registry names it, for example
    /// `darwin-aarch64`.
    pub platform: &'static str,
    pub url: &'static str,
    /// The SHA-256 of the archive, in lowercase hexadecimal.
    pub sha256: &'static str,
    /// The program inside the archive, relative to its root.
    pub cmd: &'static str,
}

/// What the Person signs in with.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
// The sign-in of a Person to Pagis has a `SignInMethod` schema of its own.
#[schema(as = HarnessSignInMethod)]
pub enum SignInMethod {
    /// The Person's subscription or account at the vendor.
    Subscription,
    /// An API key of the vendor.
    ApiKey,
}

impl SignInMethod {
    /// The name of the method on the wire: `subscription` or `api_key`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            SignInMethod::Subscription => "subscription",
            SignInMethod::ApiKey => "api_key",
        }
    }

    /// The name of the method that a person reads.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SignInMethod::Subscription => "Subscription",
            SignInMethod::ApiKey => "API key",
        }
    }
}

/// What the Client App runs in a terminal window for a Harness Sign-In.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignInAction {
    /// The ACP terminal auth method with this id, which the harness's
    /// `initialize` gives. The Client App runs the launch command with
    /// the method's `args` and `env`.
    TerminalAuth(&'static str),
    /// The vendor's own sign-in command, as an argument vector.
    Command(&'static [&'static str]),
}

/// One Harness Sign-In.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignIn {
    pub method: SignInMethod,
    pub how: SignInAction,
}

/// The Codex CLI that the sign-in runs: the version that the dependency
/// `@openai/codex` `^0.159.1` of the npm package
/// `@agentclientprotocol/codex-acp` 2.1.1 names.
const CODEX_CLI: &str = "@openai/codex@0.159.1";

/// The ACP adapters of Claude Code, Codex and pi, which a Host runs with
/// `npx` and the Computer Image installs.
const CLAUDE_ACP: &str = "@agentclientprotocol/claude-agent-acp@0.87.0";
const CODEX_ACP: &str = "@agentclientprotocol/codex-acp@2.1.1";
const PI_ACP: &str = "pi-acp@0.0.34";

/// The pi program that `pi-acp` runs in a Computer. A Host runs the
/// Person's own `pi`, so the catalog pins it only for the Computer
/// Image. pi 1.0.1 and later ship no `npm-shrinkwrap.json`: npm installs
/// the optional platform packages of a shrinkwrap for every platform,
/// and the shrinkwrap of pi 0.87 gives no `integrity` for the pi
/// packages in it.
const PI_CLI: &str = "@earendil-works/pi-coding-agent@1.0.4";

/// The Gemini CLI, from the `gemini` entry of the ACP registry. The npm
/// package is the vendor program itself.
const GEMINI_CLI: &str = "@google/gemini-cli@0.63.0";

/// The GitHub Copilot CLI, from the `github-copilot-cli` entry of the
/// ACP registry. The npm package is the vendor program itself, and it
/// brings a native binary for each platform.
const COPILOT_CLI: &str = "@github/copilot@1.0.93";

/// Claude Code, from the `claude-acp` entry of the ACP registry. The npm
/// package `@agentclientprotocol/claude-agent-acp` 0.87.0 depends on
/// `@anthropic-ai/claude-agent-sdk` 0.3.287, which brings the Claude Code
/// binary, so the Person installs no `claude`. Its `initialize` gives the
/// terminal methods `claude-ai-login` ("Use Claude subscription") and
/// `console-login` ("Anthropic Console (API usage billing)"), which run
/// `claude auth login` (code.claude.com/docs/en/cli-reference).
const CLAUDE: HarnessEntry = HarnessEntry {
    id: "claude",
    label: "Claude Code",
    registry_id: "claude-acp",
    version: "0.87.0",
    launch: Launch::Npx {
        package: CLAUDE_ACP,
        args: &[],
    },
    requires: &[],
    asks_permission: true,
    sign_in: &[
        SignIn {
            method: SignInMethod::Subscription,
            how: SignInAction::TerminalAuth("claude-ai-login"),
        },
        SignIn {
            method: SignInMethod::ApiKey,
            how: SignInAction::TerminalAuth("console-login"),
        },
    ],
    computer: Some(ComputerLaunch {
        packages: &[CLAUDE_ACP],
        program: "claude-agent-acp",
        args: &[],
    }),
};

/// Codex, from the `codex-acp` entry of the ACP registry. The npm package
/// `@agentclientprotocol/codex-acp` starts `codex app-server` and brings
/// the Codex binary. Its `initialize` gives no terminal method, so the
/// sign-in runs `codex login` (learn.chatgpt.com/docs/auth). `codex login
/// --with-api-key` reads the key from stdin: the Person pastes it in the
/// terminal window and ends the input with Ctrl-D. The credential goes to
/// `~/.codex/auth.json` or the system credential store, which the Codex of
/// the adapter reads too.
const CODEX: HarnessEntry = HarnessEntry {
    id: "codex",
    label: "Codex",
    registry_id: "codex-acp",
    version: "2.1.1",
    launch: Launch::Npx {
        package: CODEX_ACP,
        args: &[],
    },
    requires: &[],
    asks_permission: true,
    sign_in: &[
        SignIn {
            method: SignInMethod::Subscription,
            how: SignInAction::Command(&["npx", "--yes", CODEX_CLI, "login"]),
        },
        SignIn {
            method: SignInMethod::ApiKey,
            how: SignInAction::Command(&["npx", "--yes", CODEX_CLI, "login", "--with-api-key"]),
        },
    ],
    computer: Some(ComputerLaunch {
        packages: &[CODEX_ACP],
        program: "codex-acp",
        args: &[],
    }),
};

/// The OpenCode release archives, with their SHA-256, from the `opencode`
/// entry of the ACP registry.
const OPENCODE_ARCHIVES: &[Archive] = &[
    Archive {
        platform: "darwin-aarch64",
        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.35/opencode-darwin-arm64.zip",
        sha256: "80b05124357a77cd57945bfde36082a028e829c198d222d5e146617f49a2c4b7",
        cmd: "./opencode",
    },
    Archive {
        platform: "darwin-x86_64",
        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.35/opencode-darwin-x64.zip",
        sha256: "8127d69e8e94d7adc496e910435f2f73856d87d456e988d3a947f250c95c1be2",
        cmd: "./opencode",
    },
    Archive {
        platform: "linux-aarch64",
        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.35/opencode-linux-arm64.tar.gz",
        sha256: "f7f2ba59ee8aa94d388f9696575a32d20e71c2ee48def9f80fc693a60fec6c72",
        cmd: "./opencode",
    },
    Archive {
        platform: "linux-x86_64",
        url: "https://github.com/anomalyco/opencode/releases/download/v1.18.35/opencode-linux-x64.tar.gz",
        sha256: "c8f888b451f5494a18f858fffb0e0b68f4e4baa9c241761c5f206884f0fa640d",
        cmd: "./opencode",
    },
];

/// OpenCode, from the `opencode` entry of the ACP registry. Its ACP auth
/// method `opencode-login` only names a command, so the sign-in runs
/// `opencode auth login`, which signs in to every provider with OAuth or
/// an API key and keeps the credential in
/// `~/.local/share/opencode/auth.json` (opencode.ai/docs/cli).
const OPENCODE: HarnessEntry = HarnessEntry {
    id: "opencode",
    label: "OpenCode",
    registry_id: "opencode",
    version: "1.18.35",
    launch: Launch::Binary {
        program: "opencode",
        args: &["acp"],
        archives: OPENCODE_ARCHIVES,
    },
    requires: &[],
    asks_permission: true,
    sign_in: &[
        SignIn {
            method: SignInMethod::Subscription,
            how: SignInAction::Command(&["opencode", "auth", "login"]),
        },
        SignIn {
            method: SignInMethod::ApiKey,
            how: SignInAction::Command(&["opencode", "auth", "login"]),
        },
    ],
    // The Computer Image downloads the release archive of its
    // architecture, so the launch installs no npm package.
    computer: Some(ComputerLaunch {
        packages: &[],
        program: "opencode",
        args: &["acp"],
    }),
};

/// pi, from the `pi-acp` entry of the ACP registry. The npm package
/// `pi-acp` runs the `pi` program, which the Person installs with
/// `npm install -g @earendil-works/pi-coding-agent` (pi 0.81.0 or later).
/// Its `initialize` gives the terminal method `pi_terminal_login`, which
/// starts `pi`, and there the Person types `/login` for a subscription or
/// an API key (pi README). pi has no permission prompt for its tools:
/// `pi-acp` sends `session/request_permission` only to confirm a pi
/// extension.
const PI: HarnessEntry = HarnessEntry {
    id: "pi",
    label: "pi",
    registry_id: "pi-acp",
    version: "0.0.34",
    launch: Launch::Npx {
        package: PI_ACP,
        args: &[],
    },
    requires: &["pi"],
    asks_permission: false,
    sign_in: &[
        SignIn {
            method: SignInMethod::Subscription,
            how: SignInAction::TerminalAuth("pi_terminal_login"),
        },
        SignIn {
            method: SignInMethod::ApiKey,
            how: SignInAction::TerminalAuth("pi_terminal_login"),
        },
    ],
    computer: Some(ComputerLaunch {
        packages: &[PI_ACP, PI_CLI],
        program: "pi-acp",
        args: &[],
    }),
};

/// Gemini CLI, from the `gemini` entry of the ACP registry. The sign-in
/// starts `gemini`, whose auth dialog offers "Sign in with Google" and
/// "Use Gemini API Key" and keeps the key in the system keychain
/// (`docs/get-started/authentication.mdx` at tag v0.63.0).
const GEMINI: HarnessEntry = HarnessEntry {
    id: "gemini",
    label: "Gemini CLI",
    registry_id: "gemini",
    version: "0.63.0",
    launch: Launch::Npx {
        package: GEMINI_CLI,
        args: &["--acp"],
    },
    requires: &[],
    asks_permission: true,
    sign_in: &[
        SignIn {
            method: SignInMethod::Subscription,
            how: SignInAction::Command(&["npx", "--yes", GEMINI_CLI]),
        },
        SignIn {
            method: SignInMethod::ApiKey,
            how: SignInAction::Command(&["npx", "--yes", GEMINI_CLI]),
        },
    ],
    computer: None,
};

/// GitHub Copilot CLI, from the `github-copilot-cli` entry of the ACP
/// registry. The sign-in runs `copilot login` (docs.github.com,
/// "Authenticating GitHub Copilot CLI"). A personal access token works
/// only through the environment, and Pagis puts no secret there, so the
/// harness has no API-key sign-in in Pagis.
const COPILOT: HarnessEntry = HarnessEntry {
    id: "copilot",
    label: "GitHub Copilot CLI",
    registry_id: "github-copilot-cli",
    version: "1.0.93",
    launch: Launch::Npx {
        package: COPILOT_CLI,
        args: &["--acp"],
    },
    requires: &[],
    asks_permission: true,
    sign_in: &[SignIn {
        method: SignInMethod::Subscription,
        how: SignInAction::Command(&["npx", "--yes", COPILOT_CLI, "login"]),
    }],
    computer: None,
};

/// The Cursor CLI release archives, from the `cursor` entry of the ACP
/// registry. The registry gives no SHA-256 for them: each one is the
/// output of `shasum -a 256` on the archive at that URL.
const CURSOR_ARCHIVES: &[Archive] = &[
    Archive {
        platform: "darwin-aarch64",
        url: "https://downloads.cursor.com/lab/2026.10.01-14929f9/darwin/arm64/agent-cli-package.tar.gz",
        sha256: "778d04e542adc5c8b6760fda3ebe0757f903b1764f2792c232ef9a35e6e2151b",
        cmd: "./dist-package/cursor-agent",
    },
    Archive {
        platform: "darwin-x86_64",
        url: "https://downloads.cursor.com/lab/2026.10.01-14929f9/darwin/x64/agent-cli-package.tar.gz",
        sha256: "8930008f9902a4d02d3185c0d34071e0536bac3426439b55bcfd48b78765a3dd",
        cmd: "./dist-package/cursor-agent",
    },
    Archive {
        platform: "linux-aarch64",
        url: "https://downloads.cursor.com/lab/2026.10.01-14929f9/linux/arm64/agent-cli-package.tar.gz",
        sha256: "c31ef0ba6b827fdf8053919de57abae4a7bd71afefdf2cab061e276faac42b3a",
        cmd: "./dist-package/cursor-agent",
    },
    Archive {
        platform: "linux-x86_64",
        url: "https://downloads.cursor.com/lab/2026.10.01-14929f9/linux/x64/agent-cli-package.tar.gz",
        sha256: "ba9a855f8f813c91b9f2707127572d2dc9ae5a62818e1c36719625d0fb8bd452",
        cmd: "./dist-package/cursor-agent",
    },
];

/// Cursor CLI, from the `cursor` entry of the ACP registry. The installer
/// links the same binary as `agent` and as `cursor-agent`, and the
/// registry runs `cursor-agent`. The sign-in runs `cursor-agent login`
/// (cursor.com/docs/cli/reference/authentication). An API key works only
/// through the environment or a flag, and Pagis puts no secret in either,
/// so the harness has no API-key sign-in in Pagis.
const CURSOR: HarnessEntry = HarnessEntry {
    id: "cursor",
    label: "Cursor CLI",
    registry_id: "cursor",
    version: "2026.10.01",
    launch: Launch::Binary {
        program: "cursor-agent",
        args: &["acp"],
        archives: CURSOR_ARCHIVES,
    },
    requires: &[],
    asks_permission: true,
    sign_in: &[SignIn {
        method: SignInMethod::Subscription,
        how: SignInAction::Command(&["cursor-agent", "login"]),
    }],
    computer: None,
};

const CATALOG: &[HarnessEntry] = &[CLAUDE, CODEX, OPENCODE, PI, GEMINI, COPILOT, CURSOR];

/// Every Coding Harness Pagis starts, in the order a list shows them.
pub fn catalog() -> &'static [HarnessEntry] {
    CATALOG
}

/// The entry of one harness, by the id a Coding Session records.
pub fn entry(id: &str) -> Option<&'static HarnessEntry> {
    CATALOG.iter().find(|entry| entry.id == id)
}

/// The program and the arguments that start the ACP agent of a harness.
///
/// An npx entry runs `npx --yes <package> <args>`. `--yes` keeps npx
/// from asking on the harness's stdin, which carries ACP.
pub fn launch_command(entry: &HarnessEntry) -> (&'static str, Vec<&'static str>) {
    match entry.launch {
        Launch::Npx { package, args } => {
            let mut argv = vec!["--yes", package];
            argv.extend_from_slice(args);
            ("npx", argv)
        }
        Launch::Binary { program, args, .. } => (program, args.to_vec()),
    }
}

/// The prefix of each Host capability that names a Coding Harness.
pub const CAPABILITY_PREFIX: &str = "harness:";

/// The Host capability that a Host declares when it can start the
/// harness with this id: `harness:<id>`.
pub fn capability(id: &str) -> String {
    format!("{CAPABILITY_PREFIX}{id}")
}

/// The programs that must be on the Person's `PATH` to start a harness:
/// the launcher (`npx` or the binary), then the entry's `requires`.
pub fn launchers(entry: &HarnessEntry) -> Vec<&'static str> {
    let launcher = match entry.launch {
        Launch::Npx { .. } => "npx",
        Launch::Binary { program, .. } => program,
    };
    std::iter::once(launcher)
        .chain(entry.requires.iter().copied())
        .collect()
}

/// Whether the harness has a launch command for the platform of a Host,
/// as the client reports it: `macos`, `linux`, `windows` and so on. An
/// npx entry runs on each platform. A binary entry runs on the operating
/// systems of its archives, which the ACP registry names `darwin`,
/// `linux` and `windows`.
pub fn launches_on(entry: &HarnessEntry, host_platform: &str) -> bool {
    match entry.launch {
        Launch::Npx { .. } => true,
        Launch::Binary { archives, .. } => {
            let system = match host_platform {
                "macos" => "darwin",
                other => other,
            };
            archives
                .iter()
                .any(|archive| archive.platform.split('-').next() == Some(system))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four platforms the Client App and the Computer Image run on.
    const PLATFORMS: [&str; 4] = [
        "darwin-aarch64",
        "darwin-x86_64",
        "linux-aarch64",
        "linux-x86_64",
    ];

    fn harness(id: &str) -> &'static HarnessEntry {
        entry(id).unwrap_or_else(|| panic!("{id} is not in the catalog"))
    }

    /// Whether a version is exact: three dot-separated runs of digits,
    /// so no range such as `^1.2.0` and no tag such as `latest`.
    fn is_exact_version(version: &str) -> bool {
        let parts: Vec<&str> = version.split('.').collect();
        parts.len() == 3
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    }

    /// The version of an npm package spec such as `@scope/name@1.2.3`.
    fn package_version(spec: &str) -> Option<&str> {
        let (name, version) = spec.rsplit_once('@')?;
        (!name.is_empty()).then_some(version)
    }

    #[test]
    fn the_catalog_holds_the_seven_harnesses_in_order() {
        let ids: Vec<&str> = catalog().iter().map(|entry| entry.id).collect();
        assert_eq!(
            ids,
            [
                "claude", "codex", "opencode", "pi", "gemini", "copilot", "cursor"
            ]
        );
        for id in ids {
            assert_eq!(harness(id).id, id);
        }
        assert!(entry("claude-acp").is_none());
        assert!(entry("").is_none());
        assert!(entry("Claude").is_none());
    }

    #[test]
    fn each_npx_package_names_the_exact_version_of_its_entry() {
        let mut npx = 0;
        for entry in catalog() {
            if let Launch::Npx { package, .. } = entry.launch {
                npx += 1;
                assert!(is_exact_version(entry.version), "{}", entry.id);
                assert_eq!(
                    package_version(package),
                    Some(entry.version),
                    "{}",
                    entry.id
                );
            }
        }
        assert_eq!(npx, 5);
    }

    #[test]
    fn each_binary_entry_pins_an_archive_for_each_platform() {
        let mut binaries = 0;
        for entry in catalog() {
            if let Launch::Binary { archives, .. } = entry.launch {
                binaries += 1;
                let platforms: Vec<&str> =
                    archives.iter().map(|archive| archive.platform).collect();
                assert_eq!(platforms, PLATFORMS, "{}", entry.id);
                for archive in archives {
                    let hex = archive.sha256;
                    assert_eq!(hex.len(), 64, "{} {}", entry.id, archive.platform);
                    assert!(
                        hex.bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
                        "{} {}",
                        entry.id,
                        archive.platform
                    );
                    assert!(
                        archive.url.contains(entry.version),
                        "{} {}",
                        entry.id,
                        archive.platform
                    );
                }
            }
        }
        assert_eq!(binaries, 2);
    }

    #[test]
    fn only_pi_asks_no_permission() {
        let silent: Vec<&str> = catalog()
            .iter()
            .filter(|entry| !entry.asks_permission)
            .map(|entry| entry.id)
            .collect();
        assert_eq!(silent, ["pi"]);
    }

    #[test]
    fn each_harness_signs_in_with_a_subscription_and_pins_its_npx_commands() {
        for entry in catalog() {
            let methods: Vec<SignInMethod> =
                entry.sign_in.iter().map(|sign_in| sign_in.method).collect();
            assert!(
                methods.contains(&SignInMethod::Subscription),
                "{}",
                entry.id
            );
            let api_key = methods.contains(&SignInMethod::ApiKey);
            assert_eq!(
                api_key,
                !matches!(entry.id, "copilot" | "cursor"),
                "{}",
                entry.id
            );
            for sign_in in entry.sign_in {
                if let SignInAction::Command(["npx", rest @ ..]) = sign_in.how {
                    let package = rest
                        .iter()
                        .find(|arg| !arg.starts_with('-'))
                        .unwrap_or_else(|| panic!("{} runs npx with no package", entry.id));
                    let version = package_version(package)
                        .unwrap_or_else(|| panic!("{} runs {package} with no version", entry.id));
                    assert!(is_exact_version(version), "{} runs {package}", entry.id);
                }
            }
        }
    }

    #[test]
    fn the_launch_command_runs_npx_without_a_question_or_the_binary() {
        assert_eq!(
            launch_command(harness("claude")),
            (
                "npx",
                vec!["--yes", "@agentclientprotocol/claude-agent-acp@0.87.0"]
            )
        );
        assert_eq!(launch_command(harness("gemini")).1.last(), Some(&"--acp"));
        assert_eq!(
            launch_command(harness("opencode")),
            ("opencode", vec!["acp"])
        );
    }

    #[test]
    fn a_harness_is_a_host_capability_with_its_launchers() {
        assert_eq!(capability("pi"), "harness:pi");
        assert_eq!(launchers(harness("pi")), ["npx", "pi"]);
        assert_eq!(launchers(harness("cursor")), ["cursor-agent"]);
    }

    /// A file of the repository, read at run time: a test binary that
    /// another worktree built must not hold the path of this one.
    fn repository_file(path: &str) -> String {
        let manifest =
            std::env::var_os("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(&manifest).join("../..").join(path);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
    }

    /// The default value of `ARG <name>=<value>` in a Dockerfile.
    fn dockerfile_arg<'a>(dockerfile: &'a str, name: &str) -> Option<&'a str> {
        dockerfile
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("ARG {name}=")))
    }

    fn computer_launches() -> Vec<(&'static str, ComputerLaunch)> {
        catalog()
            .iter()
            .filter_map(|entry| entry.computer.map(|launch| (entry.id, launch)))
            .collect()
    }

    #[test]
    fn the_four_harnesses_that_a_harness_model_endpoint_serves_run_in_a_computer() {
        let ids: Vec<&str> = computer_launches().iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, ["claude", "codex", "opencode", "pi"]);
        for id in ["gemini", "copilot", "cursor"] {
            assert_eq!(harness(id).computer, None, "{id}");
        }
    }

    #[test]
    fn a_computer_launch_runs_the_installed_program_and_never_npx() {
        let launch = |id| {
            let launch = harness(id).computer.expect("a Computer launch");
            (launch.program, launch.args)
        };
        assert_eq!(launch("claude"), ("claude-agent-acp", &[][..]));
        assert_eq!(launch("codex"), ("codex-acp", &[][..]));
        assert_eq!(launch("opencode"), ("opencode", &["acp"][..]));
        assert_eq!(launch("pi"), ("pi-acp", &[][..]));
        for (id, launch) in computer_launches() {
            assert_ne!(launch.program, "npx", "{id}");
            for package in launch.packages {
                let version = package_version(package)
                    .unwrap_or_else(|| panic!("{id} installs {package} with no version"));
                assert!(is_exact_version(version), "{id} installs {package}");
            }
        }
    }

    /// The Computer Image installs `computer/harnesses/package.json`, so
    /// its dependencies are exactly the packages of the Computer
    /// launches, and each of these holds the npx package of its entry.
    #[test]
    fn the_computer_image_installs_the_packages_of_the_computer_launches() {
        let manifest: serde_json::Value =
            serde_json::from_str(&repository_file("computer/harnesses/package.json"))
                .expect("package.json is JSON");
        let mut installed: Vec<String> = manifest["dependencies"]
            .as_object()
            .expect("package.json has dependencies")
            .iter()
            .map(|(name, version)| format!("{name}@{}", version.as_str().unwrap_or_default()))
            .collect();
        installed.sort();
        let mut packages: Vec<String> = computer_launches()
            .iter()
            .flat_map(|(_, launch)| launch.packages.iter().map(|package| package.to_string()))
            .collect();
        packages.sort();
        assert_eq!(installed, packages);

        for (id, launch) in computer_launches() {
            if let Launch::Npx { package, .. } = harness(id).launch {
                assert!(launch.packages.contains(&package), "{id}");
            }
        }
    }

    /// `npm ci` checks each package against the `integrity` of the lock,
    /// so a lock entry with no `integrity` is a download that nothing
    /// pins.
    #[test]
    fn the_harness_lock_pins_each_package_with_its_integrity() {
        let lock: serde_json::Value =
            serde_json::from_str(&repository_file("computer/harnesses/package-lock.json"))
                .expect("package-lock.json is JSON");
        let packages = lock["packages"].as_object().expect("the lock has packages");
        assert!(packages.len() > 1);
        for (path, package) in packages.iter().filter(|(path, _)| !path.is_empty()) {
            let integrity = package["integrity"].as_str().unwrap_or_default();
            assert!(integrity.starts_with("sha512-"), "{path} has no sha512");
        }
    }

    /// The Computer Image downloads the OpenCode archive of the catalog
    /// for its architecture.
    #[test]
    fn the_computer_image_downloads_the_opencode_archive_of_the_catalog() {
        let dockerfile = repository_file("computer/Dockerfile");
        let opencode = harness("opencode");
        assert_eq!(
            dockerfile_arg(&dockerfile, "OPENCODE_VERSION"),
            Some(opencode.version)
        );
        let Launch::Binary { archives, .. } = opencode.launch else {
            panic!("opencode is a binary harness");
        };
        for (arch, platform) in [("amd64", "linux-x86_64"), ("arm64", "linux-aarch64")] {
            let archive = archives
                .iter()
                .find(|archive| archive.platform == platform)
                .unwrap_or_else(|| panic!("opencode has no {platform} archive"));
            assert_eq!(
                dockerfile_arg(&dockerfile, &format!("OPENCODE_SHA256_{arch}")),
                Some(archive.sha256),
                "{platform}"
            );
        }
    }
}
