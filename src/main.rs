use std::{path::PathBuf, thread};

use anyhow::Context;
use clap::{Parser, Subcommand};
use log::{info, warn};
use systemd_journal_logger::{connected_to_journal, JournalLog};

const DEFAULT_CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";
const DEFAULT_PORT: &str = "7878";

#[derive(Subcommand, Debug)]
enum Command {
    /// Check the configuration for syntax.
    Check,

    /// Run the daemon.
    ///
    /// For iptables, you'll need to be root.
    Run {
        /// How often to check for offending processes.
        #[arg(short, long, default_value = "60")]
        sleep_s: u64,

        #[arg(short, long, default_value = DEFAULT_PORT)]
        port: u16,    

        #[arg(short, long, default_value = "false")]
        ip_tables: bool,    
    },

    /// Perform iptables maintenance.
    ///
    /// You'll need to be root.
    IpTables {
        /// If true, remove any iptables configuration.
        #[arg(short, long, default_value = "false")]
        remove: bool,
    },

    Setup {
        /// If true, setup /etc/firefox/policies.json
        #[arg(long, default_value = "true")]
        policies: bool,

        /// If true, setup daemon for start.
        #[arg(long, default_value = "true")]
        daemon: bool,

        /// If true, start daemon now (requires `daemon``).
        #[arg(long, default_value = "true")]
        start: bool,

        /// If true, copy addon to /etc/firefox/addons
        #[arg(long, default_value = "true")]
        copy_addon: bool,

        /// If true, copy daemon to /usr/bin
        #[arg(long, default_value = "true")]
        copy_daemon: bool,
    }
}

/// A daemon designed to help avoid using some programs or websites
/// during (home)work hours.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(short, long, default_value = DEFAULT_CONFIG_PATH)]
    config: PathBuf,

    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<(), anyhow::Error> {
    if connected_to_journal() {
        eprintln!("using journal log");
        JournalLog::new().unwrap().install().unwrap();
    } else {
        eprintln!("using simple logger");
        simple_logger::SimpleLogger::new().env().init().unwrap();
    }

    let args = Args::parse();
    match args.command {
        Command::IpTables { remove } => {
            if remove {
                keep_it_focused::remove_ip_tables(keep_it_focused::IP_TABLES_PREFIX)?;
            }
        }
        Command::Check => {
            let reader = std::fs::File::open(&args.config)
                .with_context(|| format!("could not open file {}", args.config.display()))?;
            let config: keep_it_focused::config::Config = serde_yaml::from_reader(reader)
                .with_context(|| format!("could not parse file {}", args.config.display()))?;
            info!("config parsed, seems legit\n{}", serde_yaml::to_string(&config).expect("failed to display config"));
        }
        Command::Run { sleep_s, port, ip_tables } => {
            info!("loop: {}", "starting");
            let mut focuser = keep_it_focused::KeepItFocused::try_new(keep_it_focused::Options { ip_tables, port, path: args.config })
                .context("failed to apply configuration")?;
            focuser.background_serve();
        
            loop {
                info!("loop: {}", "sleeping");
                thread::sleep(std::time::Duration::from_secs(sleep_s));
                if let Err(err) = focuser.tick() {
                    warn!("problem during tick {}", err);
                }
            }        
        }
        Command::Setup { policies, copy_addon, copy_daemon, daemon, start } => {
            if policies {
                info!("setting up policies");
                keep_it_focused::setup::setup_policies()
                .context("failed to setup policies.json")?;
            }
            if copy_addon {
                info!("copying addon");
                keep_it_focused::setup::copy_addon()
                .context("failed to copy addon xpi")?;
            }
            if copy_daemon {
                info!("copying daemon");
                keep_it_focused::setup::copy_daemon()
                .context("failed to copy daemon")?;
            }
            if daemon {
                info!("setting up daemon");
                keep_it_focused::setup::setup_daemon(start)
                .context("failed to copy daemon")?;
            }
            info!("setup complete");
        }
    }
    Ok(())
}
