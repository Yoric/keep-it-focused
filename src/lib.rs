pub mod config;

mod server;
pub mod setup;
pub mod types;
#[cfg(target_family = "unix")]
pub mod unix;

use std::{collections::HashMap, ops::Not, path::PathBuf, rc::Rc, sync::Arc};

use anyhow::Context;
use config::manager::ConfigManager;
use log::{debug, info, warn};
use serde::Serialize;
use server::Server;
use typed_builder::TypedBuilder;
use types::{AcceptedInterval, Domain, RejectedInterval, Username};

use crate::{config::Binary, types::TimeOfDay};

#[cfg(target_os = "linux")]
use crate::unix::linux::notify::{notify, Urgency};
#[cfg(target_family = "unix")]
use crate::unix::uid_resolver::{self, Uid};

#[derive(Serialize, Debug, Clone)]
pub struct UserInstructions {
    user_name: Rc<Username>,
    processes: Vec<(Binary, Vec<AcceptedInterval>)>,
    ips: HashMap<Domain, Vec<RejectedInterval>>,
    web: HashMap<Domain, Vec<AcceptedInterval>>,
}
impl UserInstructions {
    pub fn new(user_name: Rc<Username>) -> Self {
        UserInstructions {
            user_name,
            processes: Vec::new(),
            ips: HashMap::new(),
            web: HashMap::new(),
        }
    }
}

#[derive(TypedBuilder, Debug)]
pub struct Options {
    #[builder(default = false)]
    pub ip_tables: bool,
    pub port: u16,

    pub main_config: PathBuf,
    pub extensions_dir: PathBuf,
}

pub struct KeepItFocused {
    /// Runtime options.
    options: Options,

    /// A component in charge of (re)loading the configuration
    config: ConfigManager,

    /// A minimal HTTP server running on its own thread to serve web filters to web browsers.
    server: Arc<Server>,
}

impl KeepItFocused {
    pub fn try_new(options: Options) -> Result<Self, anyhow::Error> {
        debug!("options: {:?}", options);
        let mut me = Self {
            server: Arc::new(Server::new(HashMap::new(), options.port)),
            config: ConfigManager::new(config::manager::Options {
                main_config: options.main_config.clone(),
                extensions_dir: options.extensions_dir.clone(),
            }),
            options,
        };
        // Load the configuration and pass it to `server`
        me.tick()?;
        Ok(me)
    }

    pub fn tick(&mut self) -> Result<(), anyhow::Error> {
        // Load any change.
        let has_changes = match self.config.load_config() {
            Err(err) => {
                warn!("Failed to reload config, keeping previous config: {}", err);
                false
            }
            Ok(has_changes) => has_changes,
        };

        // Update server data.
        if has_changes {
            let data = self.config.config().serialize_web();
            self.server
                .update_data(data)
                .context("Failed to register data to serve, was the server stopped?")?;
            if self.options.ip_tables {
                self.apply_ip_tables()
                    .context("Failed to update ip tables")?;
            }
        }
        self.find_offending_processes()
    }

    #[cfg(not(feature = "ip_tables"))]
    fn apply_ip_tables(&mut self) -> Result<(), anyhow::Error> {
        if self
            .config
            .today_per_user()
            .values()
            .any(|user| user.ips.is_empty().not())
        {
            warn!("this binary was compiled WITHOUT support for ip tables")
        }
        Ok(())
    }

    #[cfg(feature = "ip_tables")]
    fn apply_ip_tables(&mut self) -> Result<(), anyhow::Error> {
        #[derive(Debug)]
        enum Domain {
            Source(String),
            Destination(String),
        }
        #[derive(Debug)]
        struct Filter {
            uid: Uid,
            domain: Domain,
            rejection: RejectedInterval,
        }

        info!("populating web filter: {}", "start");
        remove_ip_tables(IP_TABLES_PREFIX)?;

        info!("populating web filter: {}", "compiling chains");
        // Compile to individual chains.
        let mut chains = Vec::new();
        for (uid, instructions) in &self.config.today_per_user {
            for (domain, rejected) in &instructions.ips {
                for rejection in rejected {
                    chains.push(Filter {
                        uid: *uid,
                        domain: Domain::Destination(domain.clone()),
                        rejection: rejection.clone(),
                    });
                    chains.push(Filter {
                        uid: *uid,
                        domain: Domain::Source(domain.clone()),
                        rejection: rejection.clone(),
                    });
                }
            }
        }

        for (index, filter) in chains.into_iter().enumerate() {
            let chain_name = format!("{IP_TABLES_PREFIX}{index}");
            info!("populating web filter: {}", "inserting chain");
            // Create new chain.
            let mut chain = IPTable::builder()
                .build()
                .create(&chain_name)
                .with_context(|| format!("failed to create table for {filter:?}"))?;

            // Populate it.

            // 1. If we're not during an interval of interest, this chain doesn't apply.
            chain
                .append(iptables::Filter::Time {
                    start: Some(filter.rejection.0.start),
                    end: Some(filter.rejection.0.end),
                })
                .with_context(|| format!("failed to create time rule for {filter:?}"))?;

            // 2. If this is not a user we're watching, this chain doesn't apply.
            chain
                .append(iptables::Filter::Owner { uid: filter.uid })
                .with_context(|| format!("failed to create user rule for {filter:?}"))?;

            // 3. If this is not a domain we're watching, this chain doesn't apply.
            match filter.domain {
                Domain::Source(ref source) => {
                    chain.append(iptables::Filter::Source { domain: source })
                }
                Domain::Destination(ref dest) => {
                    chain.append(iptables::Filter::Destination { domain: dest })
                }
            }
            .with_context(|| format!("failed to create domain rule for {filter:?}"))?;

            // ... If the chain still applies, it means that the domain is currently forbidden for the user!
            chain
                .finish(iptables::Finish::Drop)
                .with_context(|| format!("failed to terminate rule for {filter:?}"))?;
        }
        info!("populating web filter: {}", "done");
        Ok(())
    }

    pub fn background_serve(&self) {
        let server = self.server.clone();
        std::thread::spawn(move || server.serve_blocking());
    }

    fn find_offending_processes(&self) -> Result<(), anyhow::Error> {
        if self.config.today_per_user().is_empty() {
            // Nothing to do for today.
            debug!("find offending processes: no configuration for the day, skipping");
            return Ok(());
        }

        let now = TimeOfDay::now();
        // FIXME: All of this should move to a Linux-specific module.
        let processes = procfs::process::all_processes()
            .context("Could not access /proc, is this a Linux machine?")?;

        for proc in processes {
            // Examine process. We may not have access to all processes, e.g. if they're zombies,
            // or being killed while we look, etc. We don't really care, just skip a process if we
            // can't examine it.
            /*
                       try:
                           proc = proc.get()
                       with:
                           continue
            */
            let Ok(proc) = proc else { continue };
            let Ok(uid) = proc.uid() else { continue };
            let uid = Uid(uid);
            let Some(user_config) = self.config.today_per_user().get(&uid) else {
                // Nothing to watch for this user.
                continue;
            };
            let Ok(exe) = proc.exe() else { continue };

            for (binary, intervals) in &user_config.processes {
                if !binary.matcher.is_match(&exe) {
                    continue;
                }
                info!(
                    "found binary {} for user {}",
                    exe.to_string_lossy(),
                    user_config.user_name
                );
                if let Some(duration) = intervals
                    .iter()
                    .filter_map(|interval| interval.0.remaining(now))
                    .next()
                {
                    // We're still in permitted territory.
                    info!("binary is still allowed at this time");
                    if duration < std::time::Duration::from_secs(300) {
                        // ...however, we're less than 5 minutes away from shutdown, so let's warn user!
                        let minutes = duration.as_secs() / 60;
                        if let Err(err) = notify(
                            user_config.user_name.as_str(),
                            &format!("{} will quit in {} minutes", exe.to_string_lossy(), minutes),
                            Urgency::Significant,
                        ) {
                            warn!(target: "notify", "failed to notify user {}: {:?}", user_config.user_name, err)
                        }
                    }
                } else {
                    info!("let's kill this binary");
                    // Time to kill the binary.
                    if let Err(err) = notify(
                        &user_config.user_name,
                        &format!(
                            "{} is not permitted at this time, stopping it",
                            exe.to_string_lossy()
                        ),
                        Urgency::Significant,
                    ) {
                        warn!(target: "notify", "failed to notify user {}: {:?}", user_config.user_name, err)
                    }
                    if let Err(err) = kill_tree::blocking::kill_tree_with_config(
                        proc.pid as u32,
                        &kill_tree::Config {
                            signal: "SIGKILL".to_string(),
                            ..Default::default()
                        },
                    ) {
                        warn!(target: "notify", "failed to kill process {}: {:?}", exe.to_string_lossy(), err)
                    }
                    info!("binary killed");
                }
            }
        }
        Ok(())
    }
}

#[cfg(not(feature = "ip_tables"))]
pub fn remove_ip_tables() -> Result<(), anyhow::Error> {
    Err(anyhow::anyhow!(
        "this application was compiled without support for iptables"
    ))
}

#[cfg(feature = "ip_tables")]
pub fn remove_ip_tables() -> Result<(), anyhow::Error> {
    // We want to reset the iptables chains we use for this process.
    // The only way to do this, apparently, is to request the list and filter.
    let chains = IPTable::builder()
        .build()
        .list(true, Some(iptables::IP_TABLES_PREFIX))
        .context("Failed to list existing chains")?;

    if chains.is_empty() {
        debug!("remove_ip_tables: nothing to remove")
    }
    for chain_name in chains {
        debug!("remove_ip_tables: removing chain {}", chain_name);
        IPTable::builder()
            .build()
            .flush(&chain_name)
            .context("Failed to reset iptables chain")?;

        IPTable::builder()
            .build()
            .delete(&chain_name)
            .context("Failed to drop iptables chain")?;
    }
    Ok(())
}
