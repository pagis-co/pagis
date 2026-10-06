//! The System Settings the daemon keeps in its config file
//! (ADR-0024), and the message the CLI prints when the port is taken.
//! The daemon is the one writer of the config file: the user changes a
//! System Setting in Settings, never in an editor.

use std::path::{Path, PathBuf};

use pagis_server::{FUNNEL_PROXY, SystemConfig, SystemConfigFile};

use crate::config::{Config, DEFAULT_BIND};

/// The config file under the data directory.
pub struct FileSystemConfig {
    home: PathBuf,
}

impl FileSystemConfig {
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
        }
    }

    fn path(&self) -> PathBuf {
        self.home.join("config.toml")
    }
}

impl SystemConfigFile for FileSystemConfig {
    fn read(&self) -> Result<SystemConfig, String> {
        let config = Config::read_file(&self.path()).map_err(|error| error.to_string())?;
        Ok(SystemConfig {
            port: config.port,
            docker_endpoint: config.docker_endpoint(),
            log_level: config.log_level,
            analytics: config.analytics,
            home_exit: config.computer.home_exit,
        })
    }

    fn write(&self, settings: &SystemConfig) -> Result<(), String> {
        let path = self.path();
        // Read first and write the whole file back: the file holds
        // more than the System Settings, and none of the rest changes.
        let mut config = Config::read_file(&path).map_err(|error| error.to_string())?;
        config.port = settings.port;
        config.docker_endpoint = settings.docker_endpoint.clone().unwrap_or_default();
        config.log_level = settings.log_level.clone();
        config.analytics = settings.analytics;
        config.computer.home_exit = settings.home_exit;
        config.save(&path).map_err(|error| error.to_string())
    }

    fn remote_access(&self) -> Result<Option<String>, String> {
        let config = Config::read_file(&self.path()).map_err(|error| error.to_string())?;
        Ok(config
            .remote_access
            .enabled
            .then(|| config.public_origin(config.port)))
    }

    fn set_remote_access(&self, public_origin: Option<&str>) -> Result<(), String> {
        let path = self.path();
        let mut config = Config::read_file(&path).map_err(|error| error.to_string())?;
        // Loopback in both directions: the Client App reaches the daemon
        // at 127.0.0.1, and the Funnel of this machine does too, so the
        // plain-HTTP port never faces the network.
        config.bind = DEFAULT_BIND.to_string();
        config.remote_access.enabled = public_origin.is_some();
        config.public_origin = public_origin.unwrap_or_default().to_string();
        config.trusted_proxy = match public_origin {
            Some(_) => FUNNEL_PROXY.to_string(),
            None => String::new(),
        };
        config.save(&path).map_err(|error| error.to_string())
    }

    fn model_request_capture(&self) -> Result<(bool, u32), String> {
        let config = Config::read_file(&self.path()).map_err(|error| error.to_string())?;
        Ok((
            config.model_request_capture.enabled,
            config.model_request_capture.retention_days,
        ))
    }

    fn set_model_request_capture(&self, enabled: bool, retention_days: u32) -> Result<(), String> {
        let path = self.path();
        let mut config = Config::read_file(&path).map_err(|error| error.to_string())?;
        config.model_request_capture.enabled = enabled;
        config.model_request_capture.retention_days = retention_days;
        config.save(&path).map_err(|error| error.to_string())
    }

    fn data_directory(&self) -> PathBuf {
        self.home.clone()
    }
}

/// What the CLI prints when the port is taken (ADR-0024). It names the
/// port and the flag that starts Pagis on another port.
///
/// The bind attempt is the whole probe. The message does not name the
/// process that holds the port: that takes `lsof`, a program the
/// headless server image does not carry and a daemon does not depend
/// on for one diagnostic line. The Client App names the process on the
/// setup page, from the machine it runs on.
pub fn taken_port_message(port: u16) -> String {
    format!(
        "port {port} is already in use. Stop the process that holds it, or start Pagis on \
         another port with `pagis --port <PORT>`."
    )
}

/// The same failure for the administration port. The `--port`
/// flag moves the product port and not this one, so the message names
/// the setting that moves this one instead.
pub fn taken_administration_port_message(port: u16) -> String {
    format!(
        "the administration port {port} is already in use. Stop the process that holds it, \
         or name another port in `[administration] port` of config.toml."
    )
}

/// The same failure for the TURN server of Remote Access (ADR-0028). Its
/// port is a setting too, and Funnel names it, so a move needs the old
/// Funnel port removed and Remote Access turned on again.
pub fn taken_turn_port_message(port: u16) -> String {
    format!(
        "the TURN port {port} of Remote Access is already in use. Stop the process that holds \
         it, or name another port in `[screen] remote_access_turn_port` of config.toml, remove \
         the old one with `tailscale funnel --tls-terminated-tcp=8443 off`, and turn on Remote \
         Access again, so Tailscale Funnel publishes the new port."
    )
}

/// The same failure for the exit listener of a Server (ADR-0029). The
/// egress rules of the deployment name its port too, so a move changes
/// both.
pub fn taken_exit_port_message(port: u16) -> String {
    format!(
        "the exit port {port} of the Computers is already in use. Stop the process that holds \
         it, or name another port in `[computer] exit_port` of config.toml (or \
         PAGIS_COMPUTER_EXIT_PORT), and the same port in PAGIS_EXIT_PORT of the egress rules."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exit port is a setting of its own, and the egress rules name
    /// it, so its message names both.
    #[test]
    fn the_exit_port_message_names_its_setting_and_the_egress_rules() {
        let message = taken_exit_port_message(4403);

        assert!(message.contains("exit port 4403"));
        assert!(message.contains("[computer] exit_port"));
        assert!(message.contains("PAGIS_EXIT_PORT"));
        assert!(!message.contains("--port"));
    }

    #[test]
    fn the_taken_port_message_names_the_port_and_the_flag() {
        let message = taken_port_message(4400);

        assert!(message.contains("port 4400"));
        assert!(message.contains("`pagis --port <PORT>`"));
    }

    /// The administration port is moved by a setting and not by the
    /// `--port` flag, so its message names the setting.
    #[test]
    fn the_administration_port_message_names_its_own_setting() {
        let message = taken_administration_port_message(4401);

        assert!(message.contains("administration port 4401"));
        assert!(message.contains("[administration] port"));
        assert!(!message.contains("--port"));
    }

    /// The TURN port of Remote Access is a setting of its own, and the
    /// Funnel names it, so its message says to turn Remote Access on again.
    #[test]
    fn the_turn_port_message_names_its_own_setting() {
        let message = taken_turn_port_message(4402);

        assert!(message.contains("TURN port 4402"));
        assert!(message.contains("[screen] remote_access_turn_port"));
        assert!(message.contains("tailscale funnel --tls-terminated-tcp=8443 off"));
        assert!(message.contains("turn on Remote Access again"));
        assert!(!message.contains("--port"));
    }

    #[test]
    fn the_model_request_capture_goes_to_the_config_file_and_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        assert_eq!(file.model_request_capture().unwrap(), (false, 7));

        file.set_model_request_capture(true, 12).unwrap();

        assert_eq!(file.model_request_capture().unwrap(), (true, 12));
        // The rest of the file stays as it was.
        assert!(file.read().unwrap().analytics);
    }

    #[test]
    fn the_system_settings_go_to_the_config_file_and_come_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        assert_eq!(file.data_directory(), dir.path());

        file.write(&SystemConfig {
            port: 4500,
            docker_endpoint: Some("unix:///tmp/docker.sock".to_string()),
            log_level: "debug".to_string(),
            analytics: false,
            home_exit: false,
        })
        .unwrap();

        assert_eq!(
            file.read().unwrap(),
            SystemConfig {
                port: 4500,
                docker_endpoint: Some("unix:///tmp/docker.sock".to_string()),
                log_level: "debug".to_string(),
                analytics: false,
                home_exit: false,
            }
        );
        assert!(
            std::fs::read_to_string(dir.path().join("config.toml"))
                .unwrap()
                .contains("home_exit = false")
        );
    }

    /// Remote Access writes three settings and keeps the bind on
    /// loopback, and turning it off clears all three.
    #[test]
    fn remote_access_writes_the_origin_and_the_proxy_and_clears_them() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        let path = dir.path().join("config.toml");
        assert_eq!(file.remote_access().unwrap(), None);

        file.set_remote_access(Some("https://owner-mac.tail1234.ts.net"))
            .unwrap();

        assert_eq!(
            file.remote_access().unwrap().as_deref(),
            Some("https://owner-mac.tail1234.ts.net")
        );
        let config = Config::read_file(&path).unwrap();
        assert!(config.remote_access.enabled);
        assert!(config.bind_address().unwrap().is_loopback());
        assert_eq!(config.public_origin, "https://owner-mac.tail1234.ts.net");
        assert_eq!(config.trusted_proxy, "127.0.0.1");

        file.set_remote_access(None).unwrap();

        assert_eq!(file.remote_access().unwrap(), None);
        let config = Config::read_file(&path).unwrap();
        assert!(!config.remote_access.enabled);
        assert!(config.bind_address().unwrap().is_loopback());
        assert_eq!(config.public_origin, "");
        assert_eq!(config.trusted_proxy, "");
    }

    /// A Public Origin or a network bind that somebody wrote by hand is
    /// not Remote Access: only the switch turns it on. Turning it off
    /// binds loopback again.
    #[test]
    fn a_public_origin_alone_is_not_remote_access() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        let path = dir.path().join("config.toml");
        let mut config = Config::read_file(&path).unwrap();
        config.bind = "192.168.1.20".to_string();
        config.public_origin = "https://pagis.owner.example".to_string();
        config.save(&path).unwrap();

        assert_eq!(file.remote_access().unwrap(), None);

        file.set_remote_access(None).unwrap();
        assert!(
            Config::read_file(&path)
                .unwrap()
                .bind_address()
                .unwrap()
                .is_loopback()
        );
    }

    /// The switch keeps the System Settings.
    #[test]
    fn a_switch_keeps_the_system_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        let settings = SystemConfig {
            port: 4500,
            docker_endpoint: Some("unix:///tmp/docker.sock".to_string()),
            log_level: "debug".to_string(),
            analytics: false,
            home_exit: false,
        };
        file.write(&settings).unwrap();

        file.set_remote_access(Some("https://owner-mac.tail1234.ts.net"))
            .unwrap();

        assert_eq!(file.read().unwrap(), settings);
    }

    #[test]
    fn a_write_keeps_the_rest_of_the_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        let path = dir.path().join("config.toml");
        let before = Config::read_file(&path).unwrap();

        file.write(&SystemConfig {
            port: 4500,
            docker_endpoint: None,
            log_level: "warn".to_string(),
            analytics: true,
            home_exit: true,
        })
        .unwrap();

        let after = Config::read_file(&path).unwrap();
        assert_eq!(after.secrets.key_file(), before.secrets.key_file());
        assert_eq!(after.artifact_max_bytes, before.artifact_max_bytes);
        assert_eq!(after.docker_endpoint, "");
    }
}
