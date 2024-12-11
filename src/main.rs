use std::{io::ErrorKind, ops::Not, path::PathBuf, thread};

use anyhow::Context;
use clap::{ArgAction, Parser, Subcommand};
use keep_it_focused::{
    config::{Binary, Config, Extension, ProcessConfig, WebFilter},
    types::{DayOfWeek, Interval},
    uid_resolver::{Resolver, Uid},
    KeepItFocused,
};
use log::{debug, info, warn, LevelFilter};
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
        #[arg(long, default_value = "true", action=ArgAction::Set)]
        policies: bool,

        /// If true, setup daemon for start.
        #[arg(long, default_value = "true", action=ArgAction::Set)]
        daemon: bool,

        /// If true, start daemon now (requires `daemon``).
        #[arg(long, default_value = "true", action=ArgAction::Set)]
        start: bool,

        /// If true, copy addon to /etc/firefox/addons
        #[arg(long, default_value = "true", action=ArgAction::Set)]
        copy_addon: bool,

        /// If true, copy daemon to /usr/bin
        #[arg(long, default_value = "true", action=ArgAction::Set)]
        copy_daemon: bool,

        /// If true, create extension directory
        #[arg(long, default_value = "true", action=ArgAction::Set)]
        mkdir: bool,
    },

    /// Add a temporary rule.
    Exceptionally {
        #[arg(long)]
        user: Option<String>,

        #[command(subcommand)]
        rule: Rule,

        /// When the authorization starts.
        #[arg(long, value_parser=keep_it_focused::types::TimeOfDay::parse)]
        start: keep_it_focused::types::TimeOfDay,

        /// When the authorization stops.
        #[arg(long, value_parser=keep_it_focused::types::TimeOfDay::parse)]
        end: keep_it_focused::types::TimeOfDay,
    },

    /// Add a permanent rule.
    Permanently {
        #[arg(long)]
        user: String,

        #[command(subcommand)]
        rule: Rule,

        #[arg(long, value_parser=keep_it_focused::types::DayOfWeek::parse, required=true)]
        days: Vec<DayOfWeek>,

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
        domains: Vec<String>,
    },
    Binary {
        /// The binary (globs are permitted).
        binaries: Vec<String>,
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
            _ => LevelFilter::Debug,
        };
        log::set_max_level(max_level);
    } else {
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
                .context("Error while creating or setting up temporary rules directory")?;

            info!("loop: {}", "starting");
            let mut focuser = keep_it_focused::KeepItFocused::try_new(keep_it_focused::Options {
                ip_tables,
                port,
                main_config: args.main_config,
                extensions_dir: args.extensions,
            })
            .context("Failed to apply configuration")?;
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
                    .context("Failed to setup policies.json")?;
            }
            if copy_addon {
                info!("copying addon");
                keep_it_focused::setup::copy_addon().context("Failed to copy addon xpi")?;
            }
            if copy_daemon {
                info!("copying daemon");
                keep_it_focused::setup::copy_daemon().context("Failed to copy daemon")?;
            }
            if daemon {
                info!("setting up daemon");
                keep_it_focused::setup::setup_daemon(start).context("Failed to copy daemon")?;
            }
            if mkdir {
                info!("setting up directory for temporary extensions");
                keep_it_focused::setup::make_extension_dir(&args.extensions)
                    .context("Failed to create directory for temporary extensions")?;
            }
            info!("setup complete");
        }
        Command::Permanently {
            user,
            rule,
            days,
            start,
            end,
        } => {
            if Uid::me().is_root().not() {
                warn!("this command is meant to be executed as root");
            }
            Resolver::new().resolve(&user).context("Invalid user")?;

            // 1. Pick a temporary file.
            let temp_dir = std::env::temp_dir();
            let (temp_file, file) = loop {
                let name = format!("{}.yaml", uuid().unwrap());
                let path = std::path::Path::join(&temp_dir, name);
                match std::fs::File::create_new(&path) {
                    Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                        // We stumbled upon an existing file, try again.
                        continue;
                    }
                    Err(err) => {
                        return Err(err).context("Could not create file to write temporary rule")
                    }
                    Ok(file) => break (path, file),
                };
            };

            // 2. Read existing config.
            let input = std::fs::File::open(&args.main_config)
                .context("Failed to open main configuration")?;
            let mut config: Config = serde_yaml::from_reader(std::io::BufReader::new(input))
                .context("Failed to read/parse main configuration")?;
            let entry = config.users.entry(user).or_default();

            // 2. Amend it to a temporary file.
            //
            // Using a temporary file:
            // 1. Lets us perform a quick check that we're not breaking things too obviously.
            // 2. Decreases the chances of two concurrent changes causing us to end up with a
            //    broken /etc/keep-it-focused.yaml.
            // 3. Decreases (but does not eliminate) the chances of a power outage while a change
            //    causing a broken /etc/keep-it-focused.yaml.
            for day in days {
                let day_config = entry.0.entry(day).or_default();
                match rule {
                    Rule::Domain { ref domains } => {
                        for domain in domains {
                            day_config.web.push(WebFilter {
                                domain: domain.clone(),
                                permitted: vec![Interval { start, end }],
                            });
                        }
                    }
                    Rule::Binary { ref binaries } => {
                        for path in binaries {
                            let binary = Binary::try_new(path.as_ref())?;
                            day_config.processes.push(ProcessConfig {
                                binary: binary.clone(),
                                permitted: vec![Interval { start, end }],
                            });
                        }
                    }
                };
            }
            debug!("preparing to write new file {:?}", config);
            serde_yaml::to_writer(std::io::BufWriter::new(file), &config)
                .context("Failed to write temporary file")?;

            // 3. Check that we're not going to break keep-it-focused.
            let mut simulator = KeepItFocused::try_new(keep_it_focused::Options {
                ip_tables: false,
                port: 2425,
                main_config: temp_file.clone(),
                extensions_dir: args.extensions,
            })
            .context("Failed to launch checker")?;
            simulator
                .tick()
                .context("Could not process change, rolling back")?;

            // 4. Finally, commit change.
            //
            // Again, this is still a race condition.
            info!("committing change");
            std::fs::rename(temp_file, args.main_config).context("Failed to commit changes")?;
        }
        Command::Exceptionally {
            user,
            rule,
            start,
            end,
        } => {
            if Uid::me().is_root().not() {
                warn!("this command is meant to be executed as root");
            }

            let user = match user {
                Some(user) => user,
                None => Uid::me().name()?,
            };
            // Note: we expect that the configuration directory has been created already.
            // Generate config.
            let mut extension = Extension::default();
            let day_config = extension.users.entry(user).or_default();
            match rule {
                Rule::Domain { domains } => {
                    for domain in domains {
                        day_config.web.push(WebFilter {
                            domain,
                            permitted: vec![Interval { start, end }],
                        });
                    }
                }
                Rule::Binary { binaries } => {
                    for path in binaries {
                        let binary = Binary::try_new(path.as_ref())?;
                        day_config.processes.push(ProcessConfig {
                            binary,
                            permitted: vec![Interval { start, end }],
                        });
                    }
                }
            };
            debug!("extension {:?}", extension);
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
                        return Err(err).context("Could not create file to write temporary rule")
                    }
                    Ok(file) => break (path, file),
                };
            };
            info!("writing rule to {}", path.display());
            serde_yaml::to_writer(file, &extension).context("Failed to write extension to file")?;
        }
    }
    Ok(())
}
