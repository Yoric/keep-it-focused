use std::{
    io::ErrorKind,
    ops::{Deref, Not},
    path::PathBuf,
    thread,
};

use anyhow::Context;
use clap::{ArgAction, Parser, Subcommand};
use log::{debug, info, warn, LevelFilter};
use procfs::sys::kernel::random::uuid;
use systemd_journal_logger::{connected_to_journal, JournalLog};

use keep_it_focused::{
    config::{Binary, Config, Extension, ProcessFilter, WebFilter, manager::{ConfigManager, Options as ConfigOptions}},
    types::{DayOfWeek, Domain, Interval, TimeOfDay, Username},
    KeepItFocused,
};

const DEFAULT_CONFIG_PATH: &str = "/etc/keep-it-focused.yaml";
const DEFAULT_EXTENSIONS_PATH: &str = "/tmp/keep-it-focused.d/";
const DEFAULT_PORT: &str = "7878";

#[cfg(target_family="unix")]
use keep_it_focused::unix::uid_resolver::{Resolver, Uid};


#[derive(Subcommand, Debug)]
enum Command {
    /// Check the configuration for syntax.
    Check {
        /// If specified, display today's configuration for this user.
        user: Option<String>
    },

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
        #[command(subcommand)]
        verb: Verb<ExceptionalFilter>,
    },

    /// Add a permanent rule.
    Permanently {
        #[command(subcommand)]
        verb: Verb<PermanentFilter>,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum Kind {
    Domain {
        /// The domain, e.g. "youtube.com" (subdomains are included automatically).
        #[arg(required = true)]
        domains: Vec<String>,
    },
    Binary {
        /// The binary, e.g. "**/tetris" (globs are permitted).
        #[arg(required = true)]
        binaries: Vec<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
enum Verb<I>
where
    I: clap::Args + std::fmt::Debug + Clone,
{
    /// Allow an interval of time.
    Allow(I),

    /// Forbid an interval of time.
    Forbid(I),
}
impl<I> AsRef<I> for Verb<I>
where
    I: clap::Args + std::fmt::Debug + Clone,
{
    fn as_ref(&self) -> &I {
        match *self {
            Self::Allow(ref i) => i,
            Self::Forbid(ref i) => i,
        }
    }
}
impl<I> Deref for Verb<I>
where
    I: clap::Args + std::fmt::Debug + Clone,
{
    type Target = I;
    fn deref(&self) -> &I {
        match *self {
            Self::Allow(ref i) => i,
            Self::Forbid(ref i) => i,
        }
    }
}

#[derive(clap::Args, Debug, Clone)]
struct PermanentFilter {
    #[command(subcommand)]
    kind: Kind,

    #[arg(long)]
    user: String,

    /// Which days of the week this rule is good for.
    #[arg(long, value_parser=keep_it_focused::types::DayOfWeek::parse, required=true)]
    days: Vec<DayOfWeek>,

    /// When the authorization starts.
    #[arg(long, value_parser=TimeOfDay::parse)]
    start: TimeOfDay,

    /// When the authorization stops.
    #[arg(long, value_parser=TimeOfDay::parse)]
    end: TimeOfDay,
}

#[derive(clap::Args, Debug, Clone)]
struct ExceptionalFilter {
    #[command(subcommand)]
    kind: Kind,

    #[arg(long)]
    user: String,

    /// When it starts [default: immediately].
    #[arg(long, value_parser=TimeOfDay::parse)]
    start: Option<TimeOfDay>,

    /// When it stops [default: end of day].
    #[arg(long, value_parser=TimeOfDay::parse)]
    end: Option<TimeOfDay>,

    /// How long it lasts, in minutes (conflicts with `end`).
    #[arg(long, alias="duration", conflicts_with_all=["end"])]
    minutes: Option<u16>,
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
    info!("Starting keep-it-focused {}", env!("CARGO_PKG_VERSION"));

    let args = Args::parse();
    match args.command {
        Command::IpTables { remove } => {
            if remove {
                keep_it_focused::remove_ip_tables()?;
            }
        }
        Command::Check { user } => {
            let mut configurator = ConfigManager::new(ConfigOptions {
                main_config: args.main_config,
                extensions_dir: args.extensions,
            });
            configurator.load_config()
                .context("invalid config")?;
            info!("config parsed, seems legit");
            if let Some(user) = user {
                let mut resolver = Resolver::new();
                let uid = resolver.resolve(&Username(user.clone()))?;
                match configurator.config().today_per_user().get(&uid) {
                    None => info!("on this day, no config for user {user}"),
                    Some(config) =>
                        info!("today's config for {user}\n {}", serde_yaml::to_string(&config)
                            .context("Failed to serialize")?)

                }
            }
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
        Command::Permanently { verb } => {
            if Uid::me().is_root().not() {
                warn!("this command is meant to be executed as root");
            }
            let mut resolver = Resolver::new();
            resolver.resolve(&Username(verb.as_ref().user.clone()))?;

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
                        return Err(err).context("Could not create file to write temporary rules")
                    }
                    Ok(file) => break (path, file),
                };
            };

            // 2. Read existing config.
            let input = std::fs::File::open(&args.main_config)
                .context("Failed to open main configuration")?;
            let mut config: Config = serde_yaml::from_reader(std::io::BufReader::new(input))
                .context("Failed to read/parse main configuration")?;
            let entry = config
                .users
                .entry(Username(verb.as_ref().user.clone()))
                .or_default();

            // 2. Amend it to a temporary file.
            //
            // Using a temporary file:
            // 1. Lets us perform a quick check that we're not breaking things too obviously.
            // 2. Decreases the chances of two concurrent changes causing us to end up with a
            //    broken /etc/keep-it-focused.yaml.
            // 3. Decreases (but does not eliminate) the chances of a power outage while a change
            //    causing a broken /etc/keep-it-focused.yaml.
            let intervals = vec![Interval {
                start: verb.as_ref().start,
                end: verb.as_ref().end,
            }];
            let (permitted, forbidden) = match verb {
                Verb::Allow(_) => (intervals, vec![]),
                Verb::Forbid(_) => (vec![], intervals),
            };
            match verb.as_ref().kind {
                Kind::Domain { ref domains } => {
                    for day in &verb.days {
                        let day_config = entry.0.entry(*day).or_default();
                        for domain in domains {
                            day_config.web.push(WebFilter {
                                domain: Domain(domain.clone()),
                                permitted: permitted.clone(),
                                forbidden: forbidden.clone(),
                            });
                        }
                    }
                }
                Kind::Binary { ref binaries } => {
                    for day in &verb.days {
                        let day_config = entry.0.entry(*day).or_default();
                        for path in binaries {
                            let binary = Binary::try_new(path.as_ref())?;
                            day_config.processes.push(ProcessFilter {
                                binary: binary.clone(),
                                permitted: permitted.clone(),
                                forbidden: forbidden.clone(),
                            });
                        }
                    }
                }
            };
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
        Command::Exceptionally { verb } => {
            if Uid::me().is_root().not() {
                warn!("this command is meant to be executed as root");
            }

            // Note: we expect that the configuration directory has been created already.
            // Generate config.
            let mut extension = Extension::default();
            let day_config = extension
                .users
                .entry(Username(verb.user.clone()))
                .or_default();
            let start = verb.start.unwrap_or(TimeOfDay::now());
            let end = match verb.minutes {
                Some(duration) => TimeOfDay::from_minutes(TimeOfDay::now().as_minutes() + duration),
                None => verb.end.unwrap_or(TimeOfDay::END)
            };
            let intervals = vec![Interval {
                start,
                end,
            }];
            let (permitted, forbidden) = match verb {
                Verb::Allow(_) => (intervals, vec![]),
                Verb::Forbid(_) => (vec![], intervals),
            };
            debug!("exceptionally {:?}, {:?}", permitted, forbidden);
            match &verb.kind {
                Kind::Domain { domains } => {
                    for domain in domains {
                        day_config.web.push(WebFilter {
                            domain: Domain(domain.clone()),
                            permitted: permitted.clone(),
                            forbidden: forbidden.clone(),
                        });
                    }
                }
                Kind::Binary { binaries } => {
                    for path in binaries {
                        let binary = Binary::try_new(path.as_ref())?;
                        day_config.processes.push(ProcessFilter {
                            binary: binary.clone(),
                            permitted: permitted.clone(),
                            forbidden: forbidden.clone(),
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
