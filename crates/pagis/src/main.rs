use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Parser, Subcommand};
use pagis_server::{RESTART_EXIT_CODE, RestartSwitch};
use tokio_util::sync::CancellationToken;

/// Pagis gives you a staff of Agents: AI helpers that work as your
/// virtual assistants. With no command, pagis starts the daemon.
#[derive(Debug, Parser)]
#[command(name = "pagis", version = pagis_server::VERSION, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(flatten)]
    run: RunFlags,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check the installed server package, and start nothing.
    PackageCheck,
    /// Write a Backup of this installation into a new directory. Stop
    /// the daemon first.
    Backup {
        /// The directory to write. It must not exist.
        directory: PathBuf,
    },
    /// Restore a Backup into a new PAGIS_HOME and an empty database.
    Restore {
        /// The directory that `pagis backup` wrote.
        directory: PathBuf,
    },
    /// Print a Sign-In Link and its QR code that sign one more browser or
    /// app in. The link is good for five minutes and one use.
    Pair {
        /// The address of the Person to sign in. Without it, the first
        /// Administrator.
        #[arg(long, value_name = "ADDRESS")]
        email: Option<String>,
    },
}

fn main() -> anyhow::Result<()> {
    keep_new_files_private();
    let cli = Cli::parse();
    match cli.command {
        Some(Command::PackageCheck) => package_check(),
        Some(Command::Backup { directory }) => backup_cmd(&directory),
        Some(Command::Restore { directory }) => restore_cmd(&directory),
        Some(Command::Pair { email }) => pair_cmd(email.as_deref()),
        None => {
            let code = tokio::runtime::Runtime::new()?.block_on(run(cli.run))?;
            if code != 0 {
                std::process::exit(code);
            }
            Ok(())
        }
    }
}

/// Set the umask of the process to 077, as OpenSSH, GnuPG and
/// PostgreSQL do. Every file and directory that the process makes then
/// gives no permission to a group or to other users, whatever the umask
/// of the shell or the Client App that started it. SQLite, libgit2, the
/// log writer and `pg_dump` make their files with no mode of their own,
/// so this one call covers them.
///
/// The call comes before the runtime starts a thread, so no thread
/// makes a file under the umask that the process started with.
fn keep_new_files_private() {
    use rustix::fs::Mode;
    rustix::process::umask(Mode::RWXG | Mode::RWXO);
}

/// Check the installed server package without opening a Workspace. This does
/// not start a listener, read the keychain or create PAGIS_HOME.
fn package_check() -> anyhow::Result<()> {
    let executable = std::env::current_exe()?;
    let image = pagis_computer::IMAGE;
    pagis::validate_server_package(&executable, pagis::product_app_is_built(), image)?;
    println!(
        "pagis {} package ok: embedded Product App, sibling gog, {}",
        pagis_server::VERSION,
        image
    );
    Ok(())
}

/// `pagis backup <directory>`: write an archive of this installation
/// into a fresh directory. The daemon must be stopped; the
/// backup takes its instance lock and says so when it cannot.
///
/// `deploy/backup.sh` runs this and captures the Agent volumes beside
/// it, which belong to the Docker host rather than to this process.
fn backup_cmd(out: &Path) -> anyhow::Result<()> {
    let home = pagis_home()?;
    let manifest = pagis::backup::Installation::at(&home)?.back_up(out)?;
    println!(
        "backed up Pagis {} ({:?} records) from {} into {}",
        manifest.release,
        manifest.database,
        home.display(),
        out.display()
    );
    Ok(())
}

/// `pagis restore <directory>`: put an archive back into a fresh state
/// directory and an empty database.
fn restore_cmd(archive: &Path) -> anyhow::Result<()> {
    let home = pagis_home()?;
    let manifest = pagis::backup::Installation::from_archive(archive, &home)?.restore(archive)?;
    println!(
        "restored Pagis {} into {}; start it with Pagis {} or newer",
        manifest.release,
        home.display(),
        manifest.release
    );
    Ok(())
}

/// `pagis pair`: write a Sign-In Link of the Public Origin into the
/// records of this installation, and print it with its QR code. It needs
/// no running daemon.
fn pair_cmd(email: Option<&str>) -> anyhow::Result<()> {
    let home = pagis_home()?;
    let pairing = tokio::runtime::Runtime::new()?.block_on(pagis::pair(
        &home,
        email,
        pagis_core::now_ms(),
    ))?;
    let who = pairing
        .person
        .email
        .as_deref()
        .or(pairing.person.name.as_deref())
        .unwrap_or("the Administrator");
    println!("{}", pagis_server::qr_text(&pairing.url)?);
    println!("sign in as {who} within five minutes; the link works once:");
    println!("{}", pairing.url);
    Ok(())
}

/// The flags of a daemon run.
#[derive(Debug, PartialEq, Eq, clap::Args)]
struct RunFlags {
    /// Serve a local installation for the people of this computer. The
    /// Client App passes it. Without it, pagis is a server.
    #[arg(long)]
    local: bool,
    /// Do not open the browser at the sign-in link.
    #[arg(long)]
    no_open: bool,
    /// Listen on this port for this run, whatever config.toml says.
    #[arg(long, value_name = "PORT")]
    port: Option<u16>,
}

/// Serve until the user stops the daemon or asks for a restart.
/// Returns the process exit code.
async fn run(flags: RunFlags) -> anyhow::Result<i32> {
    let mut signals = StopSignals::install()?;
    let home = pagis_home()?;
    pagis::create_state_directory(&home)?;
    let log_level = pagis::Config::read_file(&home.join("config.toml"))?.log_level;
    let log_guard = pagis::logging::init(&home, &log_level)?;

    let installation = match flags.local {
        true => pagis::Installation::Local,
        false => pagis::Installation::Server,
    };
    let booted = pagis::boot(&home, installation).await?;
    tracing::info!(home = %booted.home.display(), "pagis booted");

    let requested_port = flags.port.unwrap_or(booted.config.port);
    // The interface is a setting. A local installation sets
    // nothing and keeps loopback; a server names the address it serves
    // the network on.
    let bind = booted.config.bind_address()?;
    let listener = bind_listener(bind, requested_port, pagis::taken_port_message).await?;
    // The Administration Interface answers on its own port, bound
    // to loopback unless the deployment names an address, so the product
    // port can face the team while this one stays private.
    let administration_port = booted.config.administration.port;
    let administration_listener = bind_listener(
        booted.config.administration.bind_address()?,
        administration_port,
        pagis::taken_administration_port_message,
    )
    .await?;
    let administration_addr = administration_listener.local_addr()?;
    // The TURN server of the live screen listens on loopback while Remote
    // Access is on, at the fixed port that Funnel publishes (ADR-0028).
    let remote_access_turn_listener = match booted.config.remote_access.enabled {
        true => Some(
            bind_listener(
                std::net::Ipv4Addr::LOCALHOST.into(),
                booted.config.screen.remote_access_turn_port()?,
                pagis::taken_turn_port_message,
            )
            .await?,
        ),
        false => None,
    };
    let addr = listener.local_addr()?;
    let port = addr.port();
    // The origin a browser reaches this installation at, and the one
    // address whose forwarded headers the daemon believes.
    let public_origin = booted.config.public_origin(port);
    let proxy = booted.config.trusted_proxy()?;
    tracing::info!(
        %addr,
        %administration_addr,
        %public_origin,
        trusted_proxy = ?proxy.address(),
        remote_access = booted.config.remote_access.enabled,
        "listening"
    );
    let restart = Arc::new(RestartSwitch::default());
    let mut options = pagis::AppOptions::production(&booted, Arc::clone(&restart))?;
    let background = options.cancel.clone();
    options.runtime_port = port;
    options.remote_access_turn_listener = remote_access_turn_listener;
    if flags.port.is_some() {
        options.port_override = Some("--port");
    }
    // `--port` moves the served port, so the origin follows it.
    options.public_origin = public_origin.clone();
    let interfaces = pagis::app(&booted, options).await?;
    // The banner comes after every part of the daemon started, so it is
    // the last thing a start prints and the one-minute link does not
    // scroll away.
    //
    // A browser a person opens by hand signs in through a one-time
    // sign-in link: one minute, one use, and no credential in the
    // address bar.
    //
    // Only a local installation gets one. A link is an administrator
    // Session with no password behind it, so on a server it would be a
    // live way in printed into the container logs on every start. The
    // installation that holds a Client Credential is exactly the
    // installation whose banner a person at the machine reads, so the
    // one record decides this too. The link starts at the local origin,
    // because the daemon accepts it from this machine alone (ADR-0025).
    let url = match booted.client_credential.is_some() {
        true => Some(pagis::start_link(&booted.stores, &booted.config.local_origin(port)).await?),
        false => None,
    };
    let way_in = match &url {
        Some(url) if !flags.no_open => WayIn::Opened(url),
        Some(url) => WayIn::Link(url),
        None if pagis_server::setup::awaits_first_administrator(
            booted.stores.orgs.as_ref(),
            booted.stores.users.as_ref(),
        )
        .await? =>
        {
            WayIn::FirstAdministrator
        }
        None => WayIn::Password,
    };
    let administration_origin = pagis_server::AdministrationAddress::of(administration_addr).origin;
    for line in banner(&public_origin, &administration_origin, way_in) {
        println!("{line}");
    }
    // First run: open the browser into the onboarding wizard.
    if let Some(url) = url.as_ref().filter(|_| booted.first_run && !flags.no_open) {
        launch_browser(url);
    }

    // The sign-in rate limiter counts attempts per source address, so
    // the connection's address reaches the handler.
    let stopping = CancellationToken::new();
    let product = axum::serve(
        listener,
        interfaces
            .product
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stopping.clone().cancelled_owned());
    let administration = axum::serve(
        administration_listener,
        interfaces
            .administration
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(stopping.clone().cancelled_owned());
    // Both listeners belong to one process, so one of them stopping is
    // the daemon stopping: one stop ends both.
    let serving = async { tokio::join!(product, administration) };
    tokio::pin!(serving);
    let stop = tokio::select! {
        (product, administration) = &mut serving => {
            product?;
            administration?;
            anyhow::bail!("the listeners stopped with no stop request");
        }
        stop = signals.request(&restart) => stop,
    };
    tracing::info!(?stop, "stopping");
    stopping.cancel();
    background.cancel();
    // A live event stream or a screen socket stays open until its
    // browser closes it, so the drain has a limit. A supervisor sends
    // SIGKILL when its own grace period ends, and the limit is shorter.
    let drain = async {
        match tokio::time::timeout(DRAIN_LIMIT, &mut serving).await {
            Ok((product, administration)) => {
                product?;
                administration?;
            }
            Err(_) => tracing::info!(
                limit = ?DRAIN_LIMIT,
                "open connections did not close in the drain limit; they close now"
            ),
        }
        anyhow::Ok(())
    };
    // A daemon that stops for good stops the Computers it holds awake,
    // because no idle sweep runs after it to stop them. A restart keeps
    // them, and the next daemon adopts them.
    let computers = async {
        if stop == Stop::Signal {
            interfaces.computers.stop_all().await;
        }
    };
    let (drained, ()) = tokio::join!(drain, computers);
    drained?;
    if stop == Stop::Restart {
        println!("pagis stopped for a restart; run `pagis` again");
        // The log writer flushes on the guard, which `exit` skips.
        drop(log_guard);
        return Ok(RESTART_EXIT_CODE);
    }
    Ok(0)
}

/// How long the listeners get to finish the requests in flight after a
/// stop. Docker and systemd wait ten seconds or more before SIGKILL.
const DRAIN_LIMIT: std::time::Duration = std::time::Duration::from_secs(3);

/// Why the daemon stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// SIGINT (Ctrl-C) or SIGTERM (`docker stop`, `systemctl stop`, the
    /// Client App's quit): the daemon stops for good.
    Signal,
    /// The Administration Interface asked for a restart.
    Restart,
}

/// The signals that stop the daemon. They are installed before the
/// daemon boots, so a signal that comes early is kept and not fatal.
struct StopSignals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
}

impl StopSignals {
    fn install() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            interrupt: signal(SignalKind::interrupt())?,
            terminate: signal(SignalKind::terminate())?,
        })
    }

    /// Wait for the first stop request: a signal, or the restart that
    /// the Administration Interface asks for.
    async fn request(&mut self, restart: &RestartSwitch) -> Stop {
        tokio::select! {
            _ = self.interrupt.recv() => Stop::Signal,
            _ = self.terminate.recv() => Stop::Signal,
            () = restart.asked() => Stop::Restart,
        }
    }
}

/// How a person gets in, as the start banner says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WayIn<'a> {
    /// The browser opened on the one-time sign-in link.
    Opened(&'a str),
    /// The one-time sign-in link, for a browser a person opens by hand.
    Link(&'a str),
    /// A server where nobody can sign in yet: the first Administrator is
    /// made on the Administration Port, or from the environment.
    FirstAdministrator,
    Password,
}

/// The start banner: the last lines a start prints.
fn banner(public_origin: &str, administration_origin: &str, way_in: WayIn) -> Vec<String> {
    let mut lines = match way_in {
        WayIn::Opened(url) => return vec![format!("open {url}")],
        _ => vec![
            format!("Pagis is listening on {public_origin}/"),
            format!("administration: {administration_origin}/"),
        ],
    };
    lines.push(match way_in {
        WayIn::Link(url) => format!("sign in within one minute: {url}"),
        WayIn::FirstAdministrator => format!(
            "nobody can sign in yet: make the first Administrator at {administration_origin}/ \
             on this machine, or set {} and {} and start Pagis again",
            pagis_server::setup::ADMIN_EMAIL_VAR,
            pagis_server::setup::ADMIN_PASSWORD_VAR,
        ),
        WayIn::Password | WayIn::Opened(_) => "sign in with an address and a password".to_string(),
    });
    lines
}

/// Bind one listener, and say how to move the port when it is taken.
/// That is the one failure the daemon cannot report through an interface
/// of its own (ADR-0024), and each port says how to move that port.
async fn bind_listener(
    bind: std::net::IpAddr,
    port: u16,
    taken: fn(u16) -> String,
) -> anyhow::Result<tokio::net::TcpListener> {
    match tokio::net::TcpListener::bind(std::net::SocketAddr::new(bind, port)).await {
        Ok(listener) => Ok(listener),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            anyhow::bail!(taken(port));
        }
        Err(error) => Err(error.into()),
    }
}

fn launch_browser(url: &str) {
    if let Err(err) = open::that_detached(url) {
        tracing::warn!(error = %err, "cannot open the browser; use the printed URL");
    }
}

fn pagis_home() -> anyhow::Result<PathBuf> {
    if let Some(dir) = std::env::var_os("PAGIS_HOME") {
        return Ok(PathBuf::from(dir));
    }
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join(".pagis"))
        .ok_or_else(|| anyhow::anyhow!("HOME is not set; set PAGIS_HOME"))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use clap::error::ErrorKind;

    use super::*;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("pagis").chain(args.iter().copied()))
    }

    #[test]
    fn a_server_with_nobody_to_sign_in_names_where_the_first_administrator_is_made() {
        let lines = banner(
            "https://pagis.example",
            "http://127.0.0.1:4701",
            WayIn::FirstAdministrator,
        );

        let last = lines.last().unwrap();
        assert!(!last.contains("sign in with an address"), "{last}");
        assert!(last.contains("http://127.0.0.1:4701/"), "{last}");
        assert!(last.contains("PAGIS_ADMIN_EMAIL"), "{last}");
        assert!(last.contains("PAGIS_ADMIN_PASSWORD"), "{last}");
    }

    #[test]
    fn a_server_with_an_administrator_asks_for_a_password() {
        let lines = banner(
            "https://pagis.example",
            "http://127.0.0.1:4701",
            WayIn::Password,
        );

        assert_eq!(
            lines,
            [
                "Pagis is listening on https://pagis.example/",
                "administration: http://127.0.0.1:4701/",
                "sign in with an address and a password",
            ]
        );
    }

    #[test]
    fn the_command_line_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn the_run_flags_suppress_the_browser_and_read_the_port() {
        assert_eq!(
            parse(&[]).unwrap().run,
            RunFlags {
                port: None,
                no_open: false,
                local: false,
            }
        );
        assert_eq!(
            parse(&["--no-open", "--local", "--port", "4500"])
                .unwrap()
                .run,
            RunFlags {
                port: Some(4500),
                no_open: true,
                local: true,
            }
        );
        assert!(parse(&["--port", "eight"]).is_err());
    }

    #[test]
    fn help_is_a_success_and_names_every_flag_and_command() {
        for flag in ["--help", "-h"] {
            let error = parse(&[flag]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::DisplayHelp);
            assert_eq!(error.exit_code(), 0);
            // Help goes to stdout, not to stderr.
            assert!(!error.use_stderr());
        }
        let help = Cli::command().render_help().to_string();
        for (name, words) in [
            ("--local", "local installation"),
            ("--no-open", "browser"),
            ("--port", "port"),
            ("package-check", "server package"),
            ("backup", "Backup"),
            ("restore", "Restore"),
            ("pair", "Sign-In Link"),
        ] {
            let line = help
                .lines()
                .find(|line| line.trim_start().starts_with(name))
                .unwrap_or_else(|| panic!("the help names no {name}:\n{help}"));
            assert!(line.contains(words), "{name} is not described: {line}");
        }
    }

    #[test]
    fn an_unknown_argument_fails_with_the_usage() {
        for args in [&["serve"][..], &["--local", "--local"], &["backup"]] {
            let error = parse(args).unwrap_err();
            assert_ne!(error.exit_code(), 0, "{args:?}");
            assert!(error.use_stderr());
            assert!(
                error.render().to_string().contains("Usage: pagis"),
                "{args:?}"
            );
        }
    }

    #[test]
    fn the_commands_take_their_directory() {
        assert!(matches!(
            parse(&["backup", "/tmp/b"]).unwrap().command,
            Some(Command::Backup { directory }) if directory == Path::new("/tmp/b")
        ));
        assert!(matches!(
            parse(&["restore", "/tmp/b"]).unwrap().command,
            Some(Command::Restore { directory }) if directory == Path::new("/tmp/b")
        ));
        assert!(matches!(
            parse(&["package-check"]).unwrap().command,
            Some(Command::PackageCheck)
        ));
        assert!(matches!(
            parse(&["pair"]).unwrap().command,
            Some(Command::Pair { email: None })
        ));
        assert!(matches!(
            parse(&["pair", "--email", "grace@example.com"]).unwrap().command,
            Some(Command::Pair { email: Some(email) }) if email == "grace@example.com"
        ));
        // A flag of the daemon run does not go with a command.
        assert!(parse(&["--local", "backup", "/tmp/b"]).is_err());
    }
}
