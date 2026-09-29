//! The System Settings the daemon keeps in its config file
//! (ADR-0024), and the message the CLI prints when the port is taken.
//! The daemon is the one writer of the config file: the user changes a
//! System Setting in Settings, never in an editor.

use std::path::{Path, PathBuf};

use pagis_server::{MultiUserMode, SystemConfig, SystemConfigFile, origin_host_is_loopback};

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
        config.save(&path).map_err(|error| error.to_string())
    }

    fn multi_user(&self) -> Result<Option<MultiUserMode>, String> {
        let config = Config::read_file(&self.path()).map_err(|error| error.to_string())?;
        let public_origin = config.public_origin(config.port);
        if origin_host_is_loopback(&public_origin) {
            return Ok(None);
        }
        let trusted_proxy = config
            .trusted_proxy()
            .map_err(|error| error.to_string())?
            .address();
        Ok(Some(MultiUserMode {
            public_origin,
            trusted_proxy,
        }))
    }

    fn set_multi_user(&self, mode: Option<&MultiUserMode>) -> Result<(), String> {
        let path = self.path();
        let mut config = Config::read_file(&path).map_err(|error| error.to_string())?;
        // Loopback in both directions: the Client App reaches the daemon
        // at 127.0.0.1, and the owner's proxy or tunnel on this machine
        // does too, so the plain-HTTP port never faces the network.
        config.bind = DEFAULT_BIND.to_string();
        config.public_origin = mode
            .map(|mode| mode.public_origin.clone())
            .unwrap_or_default();
        config.trusted_proxy = mode
            .and_then(|mode| mode.trusted_proxy)
            .map(|address| address.to_string())
            .unwrap_or_default();
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

#[cfg(test)]
mod tests {
    use super::*;

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
        })
        .unwrap();

        assert_eq!(
            file.read().unwrap(),
            SystemConfig {
                port: 4500,
                docker_endpoint: Some("unix:///tmp/docker.sock".to_string()),
                log_level: "debug".to_string(),
                analytics: false,
            }
        );
    }

    /// The mode follows from the Public Origin: nothing configured is an
    /// installation that serves its own machine, and a switch writes the
    /// origin and the proxy and keeps the bind on loopback.
    #[test]
    fn the_multi_user_mode_follows_from_the_public_origin_in_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        let path = dir.path().join("config.toml");
        assert_eq!(file.multi_user().unwrap(), None);

        let mode = MultiUserMode {
            public_origin: "https://pagis.owner.example".to_string(),
            trusted_proxy: Some("127.0.0.1".parse().unwrap()),
        };
        file.set_multi_user(Some(&mode)).unwrap();

        assert_eq!(file.multi_user().unwrap(), Some(mode));
        let config = Config::read_file(&path).unwrap();
        assert!(config.bind_address().unwrap().is_loopback());
        assert_eq!(config.public_origin, "https://pagis.owner.example");
        assert_eq!(config.trusted_proxy, "127.0.0.1");

        file.set_multi_user(None).unwrap();

        assert_eq!(file.multi_user().unwrap(), None);
        let config = Config::read_file(&path).unwrap();
        assert!(config.bind_address().unwrap().is_loopback());
        assert_eq!(config.public_origin, "");
        assert_eq!(config.trusted_proxy, "");
    }

    /// A bind on a network address with no Public Origin derives a
    /// Public Origin on that address, which is not loopback, so the
    /// installation is in the multi-user mode. Turning the mode off binds
    /// loopback, which ends it.
    #[test]
    fn a_network_bind_is_the_multi_user_mode_until_it_is_turned_off() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        let path = dir.path().join("config.toml");
        let mut config = Config::read_file(&path).unwrap();
        config.bind = "192.168.1.20".to_string();
        config.save(&path).unwrap();

        assert_eq!(
            file.multi_user().unwrap(),
            Some(MultiUserMode {
                public_origin: "http://192.168.1.20:4400".to_string(),
                trusted_proxy: None,
            })
        );

        file.set_multi_user(None).unwrap();
        assert_eq!(file.multi_user().unwrap(), None);
    }

    /// The switch writes three keys and keeps the System Settings.
    #[test]
    fn a_switch_keeps_the_system_settings() {
        let dir = tempfile::tempdir().unwrap();
        let file = FileSystemConfig::new(dir.path());
        file.write(&SystemConfig {
            port: 4500,
            docker_endpoint: Some("unix:///tmp/docker.sock".to_string()),
            log_level: "debug".to_string(),
            analytics: false,
        })
        .unwrap();

        file.set_multi_user(Some(&MultiUserMode {
            public_origin: "https://pagis.owner.example".to_string(),
            trusted_proxy: None,
        }))
        .unwrap();

        assert_eq!(
            file.read().unwrap(),
            SystemConfig {
                port: 4500,
                docker_endpoint: Some("unix:///tmp/docker.sock".to_string()),
                log_level: "debug".to_string(),
                analytics: false,
            }
        );
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
        })
        .unwrap();

        let after = Config::read_file(&path).unwrap();
        assert_eq!(after.secrets.key_file(), before.secrets.key_file());
        assert_eq!(after.artifact_max_bytes, before.artifact_max_bytes);
        assert_eq!(after.docker_endpoint, "");
    }
}
