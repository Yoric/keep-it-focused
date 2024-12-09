use std::{io::ErrorKind, ops::Not, path::PathBuf, thread};

use anyhow::Context;
use clap::{Parser, Subcommand};
use keep_it_focused::{
    config::{Binary, DayConfig, Extension, ProcessConfig, WebFilter},
    types::Interval, uid_resolver::Uid,
};
use log::{info, warn, LevelFilter};
use procfs::sys::kernel::random::uuid;
use systemd_journal_logger::{connected_to_journal, JournalLog};

const DEFAULT_CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";
const DEFAULT_EXTENSIONS_PATH: &str = "/tmp/keep-it-focused/";
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

    /// Setup this tool for use on the system.
    ///
    /// You'll need to be root.
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

        /// If true, create extension directory
        #[arg(long, default_value = "true")]
        mkdir: bool,
    },

    /// Add a temporary rule.
    Exceptionally {
        #[arg(long)]
        user: String,

        #[command(subcommand)]
        rule: Rule,

        /// When the authorization starts.
        #[arg(long, value_parser=keep_it_focused::types::TimeOfDay::parse)]
        start: keep_it_focused::types::TimeOfDay,

        /// When the authorization stops.
        #[arg(long, value_parser=keep_it_focused::types::TimeOfDay::parse)]
        end: keep_it_focused::types::TimeOfDay,
    },
}

#[derive(Subcommand, Debug)]
enum Rule {
    Domain {
        /// The domain (subdomains are included automatically).
        domain: String,
    },
    Binary {
        /// The binary (globs are permitted).
        binary: String,
    },
}

/// A daemon designed to help avoid using some programs or websites
/// during (home)work hours.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// The path to the main config file.
    #[arg(short, long, default_value = DEFAULT_CONFIG_PATH)]
    main_config: PathBuf,

    /// A path for storing additional config files valid only for one day.
    #[arg(short, long, default_value = DEFAULT_EXTENSIONS_PATH)]
    extensions: PathBuf,

    #[command(subcommand)]
    command: Command,
}

fn main() -> Result<(), anyhow::Error> {
    if connected_to_journal() {
        eprintln!("using journal log");
        JournalLog::new()
            .unwrap()
            .with_extra_fields(vec![("VERSION", env!("CARGO_PKG_VERSION"))])
            .install()
            .unwrap();
        let max_level = match std::env::var("RUST_LOG").as_deref() {
            Ok("error") => LevelFilter::Error,
            Ok("debug") => LevelFilter::Debug,
            Ok("info") => LevelFilter::Info,
            Ok("trace") => LevelFilter::Trace,
            Ok("warn") => LevelFilter::Warn,
            _ => LevelFilter::Info,
        };
        log::set_max_level(max_level);
    } else {
        eprintln!("using simple logger");
        simple_logger::SimpleLogger::new().env().init().unwrap();
    }

    let args = Args::parse();
    match args.command {
        Command::IpTables { remove } => {
            if remove {
                keep_it_focused::remove_ip_tables()?;
            }
        }
        Command::Check => {
            let reader = std::fs::File::open(&args.main_config)
                .with_context(|| format!("could not open file {}", args.main_config.display()))?;
            let config: keep_it_focused::config::Config = serde_yaml::from_reader(reader)
                .with_context(|| format!("could not parse file {}", args.main_config.display()))?;
            info!(
                "config parsed, seems legit\n{}",
                serde_yaml::to_string(&config).expect("failed to display config")
            );
        }
        Command::Run {
            sleep_s,
            port,
            ip_tables,
        } => {
            info!("preparing file for temporary rules");
            keep_it_focused::setup::make_extension_dir(&args.extensions)
                .context("error while creating or setting up temporary rules directory")?;

            info!("loop: {}", "starting");
            let mut focuser = keep_it_focused::KeepItFocused::try_new(keep_it_focused::Options {
                ip_tables,
                port,
                main_config: args.main_config,
                extensions_dir: args.extensions,
            })
            .context("failed to apply configuration")?;
            focuser.background_serve();

            loop {
                info!("loop: {}", "sleeping");
                thread::sleep(std::time::Duration::from_secs(sleep_s));
                if let Err(err) = focuser.tick() {
                    warn!("problem during tick, skipping! {:?}", err);
                }
            }
        }
        Command::Setup {
            policies,
            copy_addon,
            copy_daemon,
            daemon,
            start,
            mkdir,
        } => {
            if Uid::me().is_root().not() {
                warn!("this command is meant to be executed as root");
            }
            if policies {
                info!("setting up policies");
                keep_it_focused::setup::setup_policies()
                    .context("failed to setup policies.json")?;
            }
            if copy_addon {
                info!("copying addon");
                keep_it_focused::setup::copy_addon().context("failed to copy addon xpi")?;
            }
            if copy_daemon {
                info!("copying daemon");
                keep_it_focused::setup::copy_daemon().context("failed to copy daemon")?;
            }
            if daemon {
                info!("setting up daemon");
                keep_it_focused::setup::setup_daemon(start).context("failed to copy daemon")?;
            }
            if mkdir {
                info!("setting up directory for temporary extensions");
                keep_it_focused::setup::make_extension_dir(&args.extensions).context("failed to create directory for temporary extensions")?;
            }
            info!("setup complete");
        }
        Command::Exceptionally {
            user,
            rule,
            start,
            end,
        } => {
            // Note: we expect that the configuration directory has been created already.
            // Generate config.
            let day_config = match rule {
                Rule::Domain { domain } => DayConfig {
                    web: vec![WebFilter {
                        domain,
                        permitted: vec![Interval { start, end }],
                    }],
                    ..Default::default()
                },
                Rule::Binary { binary: path } => {
                    let binary = Binary::try_new(&path)?;
                    DayConfig {
                        processes: vec![ProcessConfig {
                            binary,
                            permitted: vec![Interval { start, end }],
                        }],
                        ..Default::default()
                    }
                }
            };
            let extension = Extension {
                users: vec![(user, day_config)].into_iter().collect(),
            };
            // Create temporary buffer.
            let (path, file) = loop {
                let name = format!("{}.yaml", uuid().unwrap());
                let path = std::path::Path::join(&args.extensions, name);
                match std::fs::File::create_new(&path) {
                    Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                        // We stumbled upon an existing file, try again.
                        continue;
                    }
                    Err(err) => {
                        return Err(err).context("could not create file to write temporary rule")
                    }
                    Ok(file) => break (path, file),
                };
            };
            info!("writing rule to {}", path.display());
            serde_yaml::to_writer(file, &extension).context("failed to write extension to file")?;
        }
    }
    Ok(())
}
