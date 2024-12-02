mod config;
mod notify;
mod types;
mod uid_resolver;
mod iptables;


use std::{
    collections::HashMap,
    path::Path,
    rc::Rc,
};

use anyhow::Context;
use iptables::IPTable;
use itertools::Itertools;
use log::{debug, info, warn};
use procfs::ProcError;

use crate::{
    config::{Binary, Config},
    notify::notify,
    types::{DayOfWeek, Interval, TimeOfDay},
    uid_resolver::Uid,
};

pub const IP_TABLES_PREFIX: &str = "KEEP-IT-FOCUSED-";

#[derive(Debug, Clone)]
pub struct AcceptedInterval(Interval);
#[derive(Debug, Clone)]
pub struct RejectedInterval(Interval);

pub struct UserInstructions {
    user_name: Rc<String>,
    processes: Vec<(Binary, /* accepted */ Box<[AcceptedInterval]>)>,
    domains: Vec<(String, /* rejected */ Box<[RejectedInterval]>)>,
}
impl UserInstructions {
    pub fn new(user_name: Rc<String>) -> Self {
        UserInstructions {
            user_name,
            processes: Vec::new(),
            domains: Vec::new(),
        }
    }
}
type PerUid = HashMap<Uid, UserInstructions>;
type PerDay = HashMap<DayOfWeek, PerUid>;

pub fn init<P: AsRef<Path>>(path: &P) -> Result<PerDay, anyhow::Error> {
    info!("reading config: {}", "start");
    let reader = std::fs::File::open(path)
        .with_context(|| format!("could not open file {}", path.as_ref().to_string_lossy()))?;
    let config: Config = serde_yaml::from_reader(reader)
        .with_context(|| format!("could not parse file {}", path.as_ref().to_string_lossy()))?;

    info!("reading config: {}", "resolving");
    let mut resolver = uid_resolver::Resolver::new();
    let mut per_day: PerDay = HashMap::new();
    for (user, week) in config.users {
        let user = Rc::new(user);
        let uid = resolver.resolve(&user)?;
        for (day, day_config) in week.0 {
            let this_day = per_day.entry(day).or_default();
            let this_user = this_day
                .entry(uid)
                .or_insert_with(|| UserInstructions::new(user.clone()));
            for proc in day_config.processes {
                let intervals = proc.permitted.into_iter().map(AcceptedInterval);
                this_user.processes.push((proc.binary, intervals.collect()))
            }
            for web in day_config.web {
                let forbidden = types::complement_intervals(web.permitted)
                    .into_iter()
                    .map(RejectedInterval)
                    .collect_vec()
                    .into_boxed_slice();
                this_user.domains.push((web.domain, forbidden));
            }
        }
    }
    info!("reading config: {}", "complete");
    Ok(per_day)
}

pub fn find_offending_processes(config: &PerDay) -> Result<(), anyhow::Error> {
    let today = DayOfWeek::now();
    let Some(today_config) = config.get(&today) else {
        // Nothing to do for today.
        return Ok(());
    };
    if today_config.is_empty() {
        // Nothing to do for today.
        return Ok(());
    }

    let now = TimeOfDay::from(chrono::Local::now());
    let processes = procfs::process::all_processes()
        .context("could not access /proc, is this a Linux machine?")?;

    for proc in processes {
        // Examine process.
        let proc = match proc {
            Ok(p) => p,
            Err(err) => {
                warn!(target: "procfs", "could not access proc, skipping: {}", err);
                continue;
            }
        };
        let uid = match proc.uid() {
            Ok(uid) => Uid(uid),
            Err(err) => {
                warn!(target: "procfs", "could not access proc uid for process {pid}, skipping: {}", err, pid=proc.pid);
                continue;
            }
        };
        let Some(user_config) = today_config.get(&uid) else {
            // Nothing to watch for this user.
            continue;
        };
        let exe = match proc.exe() {
            Ok(exe) => exe,
            Err(err @ ProcError::PermissionDenied(_)) => {
                debug!(target: "procfs", "could not access proc exe for process {pid}, skipping: {}", err, pid=proc.pid);
                continue;
            }
            Err(err) => {
                warn!(target: "procfs", "could not access proc exe for process {pid}, skipping: {}", err, pid=proc.pid);
                continue;
            }
        };

        debug!("examining process {:?}", exe);
        for (binary, intervals) in &user_config.processes {
            if !binary.matcher.is_match(&exe) {
                debug!("we're not interested in this process");
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

#[derive(Debug)]
enum Domain {
    Source(String),
    Destination(String),
}
#[derive(Debug)]
struct Filter {
    day: DayOfWeek,
    uid: Uid,
    domain: Domain,
    rejection: RejectedInterval,
}

pub fn remove_ip_tables(prefix: &str) -> Result<(), anyhow::Error> {
    // We want to reset the iptables chains we use for this process.
    // The only way to do this, apparently, is to request the list and filter.
    let chains = IPTable::builder()
        .build()
        .list(true, Some(prefix))
        .context("failed to list existing chains")?;

    if chains.is_empty() {
        debug!("remove_ip_tables: nothing to remove")
    }
    for chain_name in chains {
        debug!("remove_ip_tables: removing chain {}", chain_name);
        IPTable::builder()
            .build()
            .flush(&chain_name)
            .context("failed to reset iptables chain")?;

        IPTable::builder()
            .build()
            .delete(&chain_name)
            .context("failed to drop iptables chain")?;
    }
    Ok(())
}

pub fn setup_ip_tables(config: &PerDay) -> Result<(), anyhow::Error> {
    info!("populating web filter: {}", "start");
    remove_ip_tables(IP_TABLES_PREFIX)?;

    info!("populating web filter: {}", "compiling chains");
    // Compile to individual chains.
    let mut chains = Vec::new();
    for (day, this_day) in config {
        for (uid, instructions) in this_day {
            for (domain, rejected) in &instructions.domains {
                for rejection in rejected {
                    chains.push(Filter {
                        day: *day,
                        uid: *uid,
                        domain: Domain::Destination(domain.clone()),
                        rejection: rejection.clone(),
                    });
                    chains.push(Filter {
                        day: *day,
                        uid: *uid,
                        domain: Domain::Source(domain.clone()),
                        rejection: rejection.clone(),
                    });
                }
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
        chain.append(iptables::Filter::Time { day: Some(filter.day), start: Some(filter.rejection.0.start), end: Some(filter.rejection.0.end) })
            .with_context(|| format!("failed to create time rule for {filter:?}"))?;

        // 2. If this is not a user we're watching, this chain doesn't apply.
        chain.append(iptables::Filter::Owner { uid: filter.uid })
            .with_context(|| format!("failed to create user rule for {filter:?}"))?;

        // 3. If this is not a domain we're watching, this chain doesn't apply.
        match filter.domain {
            Domain::Source(ref source) =>
                chain.append(iptables::Filter::Source { domain: source }),
            Domain::Destination(ref dest) =>
                chain.append(iptables::Filter::Destination { domain: dest }),
        }
           .with_context(|| format!("failed to create domain rule for {filter:?}"))?;

        // ... If the chain still applies, it means that the domain is currently forbidden for the user!
        chain.finish(iptables::Finish::Drop)
            .with_context(|| format!("failed to terminate rule for {filter:?}"))?;
    }
    info!("populating web filter: {}", "done");
    Ok(())
}
