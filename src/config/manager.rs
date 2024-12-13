use std::{
    collections::HashMap, ops::Not, path::{Path, PathBuf}, rc::Rc, time::{SystemTime, UNIX_EPOCH}
};

use anyhow::Context;
use chrono::{DateTime, Datelike, Local};
use itertools::Itertools;
use log::{debug, info, warn};

use crate::{
    config::{Binary, Config, Extension},
    types::{
        is_today, AcceptedInterval, DayOfWeek, Domain, IntervalsDiff, RejectedInterval, Username,
    },
    uid_resolver::{self, Uid},
    UserInstructions,
};

use super::DayConfig;

#[derive(Debug)]
struct CacheEntry {
    /// When the file was last changed and read.
    latest_update: SystemTime,

    /// Whtn the file was created
    creation_date: SystemTime,

    /// Contents last read from that file.
    config: HashMap<Username, DayConfig>,
}

pub struct Options {
    pub main_config: PathBuf,
    pub extensions_dir: PathBuf,
}

#[derive(Debug, Default, Clone)]
pub struct Precompiled {
    today_per_user: HashMap<Uid, UserInstructions>,
}
impl Precompiled {
    /// Serialize the web component to JSON, fit for serving.
    pub fn serialize_web(&self) -> HashMap<Uid, String> {
        debug!("serializing {:?}", self);
        let data = self
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
    pub fn today_per_user(&self) -> &HashMap<Uid, UserInstructions> {
        &self.today_per_user
    }
}

pub struct ConfigManager {
    /// A compiled instance of the configuration, collated from all the currently valid configuraiton
    /// files.
    config: Precompiled,

    /// A cache from configuration files -> entries.
    cache: HashMap<PathBuf, CacheEntry>,

    /// When `config` was last computed.
    last_computed: DateTime<Local>,

    options: Options,
}
impl ConfigManager {
    pub fn new(options: Options) -> Self {
        Self {
            cache: HashMap::new(), // Data will be filled once we have executed `load_config()`.
            config: Precompiled::default(), // Data will be filled once we have executed `load_config()`.
            last_computed: DateTime::from_timestamp_micros(0).unwrap().into(), // Expect that we're running *after* the epoch.
            options,
        }
    }

    pub fn config(&self) -> &Precompiled {
        &self.config
    }

    fn fetch_and_cache<F>(
        &mut self,
        path: PathBuf,
        today_only: bool,
        read: F,
    ) -> Result<bool, anyhow::Error>
    where
        F: FnOnce(std::fs::File) -> Result<HashMap<Username, DayConfig>, anyhow::Error>,
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

    pub fn load_config(&mut self) -> Result<bool, anyhow::Error> {
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
            ips: HashMap<Domain, Vec<IntervalsDiff>>,
            web: HashMap<Domain, Vec<IntervalsDiff>>,
        }
        let mut today_per_user: HashMap</* user */ Rc<Username>, TodayPerUser> = HashMap::new();
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

    pub fn today_per_user(&self) -> &HashMap<Uid, UserInstructions> {
        &self.config.today_per_user
    }
}
