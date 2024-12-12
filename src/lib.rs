pub mod config;

#[cfg(feature = "ip_tables")]
mod iptables;
mod notify;
mod serve;
pub mod setup;
pub mod types;
pub mod uid_resolver;

use std::{
    collections::HashMap,
    ops::Not,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context;
use chrono::{DateTime, Datelike, Local};
use config::Extension;
use itertools::Itertools;
use log::{debug, info, warn};
use serde::Serialize;
use typed_builder::TypedBuilder;
use types::{AcceptedInterval, IntervalsDiff, RejectedInterval};

use crate::{
    config::{Binary, Config},
    notify::notify,
    types::{DayOfWeek, TimeOfDay},
    uid_resolver::Uid,
};

#[derive(Serialize, Debug, Clone)]
pub struct UserInstructions {
    user_name: Rc<String>,
    processes: Vec<(Binary, /* accepted */ Vec<AcceptedInterval>)>,
    ips: HashMap<String, /* rejected */ Vec<RejectedInterval>>,
    web: HashMap</* domains */ String, /* rejected */ Vec<AcceptedInterval>>,
}
impl UserInstructions {
    pub fn new(user_name: Rc<String>) -> Self {
        UserInstructions {
            user_name,
            processes: Vec::new(),
            ips: HashMap::new(),
            web: HashMap::new(),
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Precompiled {
    today_per_user: HashMap<Uid, UserInstructions>,
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

    /// A compiled instance of the configuration, collated from all the currently valid configuraiton
    /// files.
    config: Precompiled,

    /// A minimal HTTP server running on its own thread to serve web filters to web browsers.
    server: Arc<serve::Server>,

    /// A cache from configuration files -> entries.
    cache: HashMap<PathBuf, CacheEntry>,

    /// When `config` was last computed.
    last_computed: DateTime<Local>,
}

#[derive(Debug)]
struct CacheEntry {
    /// When the file was last changed and read.
    latest_update: SystemTime,

    /// Whtn the file was created
    creation_date: SystemTime,

    /// Contents last read from that file.
    config: HashMap<String /* username */, config::DayConfig>,
}

impl KeepItFocused {
    pub fn try_new(options: Options) -> Result<Self, anyhow::Error> {
        debug!("options: {:?}", options);
        let data = HashMap::new();
        let server = Arc::new(serve::Server::new(data, options.port));
        #[allow(unused_mut)]
        let mut me = Self {
            options,
            server, // Data will be filled once we have executed `load_config()`.
            cache: HashMap::new(), // Data will be filled once we have executed `load_config()`.
            config: Precompiled::default(), // Data will be filled once we have executed `load_config()`.
            last_computed: DateTime::from_timestamp_micros(0).unwrap().into(), // Expect that we're running *after* the epoch.
        };
        me.tick()?;
        Ok(me)
    }

    fn serialize(config: &Precompiled) -> HashMap<Uid, String> {
        debug!("serializing {:?}", config);
        let data = config
            .today_per_user
            .iter()
            .map(|(uid, instructions)| {
                (*uid, {
                    serde_json::to_string(&instructions.web).expect("error during serialization")
                })
            })
            .collect();
        data
    }

    pub fn tick(&mut self) -> Result<(), anyhow::Error> {
        // Load any change.
        let has_changes = match self.load_config() {
            Err(err) => {
                warn!("Failed to reload config, keeping previous config: {}", err);
                false
            }
            Ok(has_changes) => has_changes,
        };

        // Update server data.
        if has_changes {
            let data = Self::serialize(&self.config);
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
            .today_per_user
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

    fn fetch_and_cache<F>(
        &mut self,
        path: PathBuf,
        today_only: bool,
        read: F,
    ) -> Result<bool, anyhow::Error>
    where
        F: FnOnce(
            std::fs::File,
        )
            -> Result<HashMap<String /* username */, config::DayConfig>, anyhow::Error>,
    {
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("could not access configuration at {}", path.display()))?;
        let latest_update = metadata
            .modified()
            .with_context(|| format!("no latest modification time for {}", path.display()))?;
        if today_only && is_today(latest_update).not() {
            // This file has been modified before today, so it's obsolete, remove from cache.
            debug!(
                "File {} was modified before today, removing from cache and disk",
                path.display()
            );
            self.cache.remove(&path);
            if let Err(err) = std::fs::remove_file(&path) {
                warn!("failed to remove file {}: {err}", path.display());
            }
            return Ok(true);
        }

        let creation_date = metadata
            .created()
            .with_context(|| format!("no creation time for {}", path.display()))?;
        let entry = self
            .cache
            .entry(path.clone())
            .or_insert_with(|| CacheEntry {
                latest_update: UNIX_EPOCH,
                creation_date,
                config: HashMap::default(),
            });
        if latest_update <= entry.latest_update {
            // No change, keep cache.
            return Ok(false);
        }
        let reader = std::fs::File::open(&path)
            .with_context(|| format!("could not open file {}", path.to_string_lossy()))?;
        let data = read(reader)
            .with_context(|| format!("could not parse file {}", path.to_string_lossy()))?;
        entry.config = data;
        entry.latest_update = latest_update;
        Ok(true)
    }

    fn load_config(&mut self) -> Result<bool, anyhow::Error> {
        let today = DayOfWeek::now();

        let mut has_changes = false;

        // 1. Load main file.
        info!("reading config: loading main file");
        has_changes |= self.fetch_and_cache(self.options.main_config.clone(), false, |file| {
            let config: Config = serde_yaml::from_reader(file).context("Invalid format")?;
            let mut result = HashMap::new();
            for (user, mut week) in config.users {
                if let Some(day_config) = week.0.remove(&today) {
                    debug!(
                        "processing user {user} - we have a rule for today {:?}",
                        day_config
                    );
                    result.insert(user, day_config);
                } else {
                    debug!("processing user {user} - no rule for today");
                }
            }
            Ok(result)
        })?;
        debug!(
            "reading config: loading main file, {}",
            if has_changes { "changed" } else { "unchanged" }
        );

        // 2. Load other files from the directory, ignoring any error
        // (along the way, we purge from the cache directory files that are now old).
        info!("reading config: loading extensions");
        match std::fs::read_dir(&self.options.extensions_dir) {
            Err(err) => {
                warn!(
                    "failed to open directory {}, skipping extensions: {}",
                    self.options.extensions_dir.display(),
                    err
                );
            }
            Ok(dir) => {
                for entry in dir {
                    match entry {
                        Err(err) => warn!(
                            "failed to access entry in directory {}, skipping: {}",
                            self.options.extensions_dir.display(),
                            err
                        ),
                        Ok(entry) => {
                            let path = Path::join(&self.options.extensions_dir, entry.file_name());
                            match self.fetch_and_cache(path.clone(), true, |file| {
                                let config: Extension = serde_yaml::from_reader(file)
                                    .context("Error reading/parsing file")?;
                                Ok(config.users)
                            }) {
                                Ok(changes) => has_changes |= changes,
                                Err(err) => {
                                    warn!(
                                        "error while reading {}, skipping: {}",
                                        path.display(),
                                        err
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        debug!(
            "reading config: loading extensions, {}",
            if has_changes { "changed" } else { "unchanged" }
        );

        // 3. Purge from memory any file that hasn't been modified today (except for the main file).
        debug!("reading config: purging old content");
        let before = self.cache.len();
        self.cache.retain(|path, entry| {
            is_today(entry.latest_update) || path == &self.options.main_config
        });
        let after = self.cache.len();
        if after != before {
            // We have eliminated at least one entry.
            has_changes = true;
            debug!("reading config: purging old content, we have elimited at least one entry");
        } else {
            debug!("reading config: purging old content, no old content to purge");
        }

        // 4. Compile all these files.
        info!("reading config: resolving {:?}", self.cache);
        let now = Local::now();
        if has_changes || self.last_computed.day() != now.day() {
            // We need to recompile today's config if there have been changes or whenever a new day starts.
            self.config =
                Self::compile(&self.cache).context("error while compiling the configuration")?;
            self.last_computed = now;
        }
        Ok(has_changes)
    }

    /// Resolve the cache
    ///
    /// - restrict to the current day of the week;
    /// - restrict to
    fn compile(cache: &HashMap<PathBuf, CacheEntry>) -> Result<Precompiled, anyhow::Error> {
        let mut resolver = uid_resolver::Resolver::new();
        #[derive(Default)]
        struct TodayPerUser {
            processes: HashMap<Binary, Vec<IntervalsDiff>>,
            ips: HashMap<String, Vec<IntervalsDiff>>,
            web: HashMap</* domains */ String, Vec<IntervalsDiff>>,
        }
        let mut today_per_user: HashMap</* user */ Rc<String>, TodayPerUser> = HashMap::new();
        let entries = cache.values().sorted_by_key(|entry| entry.creation_date);
        for entry in entries {
            for (user, day_config) in &entry.config {
                let user_name = Rc::new(user.clone());
                let user_entry = today_per_user.entry(user_name.clone()).or_default();
                for proc in &day_config.processes {
                    let accepted = proc
                        .permitted
                        .iter()
                        .cloned()
                        .map(AcceptedInterval)
                        .collect_vec();
                    let rejected = proc
                        .forbidden
                        .iter()
                        .cloned()
                        .map(RejectedInterval)
                        .collect_vec();
                    user_entry
                        .processes
                        .entry(proc.binary.clone())
                        .or_default()
                        .push(IntervalsDiff { accepted, rejected });
                }
                for ip in &day_config.ip {
                    let accepted = ip
                        .permitted
                        .iter()
                        .cloned()
                        .map(AcceptedInterval)
                        .collect_vec();
                    let rejected = ip
                        .forbidden
                        .iter()
                        .cloned()
                        .map(RejectedInterval)
                        .collect_vec();
                    user_entry
                        .ips
                        .entry(ip.domain.clone())
                        .or_default()
                        .push(IntervalsDiff { accepted, rejected });
                }
                for web in &day_config.web {
                    let accepted = web
                        .permitted
                        .iter()
                        .cloned()
                        .map(AcceptedInterval)
                        .collect_vec();
                    let rejected = web
                        .forbidden
                        .iter()
                        .cloned()
                        .map(RejectedInterval)
                        .collect_vec();
                    user_entry
                        .web
                        .entry(web.domain.clone())
                        .or_default()
                        .push(IntervalsDiff { accepted, rejected });
                }
            }
        }

        // Now resolve intervals and usernames.
        let mut resolved = Precompiled {
            today_per_user: HashMap::new(),
        };
        for (user_name, user_entry) in today_per_user {
            let Ok(uid) = resolver.resolve(&user_name) else {
                warn!("failed to resolve user name {user_name}");
                continue;
            };
            let mut per_user = UserInstructions::new(user_name);
            for (domain, intervals) in user_entry.ips {
                let resolved = IntervalsDiff::compute_rejected_intervals(intervals);
                per_user.ips.insert(domain, resolved);
            }
            for (binary, intervals) in user_entry.processes {
                let resolved = IntervalsDiff::compute_accepted_intervals(intervals);
                per_user.processes.push((binary, resolved));
            }
            for (domain, intervals) in user_entry.web {
                let resolved = IntervalsDiff::compute_accepted_intervals(intervals);
                debug!("domain {domain}: resolving intervals => {resolved:?}");
                per_user.web.insert(domain, resolved);
            }
            resolved.today_per_user.insert(uid, per_user);
        }
        info!("reading config: {}", "complete");
        Ok(resolved)
    }

    pub fn background_serve(&self) {
        let server = self.server.clone();
        std::thread::spawn(move || server.serve_blocking());
    }

    fn find_offending_processes(&self) -> Result<(), anyhow::Error> {
        if self.config.today_per_user.is_empty() {
            // Nothing to do for today.
            debug!("find offending processes: no configuration for the day, skipping");
            return Ok(());
        }

        let now = TimeOfDay::from(chrono::Local::now());
        let processes = procfs::process::all_processes()
            .context("Could not access /proc, is this a Linux machine?")?;

        for proc in processes {
            // Examine process. We may not have access to all processes, e.g. if they're zombies,
            // or being killed while we look, etc. We don't really care.
            let Ok(proc) = proc else { continue };
            let Ok(uid) = proc.uid() else { continue };
            let uid = Uid(uid);
            let Some(user_config) = self.config.today_per_user.get(&uid) else {
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
                            &user_config.user_name,
                            &format!("{} will quit in {} minutes", exe.to_string_lossy(), minutes),
                            notify::Urgency::Significant,
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
                        notify::Urgency::Significant,
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

fn is_today(date: SystemTime) -> bool {
    let latest_update_chrono = DateTime::<Local>::from(date);
    let today = Local::now();
    today.num_days_from_ce() == latest_update_chrono.num_days_from_ce()
}
