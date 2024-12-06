//! Support for `keep-it-focused setup`.

use std::{collections::HashMap, io::{ErrorKind, Write}, path::Path};

use anyhow::Context;
use log::{debug, info, warn};
use serde::{Deserialize, Serialize};

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
        #[serde(rename="ExtensionSettings")]
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
        #[serde(rename="allowed")]
        Allowed,
        #[serde(rename="blocked")]
        Blocked,
        #[serde(rename="force_installed")]
        ForceInstalled,
        #[serde(rename="normal_installed")]
        NormalInstalled,
    }

    std::fs::create_dir_all("/etc/firefox/addons")
        .context("failed to create /etc/firefox/addons")?;

    // Load /etc/firefox/policies.json.
    debug!("reading {}", CONFIG_PATH);
    let mut config: Configuration = match std::fs::File::open(CONFIG_PATH) {
        Ok(file) => {
            serde_json::from_reader(
                std::io::BufReader::new(file)
            ).context("failed to parse policies.json")?
        },
        Err(err) if err.kind() == ErrorKind::NotFound => {
            debug!("file is empty, creating");
            Configuration::default()
        }
        Err(err) => {
            return Err(err).with_context(|| format!("failed to open {CONFIG_PATH}"))
        }
    };
        
    // Patch content.
    let extension_settings = config.policies.extension_settings.entry(EXTENSION_ID.to_string())
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
        warn!("file {} already exists, we're not overwriting it", DAEMON_CONFIG_PATH);
        let reader = std::fs::File::open(DAEMON_CONFIG_PATH)
            .with_context(|| format!("could not open existing configuration {}", DAEMON_CONFIG_PATH))?;
        let config: config::Config = serde_yaml::from_reader(reader)
            .with_context(|| format!("could not parse existing configuration {}", DAEMON_CONFIG_PATH))?;
        info!("the existing configuration seems syntactically correct\n{}", serde_yaml::to_string(&config).expect("failed to display config"));
    } else {
        let mut file = std::fs::File::create_new(SYSTEMD_CONFIG_PATH)
            .with_context(|| format!("failed to create {SYSTEMD_CONFIG_PATH}"))?;
        let config = config::Config::default();
        let data = serde_yaml::to_string(&config)
            .expect("cannot serialize an empty config?");
        file.write_all(data.as_bytes())
            .with_context(|| format!("failed to write {SYSTEMD_CONFIG_PATH}"))?;
    }


    // Write /etc/systemd/system/keep-it-focused.service
    info!("writing down system configuration to start daemon automatically");
    const SYSTEMD_DATA: &str = r"
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
    ";
    const SYSTEMD_CONFIG_PATH: &str = "/etc/systemd/system/keep-it-focused.service";
    if std::fs::metadata(SYSTEMD_CONFIG_PATH).is_ok() {
        warn!("file {} already exists, we're not overwriting it", SYSTEMD_CONFIG_PATH);
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
    cmd.spawn().context("error in `systemctl enable`")?;

    // Prepare for start.
    if auto_start {
        info!("attempting to start daemon");
        let mut cmd = std::process::Command::new("systemctl");
        cmd.args(["start", "keep-it-focused"]);
        cmd.spawn().context("error in `systemctl start`")?;
    }

    Ok(())
}