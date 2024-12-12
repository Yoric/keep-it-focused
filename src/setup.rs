//! Support for `keep-it-focused setup`.

use std::{
    collections::HashMap,
    io::{ErrorKind, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
};

use anyhow::Context;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::MetadataExt;

use crate::config;

const ADDON_FILE_NAME: &str = "keep-it-focused.xpi";

fn exe_name() -> String {
    std::env::args()
        .next()
        .expect("invalid environment missing arg[0]? this should be impossible unless you're writing your own operating system")
}

/// Copy the addon to /etc/firefox/addons.
pub fn copy_addon() -> Result<(), anyhow::Error> {
    const ADDON_SOURCE_SUBDIRECTORY: &str = "target/webext";
    const ADDONS_PATH: &str = "/etc/firefox/addons";

    // Create directory.
    std::fs::create_dir_all(ADDONS_PATH)
        .with_context(|| format!("failed to create {ADDONS_PATH}"))?;

    // Copy xpi.
    let source = Path::new(ADDON_SOURCE_SUBDIRECTORY).join(ADDON_FILE_NAME);
    let dest = Path::new(ADDONS_PATH).join(ADDON_FILE_NAME);
    debug!("copying {} to {}", source.display(), dest.display());
    std::fs::copy(&source, &dest)
        .with_context(|| format!("failed to copy {} to {}", source.display(), dest.display()))?;
    Ok(())
}

/// Setup /etc/firefox/policies.json to ensure that this addon
/// is automatically installed to all users on this machine.
pub fn setup_policies() -> Result<(), anyhow::Error> {
    const CONFIG_PATH: &str = "/etc/firefox/policies.json";
    const EXTENSION_ID: &str = "keep-it-focused@yoric.xyz";
    const INSTALL_URL: &str = "file:///etc/firefox/addons/keep-it-focused.xpi";

    // A data structure representing /etc/firefox/policies.json.
    //
    // Note that we maintain fields `_others` to maintain all the data
    // we don't want to change.
    #[derive(Deserialize, Serialize, Default)]
    struct Configuration {
        policies: Policies,
        #[serde(flatten)]
        _others: serde_json::Value,
    }
    #[derive(Deserialize, Serialize, Default)]
    struct Policies {
        #[serde(rename = "ExtensionSettings")]
        extension_settings: HashMap<String, ExtensionSettings>,
        #[serde(flatten)]
        _others: serde_json::Value,
    }
    #[derive(Deserialize, Serialize, Default)]
    struct ExtensionSettings {
        installation_mode: Option<InstallationMode>,
        install_url: Option<String>,
        #[serde(flatten)]
        _others: serde_json::Value,
    }
    #[derive(Deserialize, Serialize)]
    enum InstallationMode {
        #[serde(rename = "allowed")]
        Allowed,
        #[serde(rename = "blocked")]
        Blocked,
        #[serde(rename = "force_installed")]
        ForceInstalled,
        #[serde(rename = "normal_installed")]
        NormalInstalled,
    }

    std::fs::create_dir_all("/etc/firefox/addons")
        .context("Failed to create /etc/firefox/addons")?;

    // Load /etc/firefox/policies.json.
    debug!("reading {}", CONFIG_PATH);
    let mut config: Configuration = match std::fs::File::open(CONFIG_PATH) {
        Ok(file) => serde_json::from_reader(std::io::BufReader::new(file))
            .context("Failed to parse policies.json")?,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            debug!("file is empty, creating");
            Configuration::default()
        }
        Err(err) => return Err(err).with_context(|| format!("failed to open {CONFIG_PATH}")),
    };

    // Patch content.
    let extension_settings = config
        .policies
        .extension_settings
        .entry(EXTENSION_ID.to_string())
        .or_default();
    extension_settings.install_url = Some(INSTALL_URL.to_string());
    extension_settings.installation_mode = Some(InstallationMode::ForceInstalled);

    // Write back content.
    debug!("writing {}", CONFIG_PATH);
    let file = std::fs::File::create(CONFIG_PATH)
        .with_context(|| format!("failed to open {CONFIG_PATH} for writing"))?;
    serde_json::to_writer_pretty(std::io::BufWriter::new(file), &config)
        .with_context(|| format!("failed to write to {CONFIG_PATH}"))?;
    Ok(())
}

/// Copy this binary to /usr/bin, make it world-executable.
pub fn copy_daemon() -> Result<(), anyhow::Error> {
    info!("if the daemon is started, let's stop it before copying");
    let mut stop_command = std::process::Command::new("systemctl");
    stop_command.args(["stop", "keep-it-focused"]);
    let mut child = stop_command
        .spawn()
        .with_context(|| "failed to stop daemon")?;
    if let Err(err) = child.wait() {
        debug!("could not stop daemon: {}", err);
    }

    const DEST_DIRECTORY: &str = "/usr/bin";
    let source = exe_name();
    let name = std::path::Path::new(&source).file_name()
        .expect("missing file name? this should be impossible unless you're writing your own operating system");
    let dest = Path::new(DEST_DIRECTORY).join(name);
    debug!("copying {source} to {}", dest.display());
    std::fs::copy(&source, dest)
        .with_context(|| format!("failed to copy {source} to {DEST_DIRECTORY} - perhaps you need to stop the daemon with `sudo systemctl stop keep-it-focused`"))?;
    Ok(())
}

/// Setup this daemon for start upon next system launch.
pub fn setup_daemon(auto_start: bool) -> Result<(), anyhow::Error> {
    // Create an empty config if there's no config at the oment.
    const DAEMON_CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";
    info!("creating empty config at {DAEMON_CONFIG_PATH}");
    if std::fs::metadata(DAEMON_CONFIG_PATH).is_ok() {
        warn!(
            "file {} already exists, we're not overwriting it",
            DAEMON_CONFIG_PATH
        );
        let reader = std::fs::File::open(DAEMON_CONFIG_PATH).with_context(|| {
            format!(
                "could not open existing configuration {}",
                DAEMON_CONFIG_PATH
            )
        })?;
        let config: config::Config = serde_yaml::from_reader(reader).with_context(|| {
            format!(
                "could not parse existing configuration {}",
                DAEMON_CONFIG_PATH
            )
        })?;
        info!(
            "the existing configuration seems syntactically correct\n{}",
            serde_yaml::to_string(&config).expect("failed to display config")
        );
    } else {
        let mut file = std::fs::File::create_new(SYSTEMD_CONFIG_PATH)
            .with_context(|| format!("failed to create {SYSTEMD_CONFIG_PATH}"))?;
        let config = config::Config::default();
        let data = serde_yaml::to_string(&config).expect("cannot serialize an empty config?");
        file.write_all(data.as_bytes())
            .with_context(|| format!("failed to write {SYSTEMD_CONFIG_PATH}"))?;
    }

    // Write /etc/systemd/system/keep-it-focused.service
    info!("writing down system configuration to start daemon automatically");
    const SYSTEMD_DATA: &str = r#"
    [Unit]
    Description=Prevent some distracting applications from launching outside allowed times.
    
    [Install]
    # Make sure that the daemon is launched on startup.
    WantedBy=graphical.target multi-user.target
    
    [Service]
    User=root
    WorkingDirectory=/root
    ExecStart=/usr/bin/keep-it-focused run
    Environment=RUST_LOG=info
    Restart=always
    RestartSec=3
    "#;
    const SYSTEMD_CONFIG_PATH: &str = "/etc/systemd/system/keep-it-focused.service";
    if std::fs::metadata(SYSTEMD_CONFIG_PATH).is_ok() {
        warn!(
            "file {} already exists, we're not overwriting it",
            SYSTEMD_CONFIG_PATH
        );
    } else {
        let mut file = std::fs::File::create_new(SYSTEMD_CONFIG_PATH)
            .with_context(|| format!("failed to create {SYSTEMD_CONFIG_PATH}"))?;
        file.write_all(SYSTEMD_DATA.as_bytes())
            .with_context(|| format!("failed to write {SYSTEMD_CONFIG_PATH}"))?;
    }

    // Prepare for restart.
    info!("preparing daemon for next startup");
    let mut cmd = std::process::Command::new("systemctl");
    cmd.args(["enable", "keep-it-focused"]);
    cmd.spawn().context("Error in `systemctl enable`")?;

    // Prepare for start.
    if auto_start {
        info!("attempting to start daemon");
        let mut cmd = std::process::Command::new("systemctl");
        cmd.args(["start", "keep-it-focused"]);
        cmd.spawn().context("Error in `systemctl start`")?;
    }

    Ok(())
}

pub fn make_extension_dir(path: &Path) -> Result<(), anyhow::Error> {
    // Note: this direcotry MUST belong to root and be writeable only by root.
    let trusted = match std::fs::create_dir_all(path) {
        Ok(()) => true,
        Err(err) if err.kind() == ErrorKind::AlreadyExists => {
            /* not an error */
            false
        }
        Err(err) => {
            return Err(err).context("Failed to create directory to store temporary rules");
        }
    };

    let mut permissions = std::fs::metadata(path)
        .context("Failed to read metadata on temporary rules dir")?
        .permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(path, permissions)
        .context("Failed to set permissions on temporary rules dir")?;

        if !trusted {
        // The directory was already created, it belongs to us, but it may have been created by someone else.
        const ROOT_UID: u32 = 0;
        const ROOT_GID: u32 = 0;
        // First, make sure that we're the only ones who can access it.
        std::os::unix::fs::chown(path, Some(ROOT_UID), Some(ROOT_GID))
            .context("Failed to acquire directory to store temporary rules")?;
        // Then remove any content that was created by anyone else.
        for entry in std::fs::read_dir(path).context("Could not walk temporary rules dir")? {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.uid() != ROOT_UID || metadata.gid() != ROOT_UID {
                std::fs::remove_file(entry.path())
                    .with_context(|| format!("Failed to remove file {}", entry.path().display()))?;
            }
        }
    }

    Ok(())
}
